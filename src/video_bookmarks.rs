//! 動画ブックマーク (= 任意位置の付箋) の永続管理 (Phase 5.4)。
//!
//! `%APPDATA%/mimageviewer/video_bookmarks.db` に「ユーザーが 🔖 で付けた任意位置」を
//! 任意個数記録する。Phase 5.4 のフルスクリーン左パネルで「ピン (1 個) / ブックマーク
//! (任意) / チャプター」を縦に並べて jump サムネとして使うため。
//!
//! # スキーマ
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS video_bookmarks (
//!     id          INTEGER PRIMARY KEY AUTOINCREMENT,
//!     path        TEXT NOT NULL,
//!     pts_secs    REAL NOT NULL,
//!     title       TEXT,                 -- 任意のラベル (空文字なら NULL)
//!     thumb_webp  BLOB,                 -- 抽出済フレーム (WebP、表示用)
//!     created_at  INTEGER NOT NULL      -- UNIX 時刻 (秒)
//! );
//! CREATE INDEX IF NOT EXISTS idx_video_bookmarks_path ON video_bookmarks(path);
//! ```
//!
//! 動画パスは複数行ヒットするので PRIMARY KEY ではなく `(path, pts_secs)` の暗黙複合
//! ではなく `id` を使う。同じ動画の同じ位置にブックマークを 2 つ作るのは想定外だが、
//! `id` で一意化しておけば削除キーが明確になる (後で UI が個別行 × ボタンで使う)。
//!
//! # API
//!
//! - `list(path)`: その動画の全ブックマークを `pts_secs` 昇順で返す。
//! - `add(path, pts_secs, title, thumb_webp) -> id`: 新規追加。
//! - `update_title(id, title)`: 既存ブックマークの任意ラベルを更新。
//! - `remove(id)`: 個別削除。
//! - `clear_for(path)`: 動画切替時の cleanup などに使う想定 (Phase 5.4 では未配線)。

use std::path::{Path, PathBuf};

/// ブックマーク 1 件分。
#[derive(Clone, Debug)]
pub struct VideoBookmark {
    pub id: i64,
    pub pts_secs: f64,
    pub title: Option<String>,
    /// 抽出済の WebP バイト列 (空なら未取得)。
    pub thumb_webp: Vec<u8>,
}

/// 動画・音声・本の横断一覧で使う、元パスと登録日時を含む行。
#[derive(Clone, Debug)]
pub struct GlobalVideoBookmark {
    pub id: i64,
    pub path: PathBuf,
    pub pts_secs: f64,
    pub title: Option<String>,
    pub thumb_webp: Vec<u8>,
    pub created_at_ms: i64,
}

/// 一括ブックマーク登録の結果サマリ。
#[derive(Clone, Copy, Debug, Default)]
pub struct BulkAddSummary {
    pub added: usize,
    pub skipped_duplicates: usize,
    /// 個別行で SELECT / INSERT が失敗した件数 (try-each-row 戦略)。
    /// 全体 commit が失敗した場合は `Err` で返り、ここには現れない。
    pub errors: usize,
}

/// シークバーマーカー / ジャンプパネル用の軽量メタデータ。
#[derive(Clone, Debug)]
pub struct VideoBookmarkMeta {
    pub id: i64,
    pub pts_secs: f64,
    pub title: Option<String>,
}

impl From<&VideoBookmark> for VideoBookmarkMeta {
    fn from(value: &VideoBookmark) -> Self {
        Self {
            id: value.id,
            pts_secs: value.pts_secs,
            title: value.title.clone(),
        }
    }
}

/// ブックマーク列から、ループ境界として使う「区間の開始秒」を **正規化済み Vec** で返す。
/// finite + nonneg + sort + dedup (1us 単位)。`start_at` / `first_boundary_after` は
/// この正規化を前提に動く。
pub fn boundary_starts_from_bookmarks(bookmarks: &[VideoBookmarkMeta]) -> Vec<f64> {
    let mut v: Vec<f64> = bookmarks
        .iter()
        .map(|b| b.pts_secs)
        .filter(|s| s.is_finite() && *s >= 0.0)
        .collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v.dedup_by(|a, b| (*a - *b).abs() < 1.0e-6);
    v
}

/// 動画ブックマーク DB ハンドル。
pub struct VideoBookmarkDb {
    conn: rusqlite::Connection,
}

impl VideoBookmarkDb {
    /// DB を開く (なければ作成 + INDEX 付与)。
    pub fn open() -> Result<Self, rusqlite::Error> {
        let path = Self::db_path();
        Self::open_at(&path)
    }

    /// 横断ビューの読み出し専用 worker 用。スキーマ作成を行わない。
    pub fn open_readonly() -> Result<Self, rusqlite::Error> {
        let conn = rusqlite::Connection::open_with_flags(
            Self::db_path(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "query_only", true)?;
        Ok(Self { conn })
    }

    /// 指定パスの DB を開く。明示メタ情報転送 worker / test 用。
    pub fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    fn init_schema(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS video_bookmarks (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                path        TEXT NOT NULL,
                pts_secs    REAL NOT NULL,
                title       TEXT,
                thumb_webp  BLOB,
                created_at  INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_video_bookmarks_path
                ON video_bookmarks(path);",
        )
    }

    pub fn db_path() -> PathBuf {
        crate::data_dir::get().join("video_bookmarks.db")
    }

    /// 横断一覧用に全ブックマークを登録日時の新しい順で返す。
    /// 呼び出し側は必ず worker スレッドから利用する。
    pub fn list_all_global(&self) -> Result<Vec<GlobalVideoBookmark>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, pts_secs, title, thumb_webp, created_at
               FROM video_bookmarks
              ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let created_at: i64 = row.get(5)?;
            Ok(GlobalVideoBookmark {
                id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                pts_secs: row.get(2)?,
                title: row
                    .get::<_, Option<String>>(3)?
                    .filter(|value| !value.is_empty()),
                thumb_webp: row.get::<_, Option<Vec<u8>>>(4)?.unwrap_or_default(),
                created_at_ms: created_at.saturating_mul(1000),
            })
        })?;
        rows.collect()
    }

    /// 状態フィルタ用に、1 件以上ブックマークを持つ media path だけを返す。
    /// thumbnail BLOB や時刻行は読み込まない。呼び出し側は worker スレッドに限定する。
    pub fn list_all_path_keys(&self) -> Result<std::collections::HashSet<String>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT path FROM video_bookmarks")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect()
    }

    /// `list` の軽量版: thumbnail BLOB を読まずに `(pts_secs, title)` だけ返す。
    /// シークバーマーカー描画や J/K ジャンプのように毎フレーム呼ばれる経路で使う
    /// (4K WebP サムネを 60fps でクローンするのを避ける)。
    pub fn list_marker_meta(&self, video_path: &Path) -> Vec<(f64, Option<String>)> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let stmt = self.conn.prepare_cached(
            "SELECT pts_secs, title FROM video_bookmarks
              WHERE path = ?1 ORDER BY pts_secs ASC",
        );
        let mut stmt = match stmt {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([&key], |row| {
            let pts_secs: f64 = row.get(0)?;
            let title: Option<String> = row.get(1)?;
            Ok((pts_secs, title.filter(|s| !s.is_empty())))
        });
        match rows {
            Ok(it) => it.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// `list` の軽量版: thumbnail BLOB を読まずに `id`, `pts_secs`, `title` だけ返す。
    pub fn list_marker_entries(&self, video_path: &Path) -> Vec<VideoBookmarkMeta> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let stmt = self.conn.prepare_cached(
            "SELECT id, pts_secs, title FROM video_bookmarks
              WHERE path = ?1 ORDER BY pts_secs ASC",
        );
        let mut stmt = match stmt {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([&key], |row| {
            let id: i64 = row.get(0)?;
            let pts_secs: f64 = row.get(1)?;
            let title: Option<String> = row.get(2)?;
            Ok(VideoBookmarkMeta {
                id,
                pts_secs,
                title: title.filter(|s| !s.is_empty()),
            })
        });
        match rows {
            Ok(it) => it.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// 指定動画の全ブックマークを `pts_secs` 昇順で返す。
    pub fn list(&self, video_path: &Path) -> Vec<VideoBookmark> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let stmt = self.conn.prepare_cached(
            "SELECT id, pts_secs, title, thumb_webp FROM video_bookmarks
              WHERE path = ?1 ORDER BY pts_secs ASC",
        );
        let mut stmt = match stmt {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([&key], |row| {
            let id: i64 = row.get(0)?;
            let pts_secs: f64 = row.get(1)?;
            let title: Option<String> = row.get(2)?;
            let thumb_webp: Vec<u8> = row.get::<_, Option<Vec<u8>>>(3)?.unwrap_or_default();
            Ok(VideoBookmark {
                id,
                pts_secs,
                title: title.filter(|s| !s.is_empty()),
                thumb_webp,
            })
        });
        match rows {
            Ok(it) => it.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// 新規ブックマーク追加。返す id は `remove` のキーに使う。
    /// `title` が空文字 / None なら NULL で保存。`thumb_webp` も同様。
    pub fn add(
        &self,
        video_path: &Path,
        pts_secs: f64,
        title: Option<&str>,
        thumb_webp: &[u8],
    ) -> Result<i64, rusqlite::Error> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let title_arg = normalize_bookmark_title(title);
        let blob: Option<&[u8]> = if thumb_webp.is_empty() {
            None
        } else {
            Some(thumb_webp)
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn.execute(
            "INSERT INTO video_bookmarks
                (path, pts_secs, title, thumb_webp, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![key, pts_secs, title_arg.as_deref(), blob, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 一括登録用: 既に同じ動画で `pts_secs` に近いブックマークがあれば skip。
    /// `dedup_tolerance_secs` 以内のものは「重複」とみなす (典型値 1.0)。
    /// 戻り値: 追加した場合は `Ok(Some(id))`、skip した場合は `Ok(None)`。
    pub fn add_if_no_duplicate(
        &self,
        video_path: &Path,
        pts_secs: f64,
        title: Option<&str>,
        dedup_tolerance_secs: f64,
    ) -> Result<Option<i64>, rusqlite::Error> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let lo = pts_secs - dedup_tolerance_secs;
        let hi = pts_secs + dedup_tolerance_secs;
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM video_bookmarks
                  WHERE path = ?1 AND pts_secs >= ?2 AND pts_secs <= ?3
                  LIMIT 1
             )",
            rusqlite::params![key, lo, hi],
            |row| row.get::<_, bool>(0),
        )?;
        if exists {
            return Ok(None);
        }
        let id = self.add(video_path, pts_secs, title, &[])?;
        Ok(Some(id))
    }

    /// 一括登録: 単一トランザクションで複数件まとめて挿入する。
    /// `add_if_no_duplicate` を行数分ループする版に比べて autocommit のオーバーヘッドが
    /// 消えるため、数百件オーダーの貼り付けでも UI スレッドを長時間ブロックしにくい。
    ///
    /// - 各 entry を `dedup_tolerance_secs` (典型値 1.0) で重複判定 → 重複なら skip。
    /// - 同一バッチ内の重複も skip する (前の INSERT で追加された行が見える)。
    /// - **個別行の SELECT / INSERT が失敗しても他の行は進める** (try-each-row 戦略)。
    ///   失敗した行は `summary.errors` で件数を返し、ログを残す。
    /// - トランザクション全体の `prepare` / `commit` が失敗した場合のみ `Err` を返す。
    pub fn bulk_add_if_no_duplicate(
        &mut self,
        video_path: &Path,
        entries: &[(f64, Option<&str>)],
        dedup_tolerance_secs: f64,
    ) -> Result<BulkAddSummary, rusqlite::Error> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let tx = self.conn.transaction()?;
        let mut added = 0usize;
        let mut skipped = 0usize;
        let mut errors = 0usize;
        {
            let mut check_stmt = tx.prepare(
                "SELECT EXISTS(
                     SELECT 1 FROM video_bookmarks
                      WHERE path = ?1 AND pts_secs >= ?2 AND pts_secs <= ?3
                      LIMIT 1
                 )",
            )?;
            let mut insert_stmt = tx.prepare(
                "INSERT INTO video_bookmarks
                    (path, pts_secs, title, thumb_webp, created_at)
                 VALUES (?1, ?2, ?3, NULL, ?4)",
            )?;
            for (pts_secs, title) in entries {
                let lo = *pts_secs - dedup_tolerance_secs;
                let hi = *pts_secs + dedup_tolerance_secs;
                // try-each-row: 1 行ごとに Err を吸収し、ループを継続する。
                let exists_result: Result<bool, rusqlite::Error> = check_stmt
                    .query_row(rusqlite::params![&key, lo, hi], |row| row.get::<_, bool>(0));
                let exists = match exists_result {
                    Ok(b) => b,
                    Err(e) => {
                        crate::logger::log(format!(
                            "video bookmark bulk: dedup SELECT failed for pts={pts_secs:.2}: {e}"
                        ));
                        errors += 1;
                        continue;
                    }
                };
                if exists {
                    skipped += 1;
                    continue;
                }
                let title_arg = normalize_bookmark_title(*title);
                match insert_stmt.execute(rusqlite::params![
                    &key,
                    *pts_secs,
                    title_arg.as_deref(),
                    now,
                ]) {
                    Ok(_) => added += 1,
                    Err(e) => {
                        crate::logger::log(format!(
                            "video bookmark bulk: INSERT failed for pts={pts_secs:.2}: {e}"
                        ));
                        errors += 1;
                    }
                }
            }
        }
        tx.commit()?;
        Ok(BulkAddSummary {
            added,
            skipped_duplicates: skipped,
            errors,
        })
    }

    /// 既存ブックマークの名称を更新する。空文字 / 空白のみ / None は NULL として保存。
    #[allow(dead_code)]
    pub fn update_title(&self, id: i64, title: Option<&str>) -> Result<(), rusqlite::Error> {
        let title_arg = normalize_bookmark_title(title);
        self.conn.execute(
            "UPDATE video_bookmarks SET title = ?1 WHERE id = ?2",
            rusqlite::params![title_arg.as_deref(), id],
        )?;
        Ok(())
    }

    /// 既存ブックマークのジャンプパネル用サムネイルを更新する。
    pub fn update_thumb(&self, id: i64, thumb_webp: &[u8]) -> Result<(), rusqlite::Error> {
        if thumb_webp.is_empty() {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE video_bookmarks SET thumb_webp = ?1 WHERE id = ?2",
            rusqlite::params![thumb_webp, id],
        )?;
        Ok(())
    }

    /// 個別削除 (id = `add` の戻り値)。
    #[allow(dead_code)]
    pub fn remove(&self, id: i64) -> Result<(), rusqlite::Error> {
        self.conn
            .execute("DELETE FROM video_bookmarks WHERE id = ?1", [id])?;
        Ok(())
    }

    /// 指定動画の全ブックマークを削除 (動画ファイル削除時等の cleanup 用想定、
    /// Phase 5.4 では未配線)。
    #[allow(dead_code)]
    pub fn clear_for(&self, video_path: &Path) -> Result<(), rusqlite::Error> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        self.conn
            .execute("DELETE FROM video_bookmarks WHERE path = ?1", [&key])?;
        Ok(())
    }
}

fn normalize_bookmark_title(title: Option<&str>) -> Option<String> {
    title
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open_in_memory() -> VideoBookmarkDb {
        let conn = Connection::open_in_memory().expect("memory db");
        VideoBookmarkDb::init_schema(&conn).expect("schema");
        VideoBookmarkDb { conn }
    }

    #[test]
    fn boundary_starts_from_bookmarks_normalizes() {
        let bms = vec![
            VideoBookmarkMeta {
                id: 1,
                pts_secs: f64::NAN,
                title: None,
            },
            VideoBookmarkMeta {
                id: 2,
                pts_secs: -3.0,
                title: None,
            },
            VideoBookmarkMeta {
                id: 3,
                pts_secs: 10.0,
                title: None,
            },
            VideoBookmarkMeta {
                id: 4,
                pts_secs: 5.0,
                title: None,
            },
            VideoBookmarkMeta {
                id: 5,
                pts_secs: 5.0,
                title: None,
            }, // dup
        ];
        assert_eq!(boundary_starts_from_bookmarks(&bms), vec![5.0, 10.0]);
    }

    #[test]
    fn boundary_starts_from_bookmarks_handles_empty() {
        assert_eq!(boundary_starts_from_bookmarks(&[]), Vec::<f64>::new());
    }

    #[test]
    fn list_empty_returns_empty() {
        let db = open_in_memory();
        assert!(db.list(Path::new("C:/none.mp4")).is_empty());
    }

    #[test]
    fn add_then_list_in_order() {
        let db = open_in_memory();
        let p = Path::new("C:/Videos/M.mp4");
        let _id1 = db.add(p, 30.0, Some("end"), &[]).unwrap();
        let _id2 = db.add(p, 5.0, Some("intro"), &[1, 2]).unwrap();
        let _id3 = db.add(p, 15.0, None, &[]).unwrap();
        let list = db.list(p);
        assert_eq!(list.len(), 3);
        assert!((list[0].pts_secs - 5.0).abs() < 1e-9);
        assert_eq!(list[0].title.as_deref(), Some("intro"));
        assert_eq!(list[0].thumb_webp, vec![1, 2]);
        assert!((list[1].pts_secs - 15.0).abs() < 1e-9);
        assert!(list[1].title.is_none());
        assert!((list[2].pts_secs - 30.0).abs() < 1e-9);
    }

    #[test]
    fn remove_only_targeted_id() {
        let db = open_in_memory();
        let p = Path::new("C:/v.mp4");
        let id1 = db.add(p, 1.0, None, &[]).unwrap();
        let _id2 = db.add(p, 2.0, None, &[]).unwrap();
        db.remove(id1).unwrap();
        let list = db.list(p);
        assert_eq!(list.len(), 1);
        assert!((list[0].pts_secs - 2.0).abs() < 1e-9);
    }

    #[test]
    fn update_title_sets_and_clears_label() {
        let db = open_in_memory();
        let p = Path::new("C:/v.mp4");
        let id = db.add(p, 1.0, None, &[]).unwrap();

        db.update_title(id, Some("  見どころ  ")).unwrap();
        let list = db.list(p);
        assert_eq!(list[0].title.as_deref(), Some("見どころ"));
        let marker_meta = db.list_marker_meta(p);
        assert_eq!(marker_meta[0].1.as_deref(), Some("見どころ"));

        db.update_title(id, Some("   ")).unwrap();
        let list = db.list(p);
        assert!(list[0].title.is_none());
    }

    #[test]
    fn update_thumb_sets_existing_blob() {
        let db = open_in_memory();
        let p = Path::new("C:/v.mp4");
        let id = db.add(p, 1.0, None, &[]).unwrap();

        db.update_thumb(id, &[9, 8, 7]).unwrap();

        let list = db.list(p);
        assert_eq!(list[0].thumb_webp, vec![9, 8, 7]);
    }

    #[test]
    fn case_and_separator_normalized() {
        let db = open_in_memory();
        db.add(Path::new("C:\\A\\M.MP4"), 7.5, None, &[]).unwrap();
        let got = db.list(Path::new("c:/a/m.mp4"));
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn add_if_no_duplicate_skips_existing_within_tolerance() {
        let db = open_in_memory();
        let p = Path::new("C:/v.mp4");
        let id1 = db.add_if_no_duplicate(p, 13.0, Some("A"), 1.0).unwrap();
        assert!(id1.is_some());
        // 同じ秒なら skip
        let id2 = db.add_if_no_duplicate(p, 13.0, Some("dup"), 1.0).unwrap();
        assert!(id2.is_none());
        // ±tolerance 内なら skip
        let id3 = db.add_if_no_duplicate(p, 13.5, Some("near"), 1.0).unwrap();
        assert!(id3.is_none());
        // tolerance を超えれば追加される
        let id4 = db.add_if_no_duplicate(p, 15.0, Some("ok"), 1.0).unwrap();
        assert!(id4.is_some());
        let list = db.list(p);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].title.as_deref(), Some("A"));
        assert_eq!(list[1].title.as_deref(), Some("ok"));
    }

    #[test]
    fn bulk_add_if_no_duplicate_dedups_within_batch_and_against_existing() {
        let mut db = open_in_memory();
        let p = Path::new("C:/v.mp4");
        // 既存ブックマーク 1 件 (5.0 秒) を準備
        db.add(p, 5.0, Some("existing"), &[]).unwrap();
        // バッチ: 既存と重複 / 範囲内 / 範囲外 / バッチ内重複
        let entries = vec![
            (5.0, Some("dup-with-existing")),
            (5.4, Some("near-existing")),
            (10.0, Some("ok-1")),
            (10.5, Some("dup-in-batch")),
            (20.0, Some("ok-2")),
        ];
        let summary = db.bulk_add_if_no_duplicate(p, &entries, 1.0).unwrap();
        assert_eq!(summary.added, 2);
        assert_eq!(summary.skipped_duplicates, 3);
        // 全件確認: existing + ok-1 + ok-2 の 3 件
        let list = db.list(p);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].title.as_deref(), Some("existing"));
        assert_eq!(list[1].title.as_deref(), Some("ok-1"));
        assert_eq!(list[2].title.as_deref(), Some("ok-2"));
    }

    #[test]
    fn bulk_add_if_no_duplicate_empty_batch_is_noop() {
        let mut db = open_in_memory();
        let summary = db
            .bulk_add_if_no_duplicate(Path::new("C:/v.mp4"), &[], 1.0)
            .unwrap();
        assert_eq!(summary.added, 0);
        assert_eq!(summary.skipped_duplicates, 0);
        assert_eq!(summary.errors, 0);
    }

    #[test]
    fn bulk_add_if_no_duplicate_continues_after_in_loop_dup() {
        // try-each-row 戦略の動作確認: 重複と通常を交互に与えて、後続が
        // commit されることを確認する (= 1 件の skip でロールバックしない)。
        let mut db = open_in_memory();
        let p = Path::new("C:/v.mp4");
        db.add(p, 5.0, None, &[]).unwrap();
        let entries = vec![
            (5.0, Some("dup")),  // skipped
            (10.0, Some("ok1")), // added
            (5.5, Some("dup")),  // skipped (within tolerance)
            (20.0, Some("ok2")), // added
        ];
        let summary = db.bulk_add_if_no_duplicate(p, &entries, 1.0).unwrap();
        assert_eq!(summary.added, 2);
        assert_eq!(summary.skipped_duplicates, 2);
        assert_eq!(summary.errors, 0);
        let list = db.list(p);
        // existing (5.0) + ok1 (10.0) + ok2 (20.0)
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn add_if_no_duplicate_isolates_by_video() {
        let db = open_in_memory();
        db.add_if_no_duplicate(Path::new("C:/a.mp4"), 10.0, None, 1.0)
            .unwrap();
        // 別動画の同じ pts は追加される
        let added = db
            .add_if_no_duplicate(Path::new("C:/b.mp4"), 10.0, None, 1.0)
            .unwrap();
        assert!(added.is_some());
    }

    #[test]
    fn clear_for_removes_only_one_video() {
        let db = open_in_memory();
        db.add(Path::new("C:/a.mp4"), 1.0, None, &[]).unwrap();
        db.add(Path::new("C:/a.mp4"), 2.0, None, &[]).unwrap();
        db.add(Path::new("C:/b.mp4"), 1.0, None, &[]).unwrap();
        db.clear_for(Path::new("C:/a.mp4")).unwrap();
        assert!(db.list(Path::new("C:/a.mp4")).is_empty());
        assert_eq!(db.list(Path::new("C:/b.mp4")).len(), 1);
    }
}
