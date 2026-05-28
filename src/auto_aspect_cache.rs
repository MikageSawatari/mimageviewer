//! Folder-level cache for the last confirmed automatic thumbnail aspect.
//!
//! This cache is intentionally small: it stores only the last `ThumbAspect`
//! selected by the auto-aspect statistics for a folder/container. Thumbnail
//! image data and representative-folder thumbnail targets remain owned by the
//! existing catalog / folder-thumb-pin paths.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::settings::ThumbAspect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoAspectCacheEntry {
    pub aspect: ThumbAspect,
    pub sample_count: usize,
    pub eligible_total: usize,
    pub updated_at: i64,
}

pub struct AutoAspectCacheDb {
    conn: rusqlite::Connection,
}

impl AutoAspectCacheDb {
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    pub fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS auto_aspect_cache (
                folder_key     TEXT PRIMARY KEY,
                aspect         INTEGER NOT NULL,
                sample_count   INTEGER NOT NULL DEFAULT 0,
                eligible_total INTEGER NOT NULL DEFAULT 0,
                updated_at     INTEGER NOT NULL
            )",
        )?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("auto_aspect_cache.db")
    }

    pub fn get(&self, folder: &Path) -> Option<AutoAspectCacheEntry> {
        let key = folder_key(folder);
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT aspect, sample_count, eligible_total, updated_at \
                 FROM auto_aspect_cache WHERE folder_key = ?1",
            )
            .ok()?;
        stmt.query_row([&key], |row| {
            let aspect_raw: i32 = row.get(0)?;
            let sample_count: i64 = row.get(1)?;
            let eligible_total: i64 = row.get(2)?;
            let updated_at: i64 = row.get(3)?;
            Ok((aspect_raw, sample_count, eligible_total, updated_at))
        })
        .ok()
        .and_then(|(aspect_raw, sample_count, eligible_total, updated_at)| {
            Some(AutoAspectCacheEntry {
                aspect: aspect_from_int(aspect_raw)?,
                sample_count: sample_count.max(0) as usize,
                eligible_total: eligible_total.max(0) as usize,
                updated_at,
            })
        })
    }

    pub fn upsert(
        &self,
        folder: &Path,
        aspect: ThumbAspect,
        sample_count: usize,
        eligible_total: usize,
    ) -> Result<(), rusqlite::Error> {
        let key = folder_key(folder);
        let updated_at = now_unix_secs();
        self.conn.execute(
            "INSERT INTO auto_aspect_cache \
             (folder_key, aspect, sample_count, eligible_total, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(folder_key) DO UPDATE SET \
               aspect = excluded.aspect, \
               sample_count = excluded.sample_count, \
               eligible_total = excluded.eligible_total, \
               updated_at = excluded.updated_at",
            rusqlite::params![
                key,
                aspect_to_int(aspect),
                sample_count as i64,
                eligible_total as i64,
                updated_at
            ],
        )?;
        Ok(())
    }

    pub fn clear_all(&self) -> Result<usize, rusqlite::Error> {
        self.conn.execute("DELETE FROM auto_aspect_cache", [])
    }

    pub fn count(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM auto_aspect_cache", [], |row| {
                row.get(0)
            })
            .unwrap_or(0)
    }
}

fn folder_key(path: &Path) -> String {
    crate::path_key::normalize_keep_drive(path)
}

fn aspect_to_int(aspect: ThumbAspect) -> i32 {
    match aspect {
        ThumbAspect::Landscape16x9 => 0,
        ThumbAspect::Landscape3x2 => 1,
        ThumbAspect::Landscape4x3 => 2,
        ThumbAspect::Square => 3,
        ThumbAspect::Portrait3x4 => 4,
        ThumbAspect::Portrait2x3 => 5,
        ThumbAspect::Portrait9x16 => 6,
    }
}

fn aspect_from_int(value: i32) -> Option<ThumbAspect> {
    match value {
        0 => Some(ThumbAspect::Landscape16x9),
        1 => Some(ThumbAspect::Landscape3x2),
        2 => Some(ThumbAspect::Landscape4x3),
        3 => Some(ThumbAspect::Square),
        4 => Some(ThumbAspect::Portrait3x4),
        5 => Some(ThumbAspect::Portrait2x3),
        6 => Some(ThumbAspect::Portrait9x16),
        _ => None,
    }
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_int_roundtrip() {
        for &aspect in ThumbAspect::all() {
            assert_eq!(aspect_from_int(aspect_to_int(aspect)), Some(aspect));
        }
        assert_eq!(aspect_from_int(-1), None);
        assert_eq!(aspect_from_int(99), None);
    }

    #[test]
    fn cache_roundtrip_and_clear() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("auto_aspect_cache.db");
        let db = AutoAspectCacheDb::open_at(&db_path).expect("open db");
        let folder = Path::new(r"C:\Books\Series");

        assert_eq!(db.get(folder), None);
        db.upsert(folder, ThumbAspect::Portrait2x3, 12, 40)
            .expect("insert cache");

        let entry = db.get(folder).expect("cache entry");
        assert_eq!(entry.aspect, ThumbAspect::Portrait2x3);
        assert_eq!(entry.sample_count, 12);
        assert_eq!(entry.eligible_total, 40);
        assert_eq!(db.count(), 1);

        db.upsert(folder, ThumbAspect::Square, 8, 20)
            .expect("update cache");
        let entry = db.get(folder).expect("updated cache entry");
        assert_eq!(entry.aspect, ThumbAspect::Square);
        assert_eq!(entry.sample_count, 8);
        assert_eq!(entry.eligible_total, 20);
        assert_eq!(db.clear_all().expect("clear"), 1);
        assert_eq!(db.count(), 0);
    }
}
