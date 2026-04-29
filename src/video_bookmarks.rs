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

/// 動画ブックマーク DB ハンドル。
pub struct VideoBookmarkDb {
    conn: rusqlite::Connection,
}

impl VideoBookmarkDb {
    /// DB を開く (なければ作成 + INDEX 付与)。
    pub fn open() -> Result<Self, rusqlite::Error> {
        let path = Self::db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(&path)?;
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

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("video_bookmarks.db")
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
    #[allow(dead_code)]
    pub fn add(
        &self,
        video_path: &Path,
        pts_secs: f64,
        title: Option<&str>,
        thumb_webp: &[u8],
    ) -> Result<i64, rusqlite::Error> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let title_arg: Option<&str> = title.filter(|s| !s.is_empty());
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
            rusqlite::params![key, pts_secs, title_arg, blob, now],
        )?;
        Ok(self.conn.last_insert_rowid())
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
    fn case_and_separator_normalized() {
        let db = open_in_memory();
        db.add(Path::new("C:\\A\\M.MP4"), 7.5, None, &[]).unwrap();
        let got = db.list(Path::new("c:/a/m.mp4"));
        assert_eq!(got.len(), 1);
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
