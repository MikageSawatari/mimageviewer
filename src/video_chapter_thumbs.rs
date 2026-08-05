//! 動画チャプター用ジャンプサムネイルの永続キャッシュ。
//!
//! ブックマークは `video_bookmarks.thumb_webp` に保存できるが、埋め込みチャプターは
//! 動画ファイル側のメタデータなので mIV 側に保存先がない。そこで
//! `%APPDATA%/mimageviewer/video_chapter_thumbs.db` に、動画ファイルの path + size +
//! mtime + chapter start 秒をキーにして WebP を保存する。動画が更新された場合は
//! size/mtime が変わるため古いサムネは自然に参照されない。

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct VideoChapterThumb {
    pub start_secs: f64,
    pub thumb_webp: Vec<u8>,
}

pub struct VideoChapterThumbDb {
    conn: rusqlite::Connection,
}

impl VideoChapterThumbDb {
    pub fn open() -> Result<Self, rusqlite::Error> {
        let path = Self::db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(&path)?;
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    /// 一覧生成 worker 用。起動時に初期化済みの DB を書き換えずに開く。
    pub fn open_readonly() -> Result<Self, rusqlite::Error> {
        let conn = rusqlite::Connection::open_with_flags(
            Self::db_path(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_millis(750))?;
        Ok(Self { conn })
    }

    fn init_schema(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS video_chapter_thumbs (
                path TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                mtime_ms INTEGER NOT NULL,
                chapter_start_us INTEGER NOT NULL,
                start_secs REAL NOT NULL,
                thumb_webp BLOB NOT NULL,
                created_at INTEGER NOT NULL,
                PRIMARY KEY(path, file_size, mtime_ms, chapter_start_us)
             );
             CREATE INDEX IF NOT EXISTS idx_video_chapter_thumbs_path
                ON video_chapter_thumbs(path);",
        )
    }

    pub fn db_path() -> PathBuf {
        crate::data_dir::get().join("video_chapter_thumbs.db")
    }

    pub fn list(&self, video_path: &Path) -> Vec<VideoChapterThumb> {
        let Some((key, file_size, mtime_ms)) = file_identity(video_path) else {
            return Vec::new();
        };
        let stmt = self.conn.prepare_cached(
            "SELECT start_secs, thumb_webp FROM video_chapter_thumbs
              WHERE path = ?1 AND file_size = ?2 AND mtime_ms = ?3
              ORDER BY chapter_start_us ASC",
        );
        let mut stmt = match stmt {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map(
            rusqlite::params![key, file_size, mtime_ms],
            |row| -> Result<VideoChapterThumb, rusqlite::Error> {
                Ok(VideoChapterThumb {
                    start_secs: row.get(0)?,
                    thumb_webp: row.get::<_, Vec<u8>>(1)?,
                })
            },
        );
        match rows {
            Ok(it) => it.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn set(
        &self,
        video_path: &Path,
        start_secs: f64,
        thumb_webp: &[u8],
    ) -> Result<(), rusqlite::Error> {
        if !start_secs.is_finite() || start_secs < 0.0 || thumb_webp.is_empty() {
            return Ok(());
        }
        let Some((key, file_size, mtime_ms)) = file_identity(video_path) else {
            return Ok(());
        };
        let start_us = chapter_start_key(start_secs);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn.execute(
            "INSERT INTO video_chapter_thumbs
                (path, file_size, mtime_ms, chapter_start_us, start_secs, thumb_webp, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(path, file_size, mtime_ms, chapter_start_us) DO UPDATE SET
                start_secs = excluded.start_secs,
                thumb_webp = excluded.thumb_webp,
                created_at = excluded.created_at",
            rusqlite::params![
                key, file_size, mtime_ms, start_us, start_secs, thumb_webp, now
            ],
        )?;
        Ok(())
    }
}

pub fn chapter_start_key(start_secs: f64) -> i64 {
    (start_secs.max(0.0) * 1_000_000.0).round() as i64
}

fn file_identity(video_path: &Path) -> Option<(String, i64, i64)> {
    let meta = std::fs::metadata(video_path).ok()?;
    let file_size = i64::try_from(meta.len()).ok()?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    Some((
        crate::path_key::normalize_keep_drive(video_path),
        file_size,
        mtime_ms,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open_in_memory() -> VideoChapterThumbDb {
        let conn = Connection::open_in_memory().expect("memory db");
        VideoChapterThumbDb::init_schema(&conn).expect("schema");
        VideoChapterThumbDb { conn }
    }

    #[test]
    fn chapter_start_key_uses_microseconds() {
        assert_eq!(chapter_start_key(1.234_567_4), 1_234_567);
        assert_eq!(chapter_start_key(1.234_567_6), 1_234_568);
        assert_eq!(chapter_start_key(-1.0), 0);
    }

    #[test]
    fn set_then_list_uses_file_identity() {
        let db = open_in_memory();
        let path = std::env::temp_dir().join(format!(
            "mimageviewer_chapter_thumb_test_{}_{}.mp4",
            std::process::id(),
            chapter_start_key(12.345)
        ));
        std::fs::write(&path, b"video").expect("write temp video");

        db.set(&path, 12.345, &[1, 2, 3]).unwrap();
        let got = db.list(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(got.len(), 1);
        assert!((got[0].start_secs - 12.345).abs() < 1e-9);
        assert_eq!(got[0].thumb_webp, vec![1, 2, 3]);
    }
}
