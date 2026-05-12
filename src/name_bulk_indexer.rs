//! 名前索引 (Ctrl+S 検索用 `search_index_db`) のバックグラウンドバルクスキャナ。
//!
//! お気に入りで「名前」フル索引化フラグ (`auto_index_structure=true`) を ON にしたとき、
//! その favorite 配下を一度だけ再帰的に走査してフォルダ / ZIP / PDF / 動画名を一括登録する。
//!
//! - 閲覧時自動追記の経路は廃止された (`src/app.rs::load_folder` の "訪問時自動索引化は廃止"
//!   コメント参照)。現在は `NameIndexSupervisor` の起動時バルク + notify-rs 監視で
//!   全エントリを投入する。本 module はそのバルク部分の実装。
//! - メタ索引の supervisor とは独立に動く (別 DB、別スレッド、キャンセル独立)。
//! - Tantivy writer 制約は無いので複数 favorite を並列に走らせても問題ない。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::folder_tree::{
    SUPPORTED_VIDEO_EXTENSIONS, is_apple_double, walk_dirs_recursive_with_progress,
};
use crate::indexer_progress::ProgressReporter;
use crate::search_index_db::{IndexEntry, IndexKind, SearchIndexDb};

/// `walk_dirs_recursive_with_progress` の `on_error` と Pass 2 の `read_dir` 失敗ブランチ
/// から共有する rate-limited logger。
///
/// 同一 path について 30 秒以内の再 log を抑制 (= 一過性のロック / 権限不足で毎フレーム
/// 同じ行が走るケースで `mimageviewer.log` を膨張させない)。`HashMap` をローカルに 1 つ
/// 持つことで、Pass 1 の再帰呼び出し越し / Pass 2 のループ越しで抑制が継続する
/// (Codex P2 第 2 レビュー指摘: `folder_tree.rs` の汎用 DFS に直接 logging を入れずに、
/// 呼び出し側 (本 module) でまとめて持つ設計)。
struct ReadDirLogger {
    last_logged: HashMap<PathBuf, Instant>,
}

impl ReadDirLogger {
    fn new() -> Self {
        Self {
            last_logged: HashMap::new(),
        }
    }

    fn log(&mut self, p: &Path, e: &std::io::Error) {
        const COOLDOWN: Duration = Duration::from_secs(30);
        let now = Instant::now();
        let key = p.to_path_buf();
        let should_log = self
            .last_logged
            .get(&key)
            .is_none_or(|t| now.duration_since(*t) >= COOLDOWN);
        if should_log {
            crate::logger::log(format!(
                "name_bulk_indexer: read_dir failed for {}: {e}",
                p.display()
            ));
            self.last_logged.insert(key, now);
        }
    }
}

/// バルクスキャン 1 回分の進捗と結果サマリ (将来 UI 表示するなら拡張)。
#[derive(Debug, Default, Clone, Copy)]
pub struct BulkSummary {
    pub folders_visited: usize,
    pub entries_written: usize,
    pub cancelled: bool,
    /// Pass 1 / Pass 2 で `read_dir` / `file_type` / `upsert_children` のいずれかが失敗
    /// したか。`true` の場合は post-scan prune を skip する (不完全観測で正当な行を
    /// 消さないため — Codex P2 第 11 レビュー指摘)。
    pub had_error: bool,
}

/// favorite 1 つ分のバルクスキャンを同期実行する。通常は `std::thread::spawn` で包む。
///
/// - `fav_path`: お気に入りルート (絶対パス)
/// - `db`: 書き込み先 SQLite
/// - `cancel`: true になったら速やかに中断
///
/// 既存エントリとの衝突は `upsert_children` が同フォルダ配下を入れ替える挙動なので
/// 冪等に動作する (途中キャンセル後に再実行しても破綻しない)。
///
/// `activity_gate` を渡すと、各フォルダの処理前に UI 操作が静穏になるまで待機する
/// (2026-04 F: 操作中は bulk スキャンを一時停止)。
pub fn run_bulk_name_index(
    fav_path: &std::path::Path,
    db: &SearchIndexDb,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
    cancel: &AtomicBool,
    progress: Option<&ProgressReporter>,
) -> BulkSummary {
    let mut summary = BulkSummary::default();

    if let Some(p) = progress {
        p.set(format!("スキャン開始: {}", fav_path.display()));
    }

    // post-scan prune の cutoff。`next_write_stamp` はプロセス全域で厳密単調増加 (AtomicI64)
    // なので、直後に走る upsert の stamp は常に `> scan_start_stamp`。
    // `<` 比較で「未観測 stale 行 / 今回 scan が触った新しい行」を race-free に分離できる。
    // (旧実装は秒精度で、同秒に連続スキャンが入ると stale 行が残留するバグがあった)
    let scan_start_stamp = crate::search_index_db::next_write_stamp();

    // Pass 1: サブフォルダ列挙。`on_visit` は各フォルダの `read_dir` 前に呼ばれるので、
    // ここで ActivityGate を待てば再帰中でもユーザー操作中はフォルダ単位で列挙が停止する。
    // (cancel 伝播は `walk_dirs_recursive_with_progress` 側の責務なので wait のみ)
    // `on_error` は read_dir 失敗を rate-limit して log + had_error フラグを立てる
    // (Codex P2 第 11 レビュー指摘: 不完全観測時は post-scan prune を skip)。
    let mut found: Vec<PathBuf> = Vec::new();
    let mut read_dir_logger = ReadDirLogger::new();
    let mut had_error = false;
    walk_dirs_recursive_with_progress(
        fav_path,
        &mut found,
        cancel,
        &mut |cur| {
            if let Some(gate) = activity_gate {
                gate.wait_until_idle(cancel);
            }
            if let Some(p) = progress {
                let display = cur.strip_prefix(fav_path).unwrap_or(cur).display();
                p.set(format!("フォルダ列挙 {}", display));
            }
        },
        &mut |p, e| {
            had_error = true;
            read_dir_logger.log(p, e);
        },
    );
    if cancel.load(Ordering::Relaxed) {
        summary.cancelled = true;
        return summary;
    }
    summary.folders_visited = found.len();
    let total_folders = found.len();

    // Pass 2: 各フォルダ直下の Folder / ZipFile / PdfFile / VideoFile を集めて upsert
    for (i, folder) in found.iter().enumerate() {
        // フォルダ 1 つ分を処理してから次でまた判定 (gate + cancel 両対応)。
        if crate::activity_gate::wait_and_check_cancel(activity_gate, cancel) {
            summary.cancelled = true;
            break;
        }
        if let Some(p) = progress {
            // カウントを先頭に / フォルダは favorite 相対でフルパスが切れにくいようにする。
            let display = folder.strip_prefix(fav_path).unwrap_or(folder).display();
            p.set_msg_and_count(
                format!("取込 ({}/{}) {}", i + 1, total_folders, display),
                (i + 1) as u64,
                total_folders as u64,
            );
        }
        let entries = match std::fs::read_dir(folder) {
            Ok(e) => e,
            Err(e) => {
                had_error = true;
                read_dir_logger.log(folder, &e);
                continue;
            }
        };
        // `collect_index_entries` が per-entry エラー (DirEntry::Err / file_type 失敗) を
        // 検知して had_entry_error=true で返す (Codex P2 第 11 レビュー指摘)。
        let (children, had_entry_error) = collect_index_entries(entries, "name_bulk_indexer");
        if had_entry_error {
            // **upsert を skip する** (Codex P2 第 12 レビュー指摘):
            // `upsert_children` は親直下を DELETE → INSERT で authoritative replace するので、
            // 観測できなかった legit 子エントリがこの瞬間に消えてしまう。post-scan prune を
            // skip しても、直接子の削除はもう発生している。
            // 不完全観測のフォルダ全体を skip して既存行を保護する (次回 watcher event /
            // 次回起動 walker の 3-way diff で補修される設計に揃える)。
            had_error = true;
            crate::logger::log(format!(
                "name_bulk_indexer: skipping upsert_children for {} (per-entry error: incomplete observation)",
                folder.display()
            ));
            continue;
        }
        // **Codex P2 #1 回帰修正 (2026-04)**: `children.is_empty()` でも continue せず、
        // `upsert_children` を呼び出す。旧実装はここで skip していたため、アプリ停止中に
        // 子フォルダ/ZIP/PDF/動画がすべて削除されて「空になった親フォルダ」では、
        // upsert_children の DELETE が走らず古い行が残り続けるバグがあった。
        // (tests/search_name_e2e.rs::full_scan_removes_stale_entries_from_became_empty_folder)
        // upsert_children は DELETE → INSERT の順なので、children が空のときは
        // DELETE だけが走って「この親配下の子エントリを全消去」する正しい挙動になる。
        //
        // **Codex P2 race 対策**: upsert_children 直前にも cancel を確認する。
        // これで「UI が OFF に切り替えた → clear_for_favorite が走る → 直前の
        // in-flight upsert が race で書き戻す」窓を最小化する。
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        summary.entries_written += children.len();
        if let Err(e) = db.upsert_children(fav_path, folder, &children) {
            had_error = true;
            crate::logger::log(format!(
                "name_bulk_indexer: upsert_children failed for {}: {e}",
                folder.display()
            ));
        }
    }

    summary.had_error = had_error;

    if let Some(p) = progress {
        // バルク取込完了 — ETA カウントをクリアして UI から残り時間を消す。
        p.clear_count();
    }

    // stale 行を一掃 (cancel / per-entry エラー / read_dir 失敗 / upsert 失敗のいずれかで
    // 不完全観測の場合は skip — 観測できなかった正当な行を消さないため)。
    if !summary.cancelled && !summary.had_error {
        match db.prune_stale_for_favorite(fav_path, scan_start_stamp) {
            Ok(0) => {}
            Ok(n) => crate::logger::log(format!(
                "name_bulk_indexer: pruned {n} stale rows under {}",
                fav_path.display()
            )),
            Err(e) => crate::logger::log(format!(
                "name_bulk_indexer: prune_stale_for_favorite failed for {}: {e}",
                fav_path.display()
            )),
        }
    } else if summary.had_error {
        crate::logger::log(format!(
            "name_bulk_indexer: skipping prune_stale_for_favorite for {} (incomplete scan: cancelled={}, had_error={})",
            fav_path.display(),
            summary.cancelled,
            summary.had_error
        ));
    }

    summary
}

/// `read_dir` の `ReadDir` イテレータから索引対象の `IndexEntry` 群を集める共通ヘルパ。
///
/// 戻り値の `bool` は「per-entry エラー (DirEntry::Err / file_type() 失敗) が 1 件以上
/// あったか」を示す。`true` の場合、呼び出し側は「不完全観測」として扱い、後続の
/// `prune_*` 系を **skip すべき** (Codex P2 第 11 レビュー指摘: `entries.flatten()` や
/// `let Ok(ft) = ... else { continue };` で silent skip すると、観測できなかった行が
/// stale 扱いで消える事故が起きる)。
///
/// `log_prefix` は per-entry エラー時のログ前缀 (例: `"name_bulk_indexer"` /
/// `"name_index Pass 2"`)。
///
/// なお `upsert_children` の DELETE は this 関数が返した `children` に含まれない
/// 直下行を消す best-effort 動作なので、`had_entry_error == true` のときに上層で
/// upsert を呼ぶか否かは呼び出し側の判断 (現状は best-effort で呼ぶが、post-scan
/// prune は必ず skip する)。
pub fn collect_index_entries(
    entries: std::fs::ReadDir,
    log_prefix: &str,
) -> (Vec<IndexEntry>, bool) {
    let mut children: Vec<IndexEntry> = Vec::new();
    let mut had_entry_error = false;
    for entry_result in entries {
        let entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                had_entry_error = true;
                crate::logger::log(format!("{log_prefix}: dir entry read failed: {e}"));
                continue;
            }
        };
        let p = entry.path();
        if is_apple_double(&p) {
            continue;
        }
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) => {
                had_entry_error = true;
                crate::logger::log(format!(
                    "{log_prefix}: file_type failed for {}: {e}",
                    p.display()
                ));
                continue;
            }
        };
        let Some(kind) = classify_name_index_kind(&p, &ft) else {
            continue;
        };
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        children.push(IndexEntry {
            path: p,
            display_name: name,
            kind,
            mtime: 0,
        });
    }
    (children, had_entry_error)
}

/// DirEntry を名前索引の `IndexKind` に分類する共通ヘルパ。
/// index_creator / name_bulk_indexer から共有する (UI-responsiveness の観点で
/// `entry.file_type()` 経由で判定する経路に寄せる)。
pub fn classify_name_index_kind(
    path: &std::path::Path,
    file_type: &std::fs::FileType,
) -> Option<IndexKind> {
    if file_type.is_dir() {
        return Some(IndexKind::Folder);
    }
    if !file_type.is_file() {
        return None;
    }
    let ext = path.extension().and_then(|e| e.to_str())?;
    if ext.eq_ignore_ascii_case("zip") {
        Some(IndexKind::ZipFile)
    } else if ext.eq_ignore_ascii_case("pdf") {
        Some(IndexKind::PdfFile)
    } else if SUPPORTED_VIDEO_EXTENSIONS
        .iter()
        .any(|video_ext| ext.eq_ignore_ascii_case(video_ext))
    {
        Some(IndexKind::VideoFile)
    } else {
        None
    }
}

/// `std::thread::spawn` ラッパ。呼び出し側は `Arc<SearchIndexDb>` と `Arc<AtomicBool>` を
/// 渡して、スレッドハンドルを保持せずに投げ捨てる想定 (長期間走らないバルクなので)。
pub fn spawn_bulk(
    fav_path: PathBuf,
    db: Arc<SearchIndexDb>,
    cancel: Arc<AtomicBool>,
    progress: Option<ProgressReporter>,
) -> std::thread::JoinHandle<BulkSummary> {
    std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        let summary = run_bulk_name_index(&fav_path, &db, None, &cancel, progress.as_ref());
        crate::logger::log(format!(
            "name_bulk_indexer: {} done in {} ms (folders={}, entries={}, cancelled={})",
            fav_path.display(),
            t0.elapsed().as_millis(),
            summary.folders_visited,
            summary.entries_written,
            summary.cancelled,
        ));
        if let Some(p) = progress {
            p.clear();
        }
        summary
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn mkdir(p: &std::path::Path) {
        std::fs::create_dir_all(p).unwrap();
    }
    fn touch(p: &std::path::Path) {
        std::fs::write(p, b"").unwrap();
    }

    #[test]
    fn bulk_collects_folders_zips_pdfs_and_ignores_other_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        let sub = root.join("sub");
        mkdir(&sub);
        touch(&root.join("a.zip"));
        touch(&root.join("b.pdf"));
        touch(&root.join("c.jpg")); // 画像は名前索引対象外
        touch(&sub.join("d.zip"));

        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        let summary = run_bulk_name_index(&root, &db, None, &cancel, None);

        // folders_visited = root + sub
        assert_eq!(summary.folders_visited, 2);
        // entries_written: root 直下 = sub(Folder) + a.zip + b.pdf = 3,
        //                  sub 直下  = d.zip = 1
        assert_eq!(summary.entries_written, 4);
        assert!(!summary.cancelled);

        // DB に登録されたエントリを count で確認 (root に 4 件入るはず)
        let count = db.count_for_favorite(&root).unwrap();
        assert_eq!(count, 4);
    }

    /// B2 (Codex P2 レビュー反映): depth 3 のサブツリーが正しく再帰投入されることを
    /// supervisor を経由せずに bulk 本体だけで固定する。
    /// 既存 `bulk_collects_folders_zips_pdfs_and_ignores_other_files` は depth 2 までしか
    /// カバーしていなかった。
    #[test]
    fn bulk_indexes_zip_pdf_video_at_depth_three() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        // root/a/b/c/leaf.zip と leaf.pdf と leaf.mp4
        let c = root.join("a").join("b").join("c");
        mkdir(&c);
        touch(&c.join("leaf.zip"));
        touch(&c.join("leaf.pdf"));
        touch(&c.join("leaf.mp4"));

        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        let summary = run_bulk_name_index(&root, &db, None, &cancel, None);

        // folders_visited = root + a + b + c = 4
        assert_eq!(summary.folders_visited, 4);
        // entries_written: root直下=a(Folder)=1, a直下=b(Folder)=1, b直下=c(Folder)=1,
        //                  c直下=leaf.zip + leaf.pdf + leaf.mp4 = 3  → 合計 6
        assert_eq!(summary.entries_written, 6);
        assert!(!summary.cancelled);

        // 深い場所の ZIP / PDF / 動画も検索ヒット
        let leaf = db
            .search("leaf", &[root.clone()], crate::search_query::MatchMode::And)
            .unwrap();
        assert_eq!(
            leaf.len(),
            3,
            "leaf.zip + leaf.pdf + leaf.mp4 が 3 件ヒット"
        );
        let kinds: Vec<IndexKind> = leaf.iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&IndexKind::ZipFile));
        assert!(kinds.contains(&IndexKind::PdfFile));
        assert!(kinds.contains(&IndexKind::VideoFile));
    }

    #[test]
    fn bulk_respects_cancel_token() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        mkdir(&root.join("a"));
        mkdir(&root.join("b"));

        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(true); // 最初から立てておく
        let summary = run_bulk_name_index(&root, &db, None, &cancel, None);
        assert!(summary.cancelled);
    }
}
