//! 再帰レーティング件数のバックグラウンド集計。
//!
//! レーティングフィルタ ON のときだけ `spawn_for_folder` を呼ぶ。worker は
//! `rating.db` に対して 1 回だけ `WHERE path LIKE 'current/%' AND stars > 0` を
//! 走らせ、結果を **子孫サブフォルダ (および ZIP/PDF) 単位に集計** して UI に返す。
//!
//! # 集計キー
//!
//! - `current/sub/file.jpg` → 集計キーは **current の直下の子** `current/sub`
//! - `current/book.zip::entry.jpg` → 集計キーは `current/book.zip`
//! - `current/doc.pdf::page_0003` → 集計キーは `current/doc.pdf`
//!
//! ZIP/PDF の `::` を仮想フォルダ境界として扱い、本体のサムネイル (= 現 view の
//! 直下子要素) にバッジを乗せる。さらに奥の階層 (`current/sub/sub2/...`) は
//! `current/sub` にまとめられる (subtree 全体の合計)。
//!
//! # バッチ送信
//!
//! 大量 rating (数十万件) で UI に per-row message を送ると channel が詰まる。
//! worker は時間ベース (50ms) + 件数ベース (100 フォルダ) のバッチで emit する。
//!
//! # キャンセル
//!
//! フォルダ切替 / フィルタ OFF / App drop で `AtomicBool::store(true)` を立て、
//! worker は次のチャンク境界で break する。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rusqlite::Connection;

/// 子フォルダ / ZIP / PDF ごとの★ per-level 件数。
/// index 0 = ★1, index 4 = ★5。★0 (未評価) は集計しない。
pub type StarCounts = [u32; 5];

/// 1 バッチぶんの部分結果。`entries` は (集計キー, per-star counts)。
pub struct CountBatch {
    pub entries: Vec<(String, StarCounts)>,
    /// worker が `current/%` のスキャンを完了したかを示す。`true` で rx 側は
    /// 「未登録フォルダは 0 件で確定」と判断できる (初回ロードの `loaded` マーク用)。
    pub finished: bool,
}

/// UI 側からアクセスするハンドル。
pub struct FolderRatingCounterHandle {
    cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<CountBatch>,
}

impl FolderRatingCounterHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for FolderRatingCounterHandle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// 指定フォルダ配下 (`folder_key/...`) の子孫レーティングを集計する worker を起動する。
///
/// - `db_path`: `rating.db` の絶対パス
/// - `folder_key`: `adjustment_db::normalize_path` で正規化したフォルダパス
///   (末尾スラッシュなし)
///
/// worker は失敗時 (DB open 失敗 / 途中で cancel) でも黙って終了する。UI 側は
/// 届いたバッチを merge するだけで、届かなければ 0 件扱いになる。
pub fn spawn_for_folder(db_path: PathBuf, folder_key: String) -> FolderRatingCounterHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_w = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("folder-rating-counter".into())
        .spawn(move || {
            run_worker(&db_path, &folder_key, &cancel_w, &tx);
        });
    FolderRatingCounterHandle { cancel, rx }
}

/// `folder_key/...` を LIKE の prefix パターンにする。
/// 呼び出し元で `ESCAPE '\'` を指定すること。
fn build_prefix_pattern(folder_key: &str) -> String {
    format!("{}/%", crate::adjustment_db::escape_like_pattern(folder_key))
}

/// DB 行 (正規化された path) から、current_folder の **直下の子コンテナ** に当たる
/// 集計キーを切り出す。
///
/// - `current/sub/file.jpg` → `Some("current/sub")`
/// - `current/book.zip::entry.jpg` → `Some("current/book.zip")`
/// - `current/doc.pdf::page_0003` → `Some("current/doc.pdf")`
/// - `current/sub/nested/file.jpg` → `Some("current/sub")` (subtree 全体を sub にまとめる)
/// - `current/file.jpg` (直下のファイル) → `None` (直下ファイルはバッジ対象外)
///
/// `current_prefix` は `current_folder_key + "/"` を渡す。
pub fn aggregation_key_for<'a>(row_path: &'a str, current_prefix: &str) -> Option<&'a str> {
    let rest = row_path.strip_prefix(current_prefix)?;
    // rest の中で最初に来る `/` (= 直下サブフォルダ境界) または
    // `::` (= ZIP/PDF 仮想境界) のうち先に現れた方を採用する。
    let slash = rest.find('/');
    let colon = rest.find("::");
    let boundary = match (slash, colon) {
        (Some(s), Some(c)) => Some(s.min(c)),
        (Some(s), None) => Some(s),
        (None, Some(c)) => Some(c),
        (None, None) => None,
    };
    let boundary = boundary?;
    // boundary が 0 のケース (先頭が / や ::) は異常値として除外。
    if boundary == 0 {
        return None;
    }
    // row_path 上の絶対位置に戻す。
    let end = current_prefix.len() + boundary;
    Some(&row_path[..end])
}

fn run_worker(
    db_path: &std::path::Path,
    folder_key: &str,
    cancel: &AtomicBool,
    tx: &mpsc::Sender<CountBatch>,
) {
    let Ok(conn) = Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        let _ = tx.send(CountBatch {
            entries: Vec::new(),
            finished: true,
        });
        return;
    };
    let pattern = build_prefix_pattern(folder_key);
    let current_prefix = format!("{folder_key}/");

    // サブフォルダ (`/`) と ZIP/PDF (`::`) が混在すると ORDER BY でも同キーが
    // 連続する保証が無いので、HashMap で蓄積して一定量ごとに drain する。
    let mut buf: HashMap<String, StarCounts> = HashMap::new();
    let mut last_flush = Instant::now();
    let send_batch = |buf: &mut HashMap<String, StarCounts>, finished: bool| -> bool {
        if buf.is_empty() && !finished {
            return true;
        }
        let entries: Vec<(String, StarCounts)> = buf.drain().collect();
        tx.send(CountBatch { entries, finished }).is_ok()
    };

    let mut stmt = match conn.prepare(
        "SELECT path, stars FROM ratings \
         WHERE path LIKE ?1 ESCAPE '\\' AND stars BETWEEN 1 AND 5",
    ) {
        Ok(s) => s,
        Err(_) => {
            let _ = tx.send(CountBatch {
                entries: Vec::new(),
                finished: true,
            });
            return;
        }
    };
    let mut rows = match stmt.query(rusqlite::params![&pattern]) {
        Ok(r) => r,
        Err(_) => {
            let _ = tx.send(CountBatch {
                entries: Vec::new(),
                finished: true,
            });
            return;
        }
    };

    const FLUSH_EVERY_ROWS: usize = 4096;
    const FLUSH_INTERVAL: Duration = Duration::from_millis(50);
    let mut row_count = 0usize;

    loop {
        if cancel.load(Ordering::Relaxed) {
            // キャンセルは黙って抜ける (UI は別ハンドルを生成する)
            return;
        }
        let row = match rows.next() {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(_) => break,
        };
        let row_path: String = match row.get(0) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let stars: i32 = row.get(1).unwrap_or(0);
        if !(1..=5).contains(&stars) {
            continue;
        }
        if let Some(key) = aggregation_key_for(&row_path, &current_prefix) {
            let entry = buf.entry(key.to_string()).or_insert([0u32; 5]);
            entry[(stars - 1) as usize] = entry[(stars - 1) as usize].saturating_add(1);
        }
        row_count += 1;
        if row_count >= FLUSH_EVERY_ROWS || last_flush.elapsed() >= FLUSH_INTERVAL {
            if !send_batch(&mut buf, false) {
                return; // UI 側が rx を drop
            }
            row_count = 0;
            last_flush = Instant::now();
        }
    }
    let _ = send_batch(&mut buf, true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_pattern_escapes_sql_wildcards() {
        let p = build_prefix_pattern("c:/a_b/100%test");
        // `_` と `%` が `\` でエスケープされる。`/%` が末尾に足される。
        assert_eq!(p, "c:/a\\_b/100\\%test/%");
    }

    #[test]
    fn aggregation_key_direct_subfolder() {
        let p = "c:/root/sub/file.jpg";
        assert_eq!(
            aggregation_key_for(p, "c:/root/"),
            Some("c:/root/sub")
        );
    }

    #[test]
    fn aggregation_key_deep_descendant_rolls_up() {
        let p = "c:/root/sub/deeper/nested/file.jpg";
        assert_eq!(
            aggregation_key_for(p, "c:/root/"),
            Some("c:/root/sub")
        );
    }

    #[test]
    fn aggregation_key_zip_entry() {
        let p = "c:/root/book.zip::chapter1/page01.jpg";
        assert_eq!(
            aggregation_key_for(p, "c:/root/"),
            Some("c:/root/book.zip")
        );
    }

    #[test]
    fn aggregation_key_pdf_page() {
        let p = "c:/root/doc.pdf::page_0003";
        assert_eq!(
            aggregation_key_for(p, "c:/root/"),
            Some("c:/root/doc.pdf")
        );
    }

    #[test]
    fn aggregation_key_direct_file_returns_none() {
        // 直下のファイル (ZIP/PDF 無し、サブフォルダ無し) は
        // current_folder の「子フォルダ」には当たらないので None。
        // (一覧 UI 上の画像セル自身のレーティングは別経路 = rating_cache で描画する)
        let p = "c:/root/file.jpg";
        assert_eq!(aggregation_key_for(p, "c:/root/"), None);
    }

    #[test]
    fn aggregation_key_non_matching_prefix() {
        let p = "c:/other/sub/file.jpg";
        assert_eq!(aggregation_key_for(p, "c:/root/"), None);
    }

    #[test]
    fn aggregation_key_zip_before_slash() {
        // ZIP の :: が / より先に来るケース
        let p = "c:/root/book.zip::folder/inner.jpg";
        assert_eq!(
            aggregation_key_for(p, "c:/root/"),
            Some("c:/root/book.zip")
        );
    }

    #[test]
    fn worker_end_to_end_via_in_memory_db() {
        // 一時ファイルに rating.db を作り、worker を走らせて集計が期待通りになることを確認。
        use std::io::Write;
        let tmp = std::env::temp_dir().join(format!(
            "mimageviewer-folder-rating-test-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);
        {
            let conn = Connection::open(&tmp).unwrap();
            conn.execute_batch(
                "CREATE TABLE ratings (path TEXT PRIMARY KEY, stars INTEGER NOT NULL);
                 INSERT INTO ratings VALUES ('c:/root/sub/a.jpg', 5);
                 INSERT INTO ratings VALUES ('c:/root/sub/b.jpg', 5);
                 INSERT INTO ratings VALUES ('c:/root/sub/c.jpg', 3);
                 INSERT INTO ratings VALUES ('c:/root/sub/deep/d.jpg', 4);
                 INSERT INTO ratings VALUES ('c:/root/other/e.jpg', 2);
                 INSERT INTO ratings VALUES ('c:/root/book.zip::p01.jpg', 5);
                 INSERT INTO ratings VALUES ('c:/root/book.zip::p02.jpg', 5);
                 INSERT INTO ratings VALUES ('c:/root/doc.pdf::page_0001', 4);
                 INSERT INTO ratings VALUES ('c:/root/direct.jpg', 1);
                 INSERT INTO ratings VALUES ('c:/otherroot/x.jpg', 5);",
            )
            .unwrap();
        }
        let h = spawn_for_folder(tmp.clone(), "c:/root".to_string());
        // finished までに複数バッチ来てもいい。集計して確認する。
        let mut totals: HashMap<String, StarCounts> = HashMap::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut finished = false;
        while !finished && Instant::now() < deadline {
            match h.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(batch) => {
                    for (k, v) in batch.entries {
                        let e = totals.entry(k).or_insert([0u32; 5]);
                        for i in 0..5 {
                            e[i] = e[i].saturating_add(v[i]);
                        }
                    }
                    finished = batch.finished;
                }
                Err(_) => continue,
            }
        }
        assert!(finished, "worker did not emit finished batch in time");
        // sub: ★5×2, ★3×1, ★4×1 (deep 子孫を sub にロールアップ)
        assert_eq!(
            totals.get("c:/root/sub"),
            Some(&[0, 0, 1, 1, 2])
        );
        // other: ★2×1
        assert_eq!(totals.get("c:/root/other"), Some(&[0, 1, 0, 0, 0]));
        // book.zip: ★5×2
        assert_eq!(
            totals.get("c:/root/book.zip"),
            Some(&[0, 0, 0, 0, 2])
        );
        // doc.pdf: ★4×1
        assert_eq!(
            totals.get("c:/root/doc.pdf"),
            Some(&[0, 0, 0, 1, 0])
        );
        // direct.jpg は直下ファイルなのでバッジ対象外 (集計キー None)
        assert!(!totals.contains_key("c:/root/direct.jpg"));
        // otherroot 配下は LIKE prefix で弾かれる
        assert!(!totals.keys().any(|k| k.starts_with("c:/otherroot")));
        // ファイルを writer.drop だけでは残るので明示削除
        drop(h);
        // 少し待って sqlite が close するのを待ってから削除
        std::thread::sleep(Duration::from_millis(50));
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(tmp.with_extension("db-wal"));
        let _ = std::fs::remove_file(tmp.with_extension("db-shm"));
        // clippy lint を黙らせるため一度 writer handle を使う
        let _ = std::io::stdout().write_all(b"");
    }
}
