//! Folder-level cache for the last confirmed automatic thumbnail aspect.
//!
//! This cache is intentionally small: it stores only the last `ThumbAspect`
//! selected by the auto-aspect statistics for a folder/container. Thumbnail
//! image data and representative-folder thumbnail targets remain owned by the
//! existing catalog / folder-thumb-pin paths.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::collection_store::CollectionId;
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
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=5000;",
        )?;
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

    pub(crate) fn db_path() -> PathBuf {
        crate::data_dir::get().join("auto_aspect_cache.db")
    }

    pub fn get(&self, folder: &Path) -> Option<AutoAspectCacheEntry> {
        query_entry(&self.conn, folder)
    }

    /// IPC worker から既存 cache を読むための read-only 入口。
    /// DB が無い場合も作成せず、App 所有 connection と writer を増やさない。
    pub fn get_read_only(folder: &Path) -> Option<AutoAspectCacheEntry> {
        Self::get_read_only_at(&Self::db_path(), folder)
    }

    fn get_read_only_at(path: &Path, folder: &Path) -> Option<AutoAspectCacheEntry> {
        if !std::fs::metadata(path).ok()?.is_file() {
            return None;
        }
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;
        conn.execute_batch("PRAGMA query_only=ON; PRAGMA busy_timeout=5000;")
            .ok()?;
        query_entry(&conn, folder)
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

    pub fn delete_for_folder(&self, folder: &Path) -> Result<usize, rusqlite::Error> {
        let key = folder_key(folder);
        self.conn.execute(
            "DELETE FROM auto_aspect_cache WHERE folder_key = ?1",
            [&key],
        )
    }

    pub fn delete_older_than_days(&self, days: u64) -> Result<usize, rusqlite::Error> {
        let days = i64::try_from(days).unwrap_or(i64::MAX / 86_400);
        let cutoff = now_unix_secs().saturating_sub(days.saturating_mul(86_400));
        self.conn.execute(
            "DELETE FROM auto_aspect_cache WHERE updated_at <= ?1",
            [cutoff],
        )
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
fn query_entry(conn: &rusqlite::Connection, folder: &Path) -> Option<AutoAspectCacheEntry> {
    let key = folder_key(folder);
    let mut stmt = conn
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

/// Collection IDs are never coerced into the folder-key namespace. One worker owns every
/// Collection cache SQLite operation; the App-owned map only serves immediate same-process reads.
pub(crate) struct CollectionAutoAspectCache {
    client: CollectionAutoAspectCacheClient,
    entries: HashMap<CollectionId, AutoAspectCacheEntry>,
    handle: Option<std::thread::JoinHandle<()>>,
}

#[derive(Clone)]
pub(crate) struct CollectionAutoAspectCacheClient {
    submission: Arc<Mutex<CollectionCacheSubmission>>,
}

// The short critical section linearizes Get with maintenance admission. No SQLite work or
// receiver wait happens while held; a Get stamped with the new epoch is behind the clear.
struct CollectionCacheSubmission {
    tx: mpsc::Sender<CollectionCacheCommand>,
    epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CollectionAutoAspectLookup {
    pub(crate) epoch: u64,
    pub(crate) entry: AutoAspectCacheEntry,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum CollectionAutoAspectMaintenance {
    Count,
    ClearAll,
    DeleteOld { days: u64 },
}

pub(crate) type CollectionAutoAspectMaintenanceReply =
    mpsc::Receiver<Result<CollectionAutoAspectMaintenanceStats, String>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CollectionAutoAspectMaintenanceStats {
    pub(crate) deleted: usize,
    pub(crate) remaining: usize,
}

enum CollectionCacheCommand {
    Get {
        id: CollectionId,
        reply: mpsc::Sender<Result<Option<AutoAspectCacheEntry>, String>>,
    },
    Upsert {
        id: CollectionId,
        entry: AutoAspectCacheEntry,
    },
    Maintenance {
        operation: CollectionAutoAspectMaintenance,
        reply: mpsc::Sender<Result<CollectionAutoAspectMaintenanceStats, String>>,
    },
    Shutdown,
}

impl CollectionAutoAspectCache {
    pub(crate) fn spawn_at(path: PathBuf) -> Result<Self, std::io::Error> {
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("collection-auto-aspect-cache".into())
            .spawn(move || run_collection_cache_actor(path, rx))?;
        Ok(Self {
            client: CollectionAutoAspectCacheClient {
                submission: Arc::new(Mutex::new(CollectionCacheSubmission { tx, epoch: 0 })),
            },
            entries: HashMap::new(),
            handle: Some(handle),
        })
    }

    pub(crate) fn client(&self) -> CollectionAutoAspectCacheClient {
        self.client.clone()
    }

    pub(crate) fn cached(&self, id: CollectionId) -> Option<AutoAspectCacheEntry> {
        self.entries.get(&id).copied()
    }

    pub(crate) fn adopt_lookup(
        &mut self,
        id: CollectionId,
        lookup: Option<CollectionAutoAspectLookup>,
    ) -> Option<AutoAspectCacheEntry> {
        if let Some(entry) = self.cached(id) {
            return Some(entry);
        }
        let lookup = lookup?;
        if lookup.epoch != self.client.submission.lock().unwrap().epoch {
            return None;
        }
        self.entries.insert(id, lookup.entry);
        Some(lookup.entry)
    }

    pub(crate) fn record(
        &mut self,
        id: CollectionId,
        aspect: ThumbAspect,
        sample_count: usize,
        eligible_total: usize,
    ) -> Result<(), String> {
        let entry = AutoAspectCacheEntry {
            aspect,
            sample_count,
            eligible_total,
            updated_at: now_unix_secs(),
        };
        self.client
            .submission
            .lock()
            .unwrap()
            .tx
            .send(CollectionCacheCommand::Upsert { id, entry })
            .map_err(|_| "Collection Auto 比率 cache worker を利用できません".to_owned())?;
        self.entries.insert(id, entry);
        Ok(())
    }

    pub(crate) fn begin_maintenance(
        &mut self,
        operation: CollectionAutoAspectMaintenance,
    ) -> Result<CollectionAutoAspectMaintenanceReply, String> {
        let (reply, receiver) = mpsc::channel();
        let mut submission = self.client.submission.lock().unwrap();
        submission
            .tx
            .send(CollectionCacheCommand::Maintenance { operation, reply })
            .map_err(|_| "Collection Auto 比率 cache worker を利用できません".to_owned())?;
        if !matches!(operation, CollectionAutoAspectMaintenance::Count) {
            submission.epoch = submission.epoch.wrapping_add(1);
        }
        drop(submission);
        if !matches!(operation, CollectionAutoAspectMaintenance::Count) {
            self.entries.clear();
        }
        Ok(receiver)
    }
}

impl Drop for CollectionAutoAspectCache {
    fn drop(&mut self) {
        // Commands submitted before Shutdown, including final writes, are drained in order.
        let _ = self
            .client
            .submission
            .lock()
            .unwrap()
            .tx
            .send(CollectionCacheCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl CollectionAutoAspectCacheClient {
    /// Optional cache lookup on an existing prepare worker. Never wait on the UI thread.
    pub(crate) fn get_bounded(
        &self,
        id: CollectionId,
        cancel: &AtomicBool,
        budget: Duration,
    ) -> Option<CollectionAutoAspectLookup> {
        if cancel.load(Ordering::Acquire) {
            return None;
        }
        let (reply, receiver) = mpsc::channel();
        let epoch = {
            let submission = self.submission.lock().unwrap();
            submission
                .tx
                .send(CollectionCacheCommand::Get { id, reply })
                .ok()?;
            submission.epoch
        };
        let deadline = Instant::now() + budget;
        loop {
            if cancel.load(Ordering::Acquire) {
                return None;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match receiver.recv_timeout(remaining.min(Duration::from_millis(10))) {
                Ok(Ok(Some(entry))) => return Some(CollectionAutoAspectLookup { epoch, entry }),
                Ok(Ok(None) | Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => return None,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

fn run_collection_cache_actor(path: PathBuf, rx: mpsc::Receiver<CollectionCacheCommand>) {
    let db = match CollectionAutoAspectDb::open_at(&path) {
        Ok(db) => db,
        Err(error) => {
            crate::logger::log(format!("collection auto-aspect cache open failed: {error}"));
            return;
        }
    };
    while let Ok(command) = rx.recv() {
        match command {
            CollectionCacheCommand::Get { id, reply } => {
                let _ = reply.send(db.get_collection(id).map_err(|error| error.to_string()));
            }
            CollectionCacheCommand::Upsert { id, entry } => {
                if let Err(error) = db.upsert_collection(id, entry) {
                    crate::logger::log(format!(
                        "collection auto-aspect cache save failed: {error}"
                    ));
                }
            }
            CollectionCacheCommand::Maintenance { operation, reply } => {
                let deleted = match operation {
                    CollectionAutoAspectMaintenance::Count => Ok(0),
                    CollectionAutoAspectMaintenance::ClearAll => db.clear_collections(),
                    CollectionAutoAspectMaintenance::DeleteOld { days } => {
                        db.delete_old_collections(days)
                    }
                };
                let result = deleted.and_then(|deleted| {
                    Ok(CollectionAutoAspectMaintenanceStats {
                        deleted,
                        remaining: db.count_collections()?,
                    })
                });
                let _ = reply.send(result.map_err(|error| error.to_string()));
            }
            CollectionCacheCommand::Shutdown => break,
        }
    }
}

struct CollectionAutoAspectDb {
    conn: rusqlite::Connection,
}

impl CollectionAutoAspectDb {
    fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS collection_auto_aspect_cache (
                collection_id TEXT PRIMARY KEY,
                aspect INTEGER NOT NULL,
                sample_count INTEGER NOT NULL DEFAULT 0,
                eligible_total INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL
             )",
        )?;
        Ok(Self { conn })
    }

    fn get_collection(
        &self,
        id: CollectionId,
    ) -> Result<Option<AutoAspectCacheEntry>, rusqlite::Error> {
        use rusqlite::OptionalExtension;
        let raw = self
            .conn
            .query_row(
                "SELECT aspect, sample_count, eligible_total, updated_at \
             FROM collection_auto_aspect_cache WHERE collection_id = ?1",
                [id.as_uuid().to_string()],
                |row| {
                    Ok((
                        row.get::<_, i32>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        Ok(
            raw.and_then(|(aspect, sample_count, eligible_total, updated_at)| {
                Some(AutoAspectCacheEntry {
                    aspect: aspect_from_int(aspect)?,
                    sample_count: sample_count.max(0) as usize,
                    eligible_total: eligible_total.max(0) as usize,
                    updated_at,
                })
            }),
        )
    }

    fn upsert_collection(
        &self,
        id: CollectionId,
        entry: AutoAspectCacheEntry,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO collection_auto_aspect_cache \
             (collection_id, aspect, sample_count, eligible_total, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(collection_id) DO UPDATE SET \
               aspect = excluded.aspect, sample_count = excluded.sample_count, \
               eligible_total = excluded.eligible_total, updated_at = excluded.updated_at",
            rusqlite::params![
                id.as_uuid().to_string(),
                aspect_to_int(entry.aspect),
                entry.sample_count as i64,
                entry.eligible_total as i64,
                entry.updated_at
            ],
        )?;
        Ok(())
    }

    fn count_collections(&self) -> Result<usize, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM collection_auto_aspect_cache",
            [],
            |row| row.get(0),
        )
    }

    fn clear_collections(&self) -> Result<usize, rusqlite::Error> {
        self.conn
            .execute("DELETE FROM collection_auto_aspect_cache", [])
    }

    fn delete_old_collections(&self, days: u64) -> Result<usize, rusqlite::Error> {
        let days = i64::try_from(days).unwrap_or(i64::MAX / 86_400);
        let cutoff = now_unix_secs().saturating_sub(days.saturating_mul(86_400));
        self.conn.execute(
            "DELETE FROM collection_auto_aspect_cache WHERE updated_at <= ?1",
            [cutoff],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn maintenance(
        cache: &mut CollectionAutoAspectCache,
        operation: CollectionAutoAspectMaintenance,
    ) -> CollectionAutoAspectMaintenanceStats {
        cache
            .begin_maintenance(operation)
            .unwrap()
            .recv()
            .unwrap()
            .unwrap()
    }

    #[test]
    fn collection_cache_persists_by_uuid_without_changing_folder_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("auto_aspect_cache.db");
        let id = CollectionId::new();
        let other = CollectionId::new();
        let folder = Path::new("C:/Books/One");
        let db = AutoAspectCacheDb::open_at(&path).unwrap();
        db.upsert(folder, ThumbAspect::Portrait2x3, 9, 20).unwrap();
        let mut cache = CollectionAutoAspectCache::spawn_at(path.clone()).unwrap();
        cache
            .record(id, ThumbAspect::Landscape16x9, 13, 30)
            .unwrap();
        assert_eq!(cache.cached(id).unwrap().aspect, ThumbAspect::Landscape16x9);
        assert_eq!(
            maintenance(&mut cache, CollectionAutoAspectMaintenance::Count).remaining,
            1
        );
        drop(cache); // queued Upsert is drained before shutdown

        let mut cache = CollectionAutoAspectCache::spawn_at(path.clone()).unwrap();
        let cancel = AtomicBool::new(false);
        let lookup = cache
            .client()
            .get_bounded(id, &cancel, Duration::from_secs(1))
            .unwrap();
        assert_eq!(lookup.entry.aspect, ThumbAspect::Landscape16x9);
        assert!(
            cache
                .client()
                .get_bounded(other, &cancel, Duration::from_secs(1))
                .is_none()
        );
        assert_eq!(cache.adopt_lookup(id, Some(lookup)), Some(lookup.entry));
        assert_eq!(db.get(folder).unwrap().aspect, ThumbAspect::Portrait2x3);

        let stats = maintenance(
            &mut cache,
            CollectionAutoAspectMaintenance::DeleteOld { days: 0 },
        );
        assert_eq!(
            stats,
            CollectionAutoAspectMaintenanceStats {
                deleted: 1,
                remaining: 0
            }
        );
        assert!(cache.cached(id).is_none());
        assert_eq!(db.count(), 1); // folder table is unchanged
    }

    #[test]
    fn collection_cache_clear_orders_get_and_rejects_old_epoch() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("auto_aspect_cache.db");
        let id = CollectionId::new();
        let mut cache = CollectionAutoAspectCache::spawn_at(path).unwrap();
        cache.record(id, ThumbAspect::Portrait3x4, 11, 22).unwrap();
        let cancel = AtomicBool::new(false);
        let stale = cache
            .client()
            .get_bounded(id, &cancel, Duration::from_secs(1))
            .unwrap();
        let stats = maintenance(&mut cache, CollectionAutoAspectMaintenance::ClearAll);
        assert_eq!(
            stats,
            CollectionAutoAspectMaintenanceStats {
                deleted: 1,
                remaining: 0
            }
        );
        assert_eq!(cache.adopt_lookup(id, Some(stale)), None);
        assert!(
            cache
                .client()
                .get_bounded(id, &cancel, Duration::from_secs(1))
                .is_none()
        );
        cache.record(id, ThumbAspect::Landscape3x2, 12, 23).unwrap();
        let new = cache
            .client()
            .get_bounded(id, &cancel, Duration::from_secs(1))
            .unwrap();
        assert_eq!(new.entry.aspect, ThumbAspect::Landscape3x2);
    }

    #[test]
    fn collection_cache_get_is_optional_and_bounded() {
        let (tx, rx) = mpsc::channel();
        let client = CollectionAutoAspectCacheClient {
            submission: Arc::new(Mutex::new(CollectionCacheSubmission { tx, epoch: 0 })),
        };
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        assert!(
            client
                .get_bounded(CollectionId::new(), &cancel, Duration::from_millis(25))
                .is_none()
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(rx.try_recv().is_ok(), true);
        cancel.store(true, Ordering::Release);
        assert!(
            client
                .get_bounded(CollectionId::new(), &cancel, Duration::from_secs(1))
                .is_none()
        );
        assert!(rx.try_recv().is_err());
    }

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
        let read_only =
            AutoAspectCacheDb::get_read_only_at(&db_path, folder).expect("read-only cache entry");
        assert_eq!(read_only, entry);
        assert_eq!(db.count(), 1);

        db.upsert(folder, ThumbAspect::Square, 8, 20)
            .expect("update cache");
        let entry = db.get(folder).expect("updated cache entry");
        assert_eq!(entry.aspect, ThumbAspect::Square);
        assert_eq!(entry.sample_count, 8);
        assert_eq!(entry.eligible_total, 20);
        assert_eq!(db.delete_for_folder(folder).expect("delete folder"), 1);
        assert_eq!(db.get(folder), None);
        assert_eq!(db.count(), 0);

        db.upsert(folder, ThumbAspect::Square, 8, 20)
            .expect("insert again");
        assert_eq!(db.delete_older_than_days(0).expect("delete old"), 1);
        assert_eq!(db.count(), 0);

        db.upsert(folder, ThumbAspect::Square, 8, 20)
            .expect("insert once more");
        assert_eq!(db.clear_all().expect("clear"), 1);
        assert_eq!(db.count(), 0);
    }
    #[test]
    fn read_only_lookup_does_not_create_a_missing_database() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("missing.db");
        assert_eq!(
            AutoAspectCacheDb::get_read_only_at(&db_path, Path::new(r"C:\Books")),
            None
        );
        assert!(!db_path.exists());
    }
}
