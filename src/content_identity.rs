//! 編集済みコンテンツの物理 identity を記録する append-on-edit ledger。
//!
//! UI 側は物理ファイル 1 件を channel へ渡すだけにし、metadata 取得・
//! ファイル読み出し・SHA-256・SQLite はすべて専用 worker が行う。
//! 新規ストアなので旧スキーマの migration は持たず、初回 open で正本スキーマを作る。

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};

const HEAD_HASH_BYTES: u64 = 64 * 1024;
const HASH_CHUNK_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ContentIdentityTrigger {
    ViewingState,
    Edit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContentKind {
    Image,
    Zip,
    Pdf,
    Convertible,
}

impl ContentKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Zip => "zip",
            Self::Pdf => "pdf",
            Self::Convertible => "convertible",
        }
    }
}

/// 編集状態が属する物理ファイル。ZIP/PDF のページはコンテナ、変換アーカイブの
/// キャッシュ ZIP は元アーカイブを指す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ContentIdentitySource {
    pub(crate) path: PathBuf,
    pub(crate) kind: ContentKind,
}

impl ContentIdentitySource {
    pub(crate) fn new(path: impl Into<PathBuf>, kind: ContentKind) -> Self {
        Self {
            path: path.into(),
            kind,
        }
    }

    /// 拡張子だけで物理対象を分類する。ファイルを開く処理は含まない。
    pub(crate) fn from_path(path: &Path) -> Option<Self> {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let kind = if crate::folder_tree::is_zip_extension(&extension) {
            ContentKind::Zip
        } else if extension == "pdf" {
            ContentKind::Pdf
        } else if crate::archive_converter::ArchiveFormat::from_extension(&extension).is_some() {
            ContentKind::Convertible
        } else if crate::folder_tree::is_recognized_image_ext(&extension) {
            ContentKind::Image
        } else {
            return None;
        };
        Some(Self::new(path, kind))
    }

    pub(crate) fn for_grid_item(
        item: &crate::grid_item::GridItem,
        archive_source_override: Option<&Path>,
        current_folder: Option<&Path>,
    ) -> Option<Self> {
        use crate::grid_item::GridItem;

        let archive_root = |container: &Path| {
            if let Some(source) = archive_source_override
                && current_folder
                    .is_some_and(|current| crate::folder_tree::path_eq(current, container))
            {
                source.to_path_buf()
            } else {
                container.to_path_buf()
            }
        };

        match item {
            GridItem::Image(path) => Some(Self::new(path, ContentKind::Image)),
            GridItem::ZipFile(path) => Some(Self::new(path, ContentKind::Zip)),
            GridItem::PdfFile(path) => Some(Self::new(path, ContentKind::Pdf)),
            GridItem::ConvertibleArchive { path, .. } => {
                Some(Self::new(path, ContentKind::Convertible))
            }
            GridItem::ZipImage { zip_path, .. } | GridItem::ZipDir { zip_path, .. } => {
                let root = archive_root(zip_path);
                if root.as_path() != zip_path {
                    Some(Self::new(root, ContentKind::Convertible))
                } else {
                    Self::from_path(&root)
                }
            }
            GridItem::PdfPage { pdf_path, .. } => Some(Self::new(pdf_path, ContentKind::Pdf)),
            GridItem::Folder(_)
            | GridItem::Video(_)
            | GridItem::Audio(_)
            | GridItem::Stack { .. }
            | GridItem::SearchContainer { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecordedFileState {
    pub(crate) file_key: String,
    pub(crate) size: u64,
    pub(crate) hashed_mtime: i64,
}

#[derive(Clone, Debug)]
struct RecordRequest {
    source: ContentIdentitySource,
    trigger: ContentIdentityTrigger,
    recorded_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CoalescedRecordRequest {
    file_key: String,
    source: ContentIdentitySource,
    trigger: ContentIdentityTrigger,
    recorded_at: i64,
}

/// 前回記録と物理観測が同一なら、大きなファイルを再度読み出す必要はない。
pub(crate) fn needs_rehashing(
    recorded: Option<&RecordedFileState>,
    file_key: &str,
    size: u64,
    hashed_mtime: i64,
) -> bool {
    !recorded.is_some_and(|recorded| {
        recorded.file_key == file_key
            && recorded.size == size
            && recorded.hashed_mtime == hashed_mtime
    })
}

/// Stage 1: 先頭 64 KiB とファイルサイズ (little-endian u64) の SHA-256。
pub(crate) fn stage1_head_hash<R: Read>(reader: &mut R, size: u64) -> io::Result<String> {
    hash_reader(reader, Some(HEAD_HASH_BYTES), Some(size), || false)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Interrupted,
            "content identity hashing cancelled",
        )
    })
}

/// Stage 2: ファイル全体の SHA-256。大きなファイルでも途中で打ち切らない。
pub(crate) fn stage2_full_hash<R: Read>(reader: &mut R) -> io::Result<String> {
    hash_reader(reader, None, None, || false)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Interrupted,
            "content identity hashing cancelled",
        )
    })
}

fn hash_reader<R: Read>(
    reader: &mut R,
    limit: Option<u64>,
    size_suffix: Option<u64>,
    is_cancelled: impl Fn() -> bool,
) -> io::Result<Option<String>> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_CHUNK_BYTES];
    let mut remaining = limit.unwrap_or(u64::MAX);
    while remaining > 0 {
        if is_cancelled() {
            return Ok(None);
        }
        let read_len = remaining.min(buffer.len() as u64) as usize;
        let count = reader.read(&mut buffer[..read_len])?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        remaining -= count as u64;
    }
    if is_cancelled() {
        return Ok(None);
    }
    if let Some(size) = size_suffix {
        hasher.update(size.to_le_bytes());
    }
    Ok(Some(format!("{:x}", hasher.finalize())))
}

struct ContentIdentityDb {
    conn: rusqlite::Connection,
}

struct StoredRecord {
    state: RecordedFileState,
    last_edit_at: i64,
}

impl ContentIdentityDb {
    fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&crate::data_dir::get().join("content_identity.db"))
    }

    fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS edit_origin (
                 file_key TEXT PRIMARY KEY,
                 size INTEGER NOT NULL,
                 head_hash TEXT NOT NULL,
                 full_hash TEXT,
                 hashed_mtime INTEGER NOT NULL,
                 kind TEXT NOT NULL,
                 last_edit_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS edit_origin_full ON edit_origin(full_hash);
             CREATE TABLE IF NOT EXISTS restore_declined (
                 full_hash TEXT NOT NULL,
                 target_key TEXT NOT NULL,
                 PRIMARY KEY(full_hash, target_key)
             );",
        )?;
        Ok(Self { conn })
    }

    fn recorded_state(&self, file_key: &str) -> Result<Option<StoredRecord>, String> {
        self.conn
            .query_row(
                "SELECT file_key, size, hashed_mtime, last_edit_at
                   FROM edit_origin WHERE file_key = ?1",
                [file_key],
                |row| {
                    let size: i64 = row.get(1)?;
                    let size = u64::try_from(size).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?;
                    Ok(StoredRecord {
                        state: RecordedFileState {
                            file_key: row.get(0)?,
                            size,
                            hashed_mtime: row.get(2)?,
                        },
                        last_edit_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    fn touch_edit(
        &self,
        file_key: &str,
        kind: ContentKind,
        last_edit_at: i64,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE edit_origin SET kind = ?2, last_edit_at = ?3 WHERE file_key = ?1",
                rusqlite::params![file_key, kind.as_str(), last_edit_at],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn upsert(
        &self,
        source: &ContentIdentitySource,
        state: &RecordedFileState,
        head_hash: &str,
        full_hash: &str,
        last_edit_at: i64,
    ) -> Result<(), String> {
        let size = i64::try_from(state.size)
            .map_err(|_| "file size exceeds SQLite INTEGER".to_string())?;
        self.conn
            .execute(
                "INSERT INTO edit_origin
                     (file_key, size, head_hash, full_hash, hashed_mtime, kind, last_edit_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(file_key) DO UPDATE SET
                     size = excluded.size,
                     head_hash = excluded.head_hash,
                     full_hash = excluded.full_hash,
                     hashed_mtime = excluded.hashed_mtime,
                     kind = excluded.kind,
                     last_edit_at = excluded.last_edit_at",
                rusqlite::params![
                    state.file_key,
                    size,
                    head_hash,
                    full_hash,
                    state.hashed_mtime,
                    source.kind.as_str(),
                    last_edit_at,
                ],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

pub(crate) struct ContentIdentityRecorder {
    tx: Option<mpsc::Sender<RecordRequest>>,
    handle: Option<std::thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl ContentIdentityRecorder {
    pub(crate) fn spawn() -> Option<Self> {
        let (tx, rx) = mpsc::channel::<RecordRequest>();
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        match std::thread::Builder::new()
            .name("content-identity-recorder".into())
            .spawn(move || run_worker(rx, worker_shutdown))
        {
            Ok(handle) => Some(Self {
                tx: Some(tx),
                handle: Some(handle),
                shutdown,
            }),
            Err(error) => {
                crate::logger::log(format!(
                    "content_identity: recorder thread spawn failed: {error}"
                ));
                None
            }
        }
    }

    /// UI thread では channel 送信だけを行う。metadata 取得を含む I/O は worker 側。
    pub(crate) fn record(&self, source: ContentIdentitySource, trigger: ContentIdentityTrigger) {
        let display = source.path.display().to_string();
        let request = RecordRequest {
            source,
            trigger,
            recorded_at: unix_time_millis(),
        };
        if self.tx.as_ref().is_none_or(|tx| tx.send(request).is_err()) {
            crate::logger::log(format!(
                "content_identity: recorder unavailable for {display}"
            ));
        }
    }
}

impl Drop for ContentIdentityRecorder {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.tx = None;
        if let Some(handle) = self.handle.take()
            && handle.join().is_err()
        {
            crate::logger::log("content_identity: recorder thread panicked".to_string());
        }
    }
}

fn run_worker(rx: mpsc::Receiver<RecordRequest>, shutdown: Arc<AtomicBool>) {
    let db = match ContentIdentityDb::open() {
        Ok(db) => db,
        Err(error) => {
            crate::logger::log(format!("content_identity: DB open failed: {error}"));
            return;
        }
    };
    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let first = match rx.recv() {
            Ok(request) => request,
            Err(_) => break,
        };
        let mut queued = vec![first];
        while let Ok(request) = rx.try_recv() {
            queued.push(request);
        }
        let coalesced = coalesce_record_requests(queued);
        process_coalesced_requests(&coalesced, &shutdown, |request| {
            if let Err(error) = record_source(&db, request, &shutdown) {
                crate::logger::log(format!(
                    "content_identity: recording failed for {}: {error}",
                    request.source.path.display()
                ));
            }
        });
    }
}

fn coalesce_record_requests(requests: Vec<RecordRequest>) -> Vec<CoalescedRecordRequest> {
    let mut index_by_key = HashMap::<String, usize>::new();
    let mut coalesced = Vec::<CoalescedRecordRequest>::new();
    for request in requests {
        let file_key = crate::path_key::normalize_keep_drive(&request.source.path);
        if let Some(index) = index_by_key.get(&file_key).copied() {
            let existing = &mut coalesced[index];
            existing.trigger = existing.trigger.max(request.trigger);
            if request.recorded_at > existing.recorded_at {
                existing.recorded_at = request.recorded_at;
                existing.source = request.source;
            }
        } else {
            index_by_key.insert(file_key.clone(), coalesced.len());
            coalesced.push(CoalescedRecordRequest {
                file_key,
                source: request.source,
                trigger: request.trigger,
                recorded_at: request.recorded_at,
            });
        }
    }
    coalesced
}

fn process_coalesced_requests(
    requests: &[CoalescedRecordRequest],
    shutdown: &AtomicBool,
    mut process: impl FnMut(&CoalescedRecordRequest),
) {
    for request in requests {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        process(request);
    }
}

fn record_source(
    db: &ContentIdentityDb,
    request: &CoalescedRecordRequest,
    shutdown: &AtomicBool,
) -> Result<(), String> {
    let source = &request.source;
    let before = std::fs::metadata(&source.path).map_err(|error| error.to_string())?;
    if !before.is_file() {
        return Err("source is not a regular file".to_string());
    }
    let state = RecordedFileState {
        file_key: request.file_key.clone(),
        size: before.len(),
        hashed_mtime: metadata_mtime(&before)?,
    };
    record_observation_with_hasher(
        db,
        source,
        &state,
        request.trigger,
        request.recorded_at,
        || {
            let mut file = File::open(&source.path).map_err(|error| error.to_string())?;
            let head_hash =
                match hash_reader(&mut file, Some(HEAD_HASH_BYTES), Some(state.size), || {
                    shutdown.load(Ordering::Acquire)
                })
                .map_err(|error| error.to_string())?
                {
                    Some(hash) => hash,
                    None => return Ok(None),
                };
            file.rewind().map_err(|error| error.to_string())?;
            let full_hash =
                match hash_reader(&mut file, None, None, || shutdown.load(Ordering::Acquire))
                    .map_err(|error| error.to_string())?
                {
                    Some(hash) => hash,
                    None => return Ok(None),
                };

            let after = file.metadata().map_err(|error| error.to_string())?;
            if after.len() != state.size || metadata_mtime(&after)? != state.hashed_mtime {
                return Err("source changed while hashing".to_string());
            }
            if shutdown.load(Ordering::Acquire) {
                return Ok(None);
            }
            Ok(Some((head_hash, full_hash)))
        },
    )
}

fn record_observation_with_hasher(
    db: &ContentIdentityDb,
    source: &ContentIdentitySource,
    state: &RecordedFileState,
    trigger: ContentIdentityTrigger,
    last_edit_at: i64,
    hasher: impl FnOnce() -> Result<Option<(String, String)>, String>,
) -> Result<(), String> {
    let recorded = db.recorded_state(&state.file_key)?;
    if !needs_rehashing(
        recorded.as_ref().map(|recorded| &recorded.state),
        &state.file_key,
        state.size,
        state.hashed_mtime,
    ) {
        return match trigger {
            ContentIdentityTrigger::Edit => {
                db.touch_edit(&state.file_key, source.kind, last_edit_at)
            }
            ContentIdentityTrigger::ViewingState => Ok(()),
        };
    }
    let Some((head_hash, full_hash)) = hasher()? else {
        return Ok(());
    };
    let stored_last_edit_at = match trigger {
        ContentIdentityTrigger::Edit => last_edit_at,
        ContentIdentityTrigger::ViewingState => recorded
            .as_ref()
            .map(|recorded| recorded.last_edit_at)
            .unwrap_or(0),
    };
    db.upsert(source, state, &head_hash, &full_hash, stored_last_edit_at)
}

fn metadata_mtime(metadata: &std::fs::Metadata) -> Result<i64, String> {
    let modified = metadata.modified().map_err(|error| error.to_string())?;
    Ok(system_time_nanos(modified))
}

fn system_time_nanos(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos().min(i64::MAX as u128) as i64,
        Err(error) => -(error.duration().as_nanos().min(i64::MAX as u128) as i64),
    }
}

fn unix_time_millis() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis().min(i64::MAX as u128) as i64,
        Err(error) => -(error.duration().as_millis().min(i64::MAX as u128) as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn stage_hashes_have_stable_sha256_vectors() {
        let mut head = Cursor::new(b"abc");
        assert_eq!(
            stage1_head_hash(&mut head, 3).unwrap(),
            "baba775df93bdbf9d34cd8eb1cfe68727c19de118e74f374100e75baeea41d90"
        );
        let mut full = Cursor::new(b"abc");
        assert_eq!(
            stage2_full_hash(&mut full).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn stage2_hashes_the_tail_beyond_the_head_window() {
        let mut first = vec![7_u8; HEAD_HASH_BYTES as usize + 1];
        let mut second = first.clone();
        *first.last_mut().unwrap() = 8;
        *second.last_mut().unwrap() = 9;
        let mut first_head = Cursor::new(&first);
        let mut second_head = Cursor::new(&second);
        assert_eq!(
            stage1_head_hash(&mut first_head, first.len() as u64).unwrap(),
            stage1_head_hash(&mut second_head, second.len() as u64).unwrap()
        );
        let mut first_full = Cursor::new(first);
        let mut second_full = Cursor::new(second);
        assert_ne!(
            stage2_full_hash(&mut first_full).unwrap(),
            stage2_full_hash(&mut second_full).unwrap()
        );
    }

    #[test]
    fn rehash_decision_uses_key_size_and_mtime() {
        let recorded = RecordedFileState {
            file_key: "c:/images/a.png".to_string(),
            size: 42,
            hashed_mtime: 100,
        };
        assert!(!needs_rehashing(
            Some(&recorded),
            "c:/images/a.png",
            42,
            100
        ));
        assert!(needs_rehashing(Some(&recorded), "d:/images/a.png", 42, 100));
        assert!(needs_rehashing(Some(&recorded), "c:/images/a.png", 43, 100));
        assert!(needs_rehashing(Some(&recorded), "c:/images/a.png", 42, 101));
        assert!(needs_rehashing(None, "c:/images/a.png", 42, 100));
    }

    #[test]
    fn unchanged_observation_skips_hasher_but_size_or_mtime_change_calls_it() {
        let temp = tempfile::tempdir().unwrap();
        let db = ContentIdentityDb::open_at(&temp.path().join("content_identity.db")).unwrap();
        let source = ContentIdentitySource::new("C:/images/a.png", ContentKind::Image);
        let original = RecordedFileState {
            file_key: "c:/images/a.png".to_string(),
            size: 42,
            hashed_mtime: 100,
        };
        db.upsert(&source, &original, "head-0", "full-0", 1)
            .unwrap();

        let calls = std::cell::Cell::new(0);
        record_observation_with_hasher(
            &db,
            &source,
            &original,
            ContentIdentityTrigger::Edit,
            2,
            || {
                calls.set(calls.get() + 1);
                Ok(Some(("head-1".to_string(), "full-1".to_string())))
            },
        )
        .unwrap();
        assert_eq!(calls.get(), 0, "同一観測では hash 関数を呼ばない");

        let changed_size = RecordedFileState {
            size: 43,
            ..original.clone()
        };
        record_observation_with_hasher(
            &db,
            &source,
            &changed_size,
            ContentIdentityTrigger::Edit,
            3,
            || {
                calls.set(calls.get() + 1);
                Ok(Some(("head-2".to_string(), "full-2".to_string())))
            },
        )
        .unwrap();
        assert_eq!(calls.get(), 1, "size 変更では再 hash する");

        let changed_mtime = RecordedFileState {
            hashed_mtime: 101,
            ..changed_size
        };
        record_observation_with_hasher(
            &db,
            &source,
            &changed_mtime,
            ContentIdentityTrigger::Edit,
            4,
            || {
                calls.set(calls.get() + 1);
                Ok(Some(("head-3".to_string(), "full-3".to_string())))
            },
        )
        .unwrap();
        assert_eq!(calls.get(), 2, "mtime 変更では再 hash する");
    }

    #[test]
    fn edit_advances_last_edit_at_while_viewing_state_does_not() {
        let temp = tempfile::tempdir().unwrap();
        let db = ContentIdentityDb::open_at(&temp.path().join("content_identity.db")).unwrap();
        let source = ContentIdentitySource::new("C:/images/a.png", ContentKind::Image);
        let state = RecordedFileState {
            file_key: "c:/images/a.png".to_string(),
            size: 42,
            hashed_mtime: 100,
        };
        db.upsert(&source, &state, "head", "full", 10).unwrap();

        record_observation_with_hasher(
            &db,
            &source,
            &state,
            ContentIdentityTrigger::ViewingState,
            20,
            || panic!("unchanged viewing state must not hash"),
        )
        .unwrap();
        assert_eq!(
            db.recorded_state(&state.file_key)
                .unwrap()
                .unwrap()
                .last_edit_at,
            10
        );

        record_observation_with_hasher(
            &db,
            &source,
            &state,
            ContentIdentityTrigger::Edit,
            30,
            || panic!("unchanged edit must not hash"),
        )
        .unwrap();
        assert_eq!(
            db.recorded_state(&state.file_key)
                .unwrap()
                .unwrap()
                .last_edit_at,
            30
        );
    }

    #[test]
    fn unchanged_viewing_state_performs_no_database_write() {
        let temp = tempfile::tempdir().unwrap();
        let db = ContentIdentityDb::open_at(&temp.path().join("content_identity.db")).unwrap();
        let source = ContentIdentitySource::new("C:/images/a.png", ContentKind::Image);
        let state = RecordedFileState {
            file_key: "c:/images/a.png".to_string(),
            size: 42,
            hashed_mtime: 100,
        };
        db.upsert(&source, &state, "head", "full", 10).unwrap();
        let total_changes = || {
            db.conn
                .query_row("SELECT total_changes()", [], |row| row.get::<_, i64>(0))
                .unwrap()
        };
        let changes_before = total_changes();

        record_observation_with_hasher(
            &db,
            &source,
            &state,
            ContentIdentityTrigger::ViewingState,
            20,
            || panic!("unchanged viewing state must not hash"),
        )
        .unwrap();

        assert_eq!(total_changes(), changes_before);
    }

    #[test]
    fn coalescing_deduplicates_file_keys_and_keeps_edit_and_latest_timestamp() {
        let duplicate = |trigger, recorded_at| RecordRequest {
            source: ContentIdentitySource::new("C:/images/a.png", ContentKind::Image),
            trigger,
            recorded_at,
        };
        let requests = vec![
            duplicate(ContentIdentityTrigger::ViewingState, 10),
            duplicate(ContentIdentityTrigger::Edit, 20),
            duplicate(ContentIdentityTrigger::ViewingState, 30),
            RecordRequest {
                source: ContentIdentitySource::new("C:/images/b.png", ContentKind::Image),
                trigger: ContentIdentityTrigger::ViewingState,
                recorded_at: 40,
            },
        ];

        let coalesced = coalesce_record_requests(requests);

        assert_eq!(coalesced.len(), 2);
        assert_eq!(coalesced[0].trigger, ContentIdentityTrigger::Edit);
        assert_eq!(coalesced[0].recorded_at, 30);
    }

    #[test]
    fn worker_stops_starting_items_after_shutdown_is_set() {
        let requests = coalesce_record_requests(vec![
            RecordRequest {
                source: ContentIdentitySource::new("C:/images/a.png", ContentKind::Image),
                trigger: ContentIdentityTrigger::Edit,
                recorded_at: 10,
            },
            RecordRequest {
                source: ContentIdentitySource::new("C:/images/b.png", ContentKind::Image),
                trigger: ContentIdentityTrigger::Edit,
                recorded_at: 20,
            },
        ]);
        let shutdown = AtomicBool::new(false);
        let mut started = Vec::new();

        process_coalesced_requests(&requests, &shutdown, |request| {
            started.push(request.file_key.clone());
            shutdown.store(true, Ordering::Release);
        });

        assert_eq!(started, vec![requests[0].file_key.clone()]);
    }

    #[test]
    fn hash_loop_abandons_between_chunks_after_shutdown() {
        struct CancelAfterFirstChunk<'a> {
            inner: Cursor<Vec<u8>>,
            shutdown: &'a AtomicBool,
            reads: usize,
        }

        impl Read for CancelAfterFirstChunk<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                let count = self.inner.read(buffer)?;
                if count > 0 {
                    self.reads += 1;
                    self.shutdown.store(true, Ordering::Release);
                }
                Ok(count)
            }
        }

        let shutdown = AtomicBool::new(false);
        let mut reader = CancelAfterFirstChunk {
            inner: Cursor::new(vec![7_u8; HASH_CHUNK_BYTES * 2]),
            shutdown: &shutdown,
            reads: 0,
        };

        let hash =
            hash_reader(&mut reader, None, None, || shutdown.load(Ordering::Acquire)).unwrap();

        assert_eq!(hash, None);
        assert_eq!(reader.reads, 1);
    }

    #[test]
    fn schema_contains_both_a1_tables_and_full_hash_index() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("content_identity.db");
        let db = ContentIdentityDb::open_at(&path).unwrap();
        let tables = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type IN ('table', 'index')")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(tables.iter().any(|name| name == "edit_origin"));
        assert!(tables.iter().any(|name| name == "edit_origin_full"));
        assert!(tables.iter().any(|name| name == "restore_declined"));
    }

    #[test]
    fn grid_item_mapping_uses_physical_container_and_original_archive() {
        use crate::grid_item::GridItem;

        assert_eq!(
            ContentIdentitySource::from_path(Path::new("C:/images/a.JPEG"))
                .map(|source| source.kind),
            Some(ContentKind::Image)
        );
        assert_eq!(
            ContentIdentitySource::from_path(Path::new("C:/books/a.cbz")).map(|source| source.kind),
            Some(ContentKind::Zip)
        );
        assert_eq!(
            ContentIdentitySource::from_path(Path::new("C:/books/a.PDF")).map(|source| source.kind),
            Some(ContentKind::Pdf)
        );
        assert_eq!(
            ContentIdentitySource::from_path(Path::new("C:/books/a.7z")).map(|source| source.kind),
            Some(ContentKind::Convertible)
        );

        let zip = PathBuf::from("C:/books/cache.zip");
        let original = PathBuf::from("C:/books/source.rar");
        let zip_page = GridItem::ZipImage {
            zip_path: zip.clone(),
            entry_name: "page.png".to_string(),
        };
        assert_eq!(
            ContentIdentitySource::for_grid_item(&zip_page, None, Some(&zip)),
            Some(ContentIdentitySource::new(&zip, ContentKind::Zip))
        );
        assert_eq!(
            ContentIdentitySource::for_grid_item(&zip_page, Some(&original), Some(&zip)),
            Some(ContentIdentitySource::new(
                &original,
                ContentKind::Convertible
            ))
        );

        let pdf = PathBuf::from("C:/books/book.pdf");
        let pdf_page = GridItem::PdfPage {
            pdf_path: pdf.clone(),
            page_num: 3,
            content_type: None,
        };
        assert_eq!(
            ContentIdentitySource::for_grid_item(&pdf_page, None, None),
            Some(ContentIdentitySource::new(pdf, ContentKind::Pdf))
        );
    }

    #[test]
    fn failed_recorder_submission_does_not_propagate_to_edit_caller() {
        let (tx, rx) = mpsc::channel();
        drop(rx);
        let recorder = ContentIdentityRecorder {
            tx: Some(tx),
            handle: None,
            shutdown: Arc::new(AtomicBool::new(false)),
        };
        recorder.record(
            ContentIdentitySource::new("C:/missing/image.png", ContentKind::Image),
            ContentIdentityTrigger::Edit,
        );
    }
}
