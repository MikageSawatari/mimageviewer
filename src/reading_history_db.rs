//! 最近読んだ本 (フォルダ / ZIP / PDF / 変換アーカイブ) の履歴 DB。
//!
//! `%APPDATA%/mimageviewer/reading_history.db` に、フルスクリーンで読んだ
//! 本コンテナを MRU として保存する。ページ送り中に UI スレッドで同期 SQLite I/O を
//! 行わないよう、書き込みは [`ReadingHistoryWriter`] の専用スレッドへ送る。

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::OptionalExtension;

use crate::archive_converter::ArchiveFormat;

/// 読書履歴の保存件数上限。
pub const READING_HISTORY_LIMIT_MAX: usize = 1000;

/// 読書履歴の保存件数デフォルト。
pub const READING_HISTORY_LIMIT_DEFAULT: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadingHistoryKind {
    Folder,
    Zip,
    Pdf,
    Archive,
}

impl ReadingHistoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Zip => "zip",
            Self::Pdf => "pdf",
            Self::Archive => "archive",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "folder" => Some(Self::Folder),
            "zip" => Some(Self::Zip),
            "pdf" => Some(Self::Pdf),
            "archive" => Some(Self::Archive),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadingHistoryEntry {
    pub key: String,
    pub path: PathBuf,
    pub kind: ReadingHistoryKind,
    pub archive_format: Option<ArchiveFormat>,
    pub title: String,
    pub last_read_at_ms: i64,
    pub last_page: Option<i64>,
    pub page_count: Option<i64>,
    pub file_size: Option<i64>,
    pub mtime_ms: Option<i64>,
}

impl ReadingHistoryEntry {
    pub fn new(
        path: PathBuf,
        kind: ReadingHistoryKind,
        archive_format: Option<ArchiveFormat>,
        title: String,
        last_page: Option<i64>,
        page_count: Option<i64>,
    ) -> Self {
        let key = normalize_path_keep_drive(&path);
        Self {
            key,
            path,
            kind,
            archive_format,
            title,
            last_read_at_ms: now_ms(),
            last_page,
            page_count,
            file_size: None,
            mtime_ms: None,
        }
    }
}

/// 読書履歴 DB ハンドル。
pub struct ReadingHistoryDb {
    conn: rusqlite::Connection,
}

impl ReadingHistoryDb {
    /// DB を開く (なければ作成)。
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(Self::db_path())
    }

    /// 集約ビューなどの読み出し専用 worker 用。DB を作成・更新しない。
    pub fn open_readonly() -> Result<Self, rusqlite::Error> {
        let conn = rusqlite::Connection::open_with_flags(
            Self::db_path(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?;
        conn.busy_timeout(Duration::from_secs(3))?;
        conn.pragma_update(None, "query_only", true)?;
        Ok(Self { conn })
    }

    fn open_at(path: PathBuf) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(&path)?;
        conn.busy_timeout(Duration::from_secs(3))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS reading_history (
                key TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                kind TEXT NOT NULL,
                archive_format TEXT,
                title TEXT NOT NULL,
                last_read_at_ms INTEGER NOT NULL,
                last_page INTEGER,
                page_count INTEGER,
                file_size INTEGER,
                mtime_ms INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_reading_history_last_read
                ON reading_history(last_read_at_ms DESC);",
        )?;
        Ok(Self { conn })
    }

    pub fn db_path() -> PathBuf {
        crate::data_dir::get().join("reading_history.db")
    }

    pub fn upsert(
        &self,
        mut entry: ReadingHistoryEntry,
        limit: usize,
    ) -> Result<(), rusqlite::Error> {
        let limit = clamp_limit(limit);
        if entry.key.is_empty() {
            entry.key = normalize_path_keep_drive(&entry.path);
        }
        if entry.file_size.is_none() || entry.mtime_ms.is_none() {
            let (file_size, mtime_ms) = path_metadata(&entry.path);
            if entry.file_size.is_none() {
                entry.file_size = file_size;
            }
            if entry.mtime_ms.is_none() {
                entry.mtime_ms = mtime_ms;
            }
        }
        let existed: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM reading_history WHERE key = ?1",
                [&entry.key],
                |row| row.get(0),
            )
            .optional()?;
        self.conn.execute(
            "INSERT INTO reading_history (
                key, path, kind, archive_format, title, last_read_at_ms,
                last_page, page_count, file_size, mtime_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(key) DO UPDATE SET
                path = excluded.path,
                kind = excluded.kind,
                archive_format = excluded.archive_format,
                title = excluded.title,
                last_read_at_ms = excluded.last_read_at_ms,
                last_page = excluded.last_page,
                page_count = excluded.page_count,
                file_size = excluded.file_size,
                mtime_ms = excluded.mtime_ms",
            rusqlite::params![
                entry.key,
                entry.path.to_string_lossy(),
                entry.kind.as_str(),
                archive_format_to_str(entry.archive_format),
                entry.title,
                entry.last_read_at_ms,
                entry.last_page,
                entry.page_count,
                entry.file_size,
                entry.mtime_ms,
            ],
        )?;
        if existed.is_none() {
            self.prune(limit)?;
        }
        Ok(())
    }

    pub fn list_recent(&self, limit: usize) -> Result<Vec<ReadingHistoryEntry>, rusqlite::Error> {
        let limit = clamp_limit(limit);
        let mut stmt = self.conn.prepare_cached(
            "SELECT key, path, kind, archive_format, title, last_read_at_ms,
                    last_page, page_count, file_size, mtime_ms
             FROM reading_history
             ORDER BY last_read_at_ms DESC, key ASC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], read_entry_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn remove_key(&self, key: &str) -> Result<(), rusqlite::Error> {
        self.conn
            .execute("DELETE FROM reading_history WHERE key = ?1", [key])?;
        Ok(())
    }

    pub fn remove_keys(&self, keys: &[String]) -> Result<usize, rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        let mut removed = 0usize;
        {
            let mut stmt = tx.prepare_cached("DELETE FROM reading_history WHERE key = ?1")?;
            for key in keys {
                removed += stmt.execute([key])?;
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    pub fn clear_all(&self) -> Result<usize, rusqlite::Error> {
        self.conn.execute("DELETE FROM reading_history", [])
    }

    pub fn count(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM reading_history", [], |row| row.get(0))
            .unwrap_or(0)
    }

    pub fn prune(&self, limit: usize) -> Result<usize, rusqlite::Error> {
        let limit = clamp_limit(limit);
        self.conn.execute(
            "DELETE FROM reading_history
             WHERE key IN (
                SELECT key FROM reading_history
                ORDER BY last_read_at_ms DESC, key ASC
                LIMIT -1 OFFSET ?1
             )",
            [limit as i64],
        )
    }
}

/// 読書履歴の書き込みを UI スレッドから外す background writer。
pub struct ReadingHistoryWriter {
    tx: Option<mpsc::Sender<ReadingHistoryCommand>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

enum ReadingHistoryCommand {
    Upsert(ReadingHistoryEntry, usize),
    RemoveKeys(Vec<String>),
    Prune(usize),
}

impl ReadingHistoryWriter {
    pub fn spawn() -> Option<Self> {
        let (tx, rx) = mpsc::channel::<ReadingHistoryCommand>();
        let spawned = std::thread::Builder::new()
            .name("reading-history-writer".into())
            .spawn(move || {
                let db = match ReadingHistoryDb::open() {
                    Ok(db) => db,
                    Err(e) => {
                        crate::logger::log(format!("reading-history writer: DB open failed: {e}"));
                        return;
                    }
                };
                while let Ok(command) = rx.recv() {
                    let result = match command {
                        ReadingHistoryCommand::Upsert(entry, limit) => db.upsert(entry, limit),
                        ReadingHistoryCommand::RemoveKeys(keys) => {
                            db.remove_keys(&keys).map(|_| ())
                        }
                        ReadingHistoryCommand::Prune(limit) => db.prune(limit).map(|_| ()),
                    };
                    if let Err(e) = result {
                        crate::logger::log(format!("reading-history writer: write failed: {e}"));
                    }
                }
            });
        match spawned {
            Ok(handle) => Some(Self {
                tx: Some(tx),
                handle: Some(handle),
            }),
            Err(e) => {
                crate::logger::log(format!("reading-history writer: thread spawn failed: {e}"));
                None
            }
        }
    }

    pub fn record(&self, entry: ReadingHistoryEntry, limit: usize) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(ReadingHistoryCommand::Upsert(entry, limit));
        }
    }

    pub fn prune(&self, limit: usize) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(ReadingHistoryCommand::Prune(limit));
        }
    }

    pub fn remove_keys(&self, keys: Vec<String>) {
        if keys.is_empty() {
            return;
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(ReadingHistoryCommand::RemoveKeys(keys));
        }
    }
}

impl Drop for ReadingHistoryWriter {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn read_entry_row(row: &rusqlite::Row<'_>) -> Result<ReadingHistoryEntry, rusqlite::Error> {
    let kind_raw: String = row.get(2)?;
    let format_raw: Option<String> = row.get(3)?;
    Ok(ReadingHistoryEntry {
        key: row.get(0)?,
        path: PathBuf::from(row.get::<_, String>(1)?),
        kind: ReadingHistoryKind::from_str(&kind_raw).unwrap_or(ReadingHistoryKind::Folder),
        archive_format: format_raw.as_deref().and_then(archive_format_from_str),
        title: row.get(4)?,
        last_read_at_ms: row.get(5)?,
        last_page: row.get(6)?,
        page_count: row.get(7)?,
        file_size: row.get(8)?,
        mtime_ms: row.get(9)?,
    })
}

fn normalize_path_keep_drive(path: &Path) -> String {
    crate::path_key::normalize_keep_drive(path)
}

fn clamp_limit(limit: usize) -> usize {
    limit.clamp(1, READING_HISTORY_LIMIT_MAX)
}

fn archive_format_to_str(format: Option<ArchiveFormat>) -> Option<&'static str> {
    match format? {
        ArchiveFormat::Rar => Some("rar"),
        ArchiveFormat::SevenZ => Some("7z"),
        ArchiveFormat::Lzh => Some("lzh"),
        ArchiveFormat::Zip => Some("zip"),
    }
}

pub fn archive_format_from_str(raw: &str) -> Option<ArchiveFormat> {
    match raw {
        "rar" => Some(ArchiveFormat::Rar),
        "7z" => Some(ArchiveFormat::SevenZ),
        "lzh" => Some(ArchiveFormat::Lzh),
        "zip" => Some(ArchiveFormat::Zip),
        _ => None,
    }
}

pub fn archive_format_for_path(path: &Path) -> Option<ArchiveFormat> {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(ArchiveFormat::nested_from_extension)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn path_metadata(path: &Path) -> (Option<i64>, Option<i64>) {
    let Ok(meta) = path.metadata() else {
        return (None, None);
    };
    let file_size = meta
        .is_file()
        .then_some(meta.len().min(i64::MAX as u64) as i64);
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64);
    (file_size, mtime_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (tempfile::TempDir, ReadingHistoryDb) {
        let dir = tempfile::tempdir().unwrap();
        let db = ReadingHistoryDb::open_at(dir.path().join("reading_history.db")).unwrap();
        (dir, db)
    }

    fn entry(path: &str, page: i64) -> ReadingHistoryEntry {
        ReadingHistoryEntry {
            key: normalize_path_keep_drive(Path::new(path)),
            path: PathBuf::from(path),
            kind: ReadingHistoryKind::Zip,
            archive_format: Some(ArchiveFormat::Zip),
            title: Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string()),
            last_read_at_ms: 1_000 + page,
            last_page: Some(page),
            page_count: Some(10),
            file_size: None,
            mtime_ms: None,
        }
    }

    #[test]
    fn upsert_list_remove_clear() {
        let (_dir, db) = temp_db();
        let mut a = entry("C:/Books/a.zip", 1);
        let b = entry("C:/Books/b.pdf", 2);
        db.upsert(a.clone(), 1000).unwrap();
        db.upsert(b.clone(), 1000).unwrap();

        let rows = db.list_recent(1000).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].path, b.path);

        a.last_read_at_ms = 2_000;
        a.last_page = Some(5);
        db.upsert(a.clone(), 1000).unwrap();
        assert_eq!(db.count(), 2);
        let rows = db.list_recent(1000).unwrap();
        assert_eq!(rows[0].path, a.path);
        assert_eq!(rows[0].last_page, Some(5));

        db.remove_key(&a.key).unwrap();
        assert_eq!(db.count(), 1);
        let removed = db
            .remove_keys(&[b.key.clone(), "missing".to_string()])
            .unwrap();
        assert_eq!(removed, 1);
        assert_eq!(db.count(), 0);
        db.upsert(b.clone(), 1000).unwrap();
        assert_eq!(db.clear_all().unwrap(), 1);
        assert_eq!(db.count(), 0);
    }

    #[test]
    fn prune_keeps_recent_limit() {
        let (_dir, db) = temp_db();
        for i in 0..5 {
            db.upsert(entry(&format!("C:/Books/{i}.zip"), i), 3)
                .unwrap();
        }
        let rows = db.list_recent(1000).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].last_page, Some(4));
        assert_eq!(rows[2].last_page, Some(2));
    }
}
