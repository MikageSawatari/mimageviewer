//! Local AI captions. All filesystem/SQLite operations here belong on workers.
//!
//! This unreleased store starts at schema 1; no migration is needed. Rollback
//! journaling is retained so metadata transfer can commit its attached DBs atomically.

use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

struct CommitWait {
    cancel: Arc<AtomicBool>,
    deadline: Instant,
}

thread_local! {
    // rusqlite 0.31's safe busy_handler accepts only a function pointer. Keep
    // owned state on the calling worker instead of passing borrowed FFI data.
    static COMMIT_WAIT: std::cell::RefCell<Option<CommitWait>> = const { std::cell::RefCell::new(None) };
    #[cfg(test)]
    static COMMIT_WAIT_NOTIFY: std::cell::RefCell<Option<std::sync::mpsc::Sender<()>>> = const { std::cell::RefCell::new(None) };
}

fn commit_busy(_attempt: i32) -> bool {
    let remaining = COMMIT_WAIT.with(|slot| {
        let slot = slot.borrow();
        let wait = slot.as_ref()?;
        if wait.cancel.load(Ordering::Acquire) {
            return None;
        }
        wait.deadline.checked_duration_since(Instant::now())
    });
    let Some(remaining) = remaining else {
        return false;
    };
    #[cfg(test)]
    COMMIT_WAIT_NOTIFY.with(|slot| {
        let notify = slot.borrow_mut().take();
        if let Some(notify) = notify {
            let _ = notify.send(());
        }
    });
    // SQLite invokes this on its DB worker while a reader holds the rollback
    // journal's commit lock. No TLS borrow spans the bounded SQLite busy wait.
    std::thread::sleep(remaining.min(Duration::from_millis(10)));
    COMMIT_WAIT.with(|slot| {
        slot.borrow().as_ref().is_some_and(|wait| {
            !wait.cancel.load(Ordering::Acquire) && Instant::now() < wait.deadline
        })
    })
}

struct CommitBusyGuard<'a> {
    conn: &'a Connection,
    previous: Option<CommitWait>,
}

impl<'a> CommitBusyGuard<'a> {
    fn install(
        conn: &'a Connection,
        cancel: &Arc<AtomicBool>,
        timeout: Duration,
    ) -> rusqlite::Result<Self> {
        let previous = COMMIT_WAIT.with(|slot| {
            slot.replace(Some(CommitWait {
                cancel: Arc::clone(cancel),
                deadline: Instant::now() + timeout,
            }))
        });
        let guard = Self { conn, previous };
        conn.busy_handler(Some(commit_busy))?;
        Ok(guard)
    }
}

impl Drop for CommitBusyGuard<'_> {
    fn drop(&mut self) {
        // Restore the store's normal busy timeout on success, cancellation,
        // SQLite failure, and unwinding. The callback retains no borrowed data.
        if let Err(error) = self.conn.busy_timeout(BUSY_TIMEOUT) {
            crate::logger::log(format!(
                "ai_caption: restoring busy timeout failed: {error}"
            ));
        }
        COMMIT_WAIT.with(|slot| *slot.borrow_mut() = self.previous.take());
    }
}

fn commit_unless_cancelled(
    conn: &Connection,
    tx: Transaction<'_>,
    cancel: &Arc<AtomicBool>,
    timeout: Duration,
) -> rusqlite::Result<bool> {
    if cancel.load(Ordering::Acquire) {
        tx.rollback()?;
        return Ok(false);
    }
    let _guard = CommitBusyGuard::install(conn, cancel, timeout)?;
    match tx.commit() {
        Ok(()) => Ok(true),
        // Transaction::commit drops/rolls back its transaction on error.
        // Only an interrupted busy wait is cancellation; real DB failures,
        // including an uncancelled five-second timeout, retain their error.
        Err(error)
            if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy)
                && cancel.load(Ordering::Acquire) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

/// Signature captured when the input is read, in portable metadata's units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileSignature {
    pub size: u64,
    pub mtime_ms: i64,
}

impl FileSignature {
    /// Uses already-read metadata; unavailable/pre-epoch modification times are invalid.
    pub fn from_metadata(metadata: &Metadata) -> Option<Self> {
        Some(Self {
            size: metadata.len(),
            mtime_ms: system_time_ms(metadata.modified().ok()?)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptionResult {
    Ok {
        caption: String,
        text: String,
        keywords: Vec<String>,
    },
    Failed,
}

/// V1-3's job passes the original image path as both paths, its read-time
/// signature, an empty cover_ref, and the model/prompt/language used for inference.
/// Only complete, normally terminated and validated output becomes `Ok`.
///
/// V1-4 supplies the book path as item_path. signature_path is the original book
/// file (including an EPUB, never its converted cache), or the actual file read
/// for a folder's cover. Its read-time signature and resolved cover_ref travel
/// with the result; EPUB signatures come from the generation actually read.
/// The saver does not resolve covers or know book kinds.
#[derive(Clone, Debug)]
pub struct SaveRequest {
    pub item_path: PathBuf,
    pub signature_path: PathBuf,
    pub signature: FileSignature,
    pub cover_ref: String,
    pub model_id: String,
    pub prompt_version: i64,
    pub lang: String,
    pub result: CaptionResult,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    /// After commit, V1-3 notifies the shared display cache (V1-4) and each
    /// index manager's wake flag (V2). No such owners are created in V1-2.
    Written,
    KeptExistingOk,
    Discarded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionStatus {
    Ok,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptionRow {
    pub item_key: String,
    pub signature: FileSignature,
    pub model_id: String,
    pub prompt_version: i64,
    pub lang: String,
    pub caption: String,
    pub visible_text: String,
    pub keywords: Vec<String>,
    pub cover_ref: String,
    pub status: CaptionStatus,
    pub created_at: i64,
    pub rev: i64,
    pub indexed: bool,
}

pub struct CaptionDb {
    conn: Connection,
}

impl CaptionDb {
    pub fn db_path() -> PathBuf {
        crate::data_dir::get().join("ai_caption.db")
    }

    pub fn open() -> rusqlite::Result<Self> {
        Self::open_at(Self::db_path())
    }

    /// Worker/test schema initialization. Does not enable WAL.
    pub fn open_at(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Self::ensure_schema(&conn)?;
        Ok(Self { conn })
    }

    /// Worker-only reads. Missing or not-yet-initialized stores are unpublished;
    /// never initializes the directory, schema, journal mode, or database file.
    pub fn open_read_only(path: impl AsRef<Path>) -> rusqlite::Result<Option<Self>> {
        let path = path.as_ref();
        let conn = match Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(conn) => conn,
            Err(error) => {
                if std::fs::metadata(path)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                {
                    return Ok(None);
                }
                return Err(error);
            }
        };
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // Schema and this version are published by the same initialization commit.
        // Opening the file can race that commit, so version zero is still absent.
        match conn.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))? {
            0 => Ok(None),
            1 => Ok(Some(Self { conn })),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }

    /// Existing, already-initialized database; never creates a missing store.
    pub fn open_existing_for_write(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(Self { conn })
    }

    fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != 0 && version != 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        // Readers use user_version as the publication boundary; commit it with all DDL.
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS captions (
                item_key TEXT PRIMARY KEY NOT NULL,
                mtime_ms INTEGER NOT NULL, size INTEGER NOT NULL,
                model_id TEXT NOT NULL, prompt_version INTEGER NOT NULL,
                lang TEXT NOT NULL, caption TEXT NOT NULL, visible_text TEXT NOT NULL,
                cover_ref TEXT NOT NULL, keywords TEXT NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('ok', 'failed')),
                created_at INTEGER NOT NULL,
                rev INTEGER NOT NULL DEFAULT 0, indexed INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS caption_seq (n INTEGER NOT NULL);
            INSERT INTO caption_seq SELECT 0 WHERE NOT EXISTS (SELECT 1 FROM caption_seq);
            CREATE TABLE IF NOT EXISTS caption_removed (
                item_key TEXT NOT NULL, rev INTEGER NOT NULL, PRIMARY KEY(item_key, rev)
            );
            CREATE TRIGGER IF NOT EXISTS captions_ins AFTER INSERT ON captions BEGIN
                UPDATE caption_seq SET n = n + 1;
                UPDATE captions SET rev = (SELECT n FROM caption_seq), indexed = 0
                    WHERE item_key = NEW.item_key;
            END;
            CREATE TRIGGER IF NOT EXISTS captions_upd AFTER UPDATE OF
                item_key, mtime_ms, size, model_id, prompt_version, caption,
                visible_text, cover_ref, keywords, status ON captions BEGIN
                UPDATE caption_seq SET n = n + 1;
                UPDATE captions SET rev = (SELECT n FROM caption_seq), indexed = 0
                    WHERE item_key = NEW.item_key;
                INSERT OR IGNORE INTO caption_removed
                    SELECT OLD.item_key, n FROM caption_seq WHERE OLD.item_key <> NEW.item_key;
            END;
            CREATE TRIGGER IF NOT EXISTS captions_del AFTER DELETE ON captions BEGIN
                UPDATE caption_seq SET n = n + 1;
                INSERT OR IGNORE INTO caption_removed SELECT OLD.item_key, n FROM caption_seq;
            END;
            PRAGMA user_version = 1;",
        )?;
        tx.commit()
    }

    /// The descriptor owns path normalization for both reads and writes.
    pub fn normalize_path(path: &Path) -> String {
        crate::rename_key_migration::STORES
            .iter()
            .find(|store| store.file == "ai_caption.db")
            .expect("caption store descriptor")
            .normalize_path(path)
    }

    /// Read by item key. Physical paths may be passed without pre-normalizing.
    pub fn get(&self, item_key: &str) -> rusqlite::Result<Option<CaptionRow>> {
        let key = Self::normalize_path(Path::new(item_key));
        self.conn
            .query_row(
                "SELECT item_key, mtime_ms, size, model_id, prompt_version, lang,
                caption, visible_text, keywords, cover_ref, status, created_at, rev, indexed
             FROM captions WHERE item_key = ?1",
                [key],
                |row| {
                    let keywords: String = row.get(8)?;
                    let keywords = serde_json::from_str(&keywords).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            8,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                    let status: String = row.get(10)?;
                    let status = match status.as_str() {
                        "ok" => CaptionStatus::Ok,
                        "failed" => CaptionStatus::Failed,
                        _ => return Err(rusqlite::Error::InvalidQuery),
                    };
                    Ok(CaptionRow {
                        item_key: row.get(0)?,
                        signature: FileSignature {
                            mtime_ms: row.get(1)?,
                            size: row.get(2)?,
                        },
                        model_id: row.get(3)?,
                        prompt_version: row.get(4)?,
                        lang: row.get(5)?,
                        caption: row.get(6)?,
                        visible_text: row.get(7)?,
                        keywords,
                        cover_ref: row.get(9)?,
                        status,
                        created_at: row.get(11)?,
                        rev: row.get(12)?,
                        indexed: row.get(13)?,
                    })
                },
            )
            .optional()
    }

    /// Read pending changes only inside the caller's existing metadata ownership.
    /// Each interval is half-open and uses the same BINARY keys as OwnedRange.
    pub fn pending_in_ranges(
        &self,
        ranges: &[crate::metadata_ownership::PathRange],
        limit: usize,
    ) -> rusqlite::Result<Vec<(String, i64, bool)>> {
        let mut pending = Vec::new();
        let mut statement = self.conn.prepare(
            "SELECT item_key, rev, removed FROM (
                SELECT item_key, rev, 0 AS removed FROM captions WHERE indexed = 0
                UNION ALL
                SELECT item_key, rev, 1 AS removed FROM caption_removed
             ) WHERE item_key >= ?1 COLLATE BINARY AND item_key < ?2 COLLATE BINARY
             ORDER BY item_key, rev, removed LIMIT ?3",
        )?;
        for range in ranges {
            let remaining = limit.saturating_sub(pending.len());
            if remaining == 0 {
                break;
            }
            let rows = statement.query_map(
                params![
                    range.start,
                    range.end,
                    i64::try_from(remaining).unwrap_or(i64::MAX)
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            for row in rows {
                pending.push(row?);
            }
        }
        Ok(pending)
    }

    /// Acknowledge only the exact revisions whose index commit succeeded.
    /// Content/key updates allocate a new global revision, so stale acks are inert.
    pub fn mark_reflected(&self, changes: &[(String, i64, bool)]) -> rusqlite::Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        {
            let mut indexed =
                tx.prepare("UPDATE captions SET indexed = 1 WHERE item_key = ?1 AND rev = ?2")?;
            let mut removed =
                tx.prepare("DELETE FROM caption_removed WHERE item_key = ?1 AND rev = ?2")?;
            for (key, rev, is_removed) in changes {
                if *is_removed {
                    removed.execute(params![key, rev])?;
                } else {
                    indexed.execute(params![key, rev])?;
                }
            }
        }
        tx.commit()
    }

    /// Startup cleanup uses all favorite roots (including metadata-disabled ones),
    /// never the narrower exclusive owner ranges used for reflection.
    pub fn cleanup_removed_outside_ranges(
        &self,
        ranges: &[crate::metadata_ownership::PathRange],
    ) -> rusqlite::Result<usize> {
        if ranges.is_empty() {
            return self.conn.execute("DELETE FROM caption_removed", []);
        }
        let values = std::iter::repeat_n("(?, ?)", ranges.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "WITH roots(start, end) AS (VALUES {values})
             DELETE FROM caption_removed WHERE NOT EXISTS (
                SELECT 1 FROM roots WHERE item_key >= start COLLATE BINARY
                  AND item_key < end COLLATE BINARY
             )"
        );
        self.conn.execute(
            &sql,
            rusqlite::params_from_iter(ranges.iter().flat_map(|range| [&range.start, &range.end])),
        )
    }

    /// Worker-only: reserve the writer before rechecking the input, so a rename
    /// cannot migrate DB keys between this check and commit. Exactly one
    /// filesystem metadata call is made inside the transaction. DB failures are
    /// returned to V1-3 to stop the job and notify the user, without retrying.
    pub fn save(&self, request: &SaveRequest) -> rusqlite::Result<SaveOutcome> {
        self.save_inner(request, None)
            .map(|result| result.expect("uncancellable save"))
    }

    /// A caption job may be cancelled while SQLite waits for another writer or
    /// for readers to release the rollback journal's commit lock. Recheck at
    /// mutation/commit and abort a busy commit when cancellation arrives. None
    /// means cancellation, distinct from an input signature change (Discarded).
    pub(crate) fn save_unless_cancelled(
        &self,
        request: &SaveRequest,
        cancel: &Arc<AtomicBool>,
    ) -> rusqlite::Result<Option<SaveOutcome>> {
        self.save_inner(request, Some(cancel))
    }

    fn save_inner(
        &self,
        request: &SaveRequest,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> rusqlite::Result<Option<SaveOutcome>> {
        let cancelled = || cancel.is_some_and(|flag| flag.load(Ordering::Acquire));
        if let CaptionResult::Ok {
            caption,
            text,
            keywords,
        } = &request.result
        {
            validate_caption(caption, text, keywords).map_err(|_| rusqlite::Error::InvalidQuery)?;
        }
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        if cancelled() {
            tx.rollback()?;
            return Ok(None);
        }
        let current = std::fs::metadata(&request.signature_path)
            .ok()
            .as_ref()
            .and_then(FileSignature::from_metadata);
        if current != Some(request.signature) {
            tx.rollback()?;
            return Ok(Some(SaveOutcome::Discarded));
        }
        let key = Self::normalize_path(&request.item_path);
        let (status, caption, text, keywords) = match &request.result {
            CaptionResult::Ok {
                caption,
                text,
                keywords,
            } => (
                "ok",
                caption.as_str(),
                text.as_str(),
                serde_json::to_string(keywords).expect("strings serialize"),
            ),
            CaptionResult::Failed => ("failed", "", "", "[]".to_string()),
        };
        if cancelled() {
            tx.rollback()?;
            return Ok(None);
        }
        let changed = tx.execute(
            "INSERT INTO captions (item_key, mtime_ms, size, model_id, prompt_version,
                lang, caption, visible_text, cover_ref, keywords, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(item_key) DO UPDATE SET
                mtime_ms=excluded.mtime_ms, size=excluded.size, model_id=excluded.model_id,
                prompt_version=excluded.prompt_version, lang=excluded.lang,
                caption=excluded.caption, visible_text=excluded.visible_text,
                cover_ref=excluded.cover_ref, keywords=excluded.keywords,
                status=excluded.status, created_at=excluded.created_at
             WHERE excluded.status = 'ok' OR captions.status <> 'ok'",
            params![
                key,
                request.signature.mtime_ms,
                request.signature.size,
                request.model_id,
                request.prompt_version,
                request.lang,
                caption,
                text,
                request.cover_ref,
                keywords,
                status,
                system_time_ms(SystemTime::now()).unwrap_or(0)
            ],
        )?;
        if let Some(cancel) = cancel {
            if !commit_unless_cancelled(&self.conn, tx, cancel, BUSY_TIMEOUT)? {
                return Ok(None);
            }
        } else {
            tx.commit()?;
        }
        Ok(Some(if changed == 0 {
            SaveOutcome::KeptExistingOk
        } else {
            SaveOutcome::Written
        }))
    }

    /// Worker-only destination initialization/purge. The trigger records removal.
    pub fn delete(&self, item_key: &str) -> rusqlite::Result<usize> {
        self.conn.execute(
            "DELETE FROM captions WHERE item_key = ?1",
            [Self::normalize_path(Path::new(item_key))],
        )
    }
}

pub(crate) fn validate_caption(
    caption: &str,
    text: &str,
    keywords: &[String],
) -> Result<(), &'static str> {
    if caption.chars().count() > 400 || text.chars().count() > 400 {
        return Err("AI caption text exceeds 400 characters");
    }
    if keywords.len() > 20 || keywords.iter().any(|word| word.chars().count() > 32) {
        return Err("AI caption keywords exceed 20 words or 32 characters per word");
    }
    Ok(())
}

fn system_time_ms(value: SystemTime) -> Option<i64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(path: &Path) -> SaveRequest {
        SaveRequest {
            item_path: path.to_path_buf(),
            signature_path: path.to_path_buf(),
            signature: FileSignature::from_metadata(&std::fs::metadata(path).unwrap()).unwrap(),
            cover_ref: String::new(),
            model_id: "model@revision".into(),
            prompt_version: 1,
            lang: "ja".into(),
            result: CaptionResult::Ok {
                caption: "青い空".into(),
                text: "表紙の文字".into(),
                keywords: vec!["空".into(), "青".into()],
            },
        }
    }

    fn fixture() -> (tempfile::TempDir, CaptionDb, SaveRequest) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("image.jpg");
        std::fs::write(&path, b"image").unwrap();
        let db = CaptionDb::open_at(temp.path().join("ai_caption.db")).unwrap();
        let req = request(&path);
        (temp, db, req)
    }

    #[test]
    fn caption_job_cancel_while_waiting_for_db_writer_does_not_save() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;

        thread_local! {
            static WAITING: std::cell::RefCell<Option<mpsc::Sender<()>>> = const { std::cell::RefCell::new(None) };
        }
        fn busy(_attempt: i32) -> bool {
            WAITING.with(|slot| {
                if let Some(notify) = slot.borrow_mut().take() {
                    let _ = notify.send(());
                }
            });
            std::thread::sleep(Duration::from_millis(1));
            true
        }
        let (temp, db, req) = fixture();
        let path = temp.path().join("ai_caption.db");
        let tx = Transaction::new_unchecked(&db.conn, TransactionBehavior::Immediate).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (notify, waiting) = mpsc::channel();
        let worker_req = req.clone();
        let worker = std::thread::spawn(move || {
            let db = CaptionDb::open_existing_for_write(path).unwrap();
            WAITING.with(|slot| *slot.borrow_mut() = Some(notify));
            db.conn.busy_handler(Some(busy)).unwrap();
            db.save_unless_cancelled(&worker_req, &worker_cancel)
                .unwrap()
        });
        waiting.recv_timeout(Duration::from_secs(10)).unwrap();
        cancel.store(true, Ordering::Release);
        tx.rollback().unwrap();
        assert_eq!(worker.join().unwrap(), None);
        assert!(read(&db, &req).is_none());
        assert_eq!(seq(&db), 0);
    }

    fn assert_commit_handler_removed(db: &CaptionDb) {
        COMMIT_WAIT.with(|slot| assert!(slot.borrow().is_none()));
        assert_eq!(
            db.conn
                .pragma_query_value::<i64, _>(None, "busy_timeout", |row| row.get(0))
                .unwrap(),
            5000
        );
    }

    #[test]
    fn caption_job_cancel_while_reader_blocks_commit_rolls_back_insert_and_update() {
        use std::sync::mpsc;

        for existing in [false, true] {
            let (temp, db, mut req) = fixture();
            if existing {
                db.save(&req).unwrap();
            }
            let before = read(&db, &req);
            let before_seq = seq(&db);
            req.result = CaptionResult::Ok {
                caption: "cancelled replacement".into(),
                text: String::new(),
                keywords: Vec::new(),
            };
            let reader =
                Transaction::new_unchecked(&db.conn, TransactionBehavior::Deferred).unwrap();
            // BEGIN alone does not acquire a shared lock. A real read holds
            // the rollback journal's reader lock until this transaction ends.
            reader
                .query_row("SELECT n FROM caption_seq", [], |row| row.get::<_, i64>(0))
                .unwrap();
            let path = temp.path().join("ai_caption.db");
            let worker_req = req.clone();
            let cancel = Arc::new(AtomicBool::new(false));
            let worker_cancel = Arc::clone(&cancel);
            let (notify, waiting) = mpsc::channel();
            let (finished_tx, finished_rx) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let saving = CaptionDb::open_existing_for_write(path).unwrap();
                COMMIT_WAIT_NOTIFY.with(|slot| *slot.borrow_mut() = Some(notify));
                let result = saving.save_unless_cancelled(&worker_req, &worker_cancel);
                assert_commit_handler_removed(&saving);
                finished_tx.send(result).unwrap();
            });
            waiting.recv_timeout(Duration::from_secs(10)).unwrap();
            cancel.store(true, Ordering::Release);
            // Keep the reader locked until the save has cancelled. Releasing
            // it here would let an uncancellable COMMIT pass this regression.
            assert_eq!(
                finished_rx
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap()
                    .unwrap(),
                None
            );
            worker.join().unwrap();
            reader.rollback().unwrap();
            assert_eq!(read(&db, &req), before);
            assert_eq!(seq(&db), before_seq);
        }
    }

    #[test]
    fn caption_job_commit_succeeds_when_reader_releases_without_cancellation() {
        use std::sync::mpsc;

        let (temp, db, req) = fixture();
        let reader = Transaction::new_unchecked(&db.conn, TransactionBehavior::Deferred).unwrap();
        reader
            .query_row("SELECT n FROM caption_seq", [], |row| row.get::<_, i64>(0))
            .unwrap();
        let path = temp.path().join("ai_caption.db");
        let worker_req = req.clone();
        let (notify, waiting) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let saving = CaptionDb::open_existing_for_write(path).unwrap();
            COMMIT_WAIT_NOTIFY.with(|slot| *slot.borrow_mut() = Some(notify));
            let result =
                saving.save_unless_cancelled(&worker_req, &Arc::new(AtomicBool::new(false)));
            assert_commit_handler_removed(&saving);
            result
        });
        waiting.recv_timeout(Duration::from_secs(10)).unwrap();
        reader.rollback().unwrap();
        assert_eq!(worker.join().unwrap().unwrap(), Some(SaveOutcome::Written));
        assert_eq!(read(&db, &req).unwrap().caption, "青い空");
    }

    #[test]
    fn uncancelled_commit_busy_deadline_is_an_error_and_restores_the_normal_timeout() {
        let (temp, db, req) = fixture();
        db.save(&req).unwrap();
        let before = read(&db, &req).unwrap();
        let before_seq = seq(&db);
        let reader = Connection::open(temp.path().join("ai_caption.db")).unwrap();
        let reader = Transaction::new_unchecked(&reader, TransactionBehavior::Deferred).unwrap();
        reader
            .query_row("SELECT n FROM caption_seq", [], |row| row.get::<_, i64>(0))
            .unwrap();
        let writer = Transaction::new_unchecked(&db.conn, TransactionBehavior::Immediate).unwrap();
        writer.execute("DELETE FROM captions", []).unwrap();
        // Exercise an already-expired deadline without a real five-second
        // test wait. SQLite still attempts COMMIT and calls the same handler.
        let result = commit_unless_cancelled(
            &db.conn,
            writer,
            &Arc::new(AtomicBool::new(false)),
            Duration::ZERO,
        );
        assert_eq!(
            result.unwrap_err().sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy)
        );
        assert_commit_handler_removed(&db);
        reader.rollback().unwrap();
        assert_eq!(read(&db, &req), Some(before));
        assert_eq!(seq(&db), before_seq);
    }

    fn read(db: &CaptionDb, req: &SaveRequest) -> Option<CaptionRow> {
        db.get(req.item_path.to_str().unwrap()).unwrap()
    }

    fn seq(db: &CaptionDb) -> i64 {
        db.conn
            .query_row("SELECT n FROM caption_seq", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn schema_reopen_and_existing_write_keep_rollback_journal() {
        let (temp, db, req) = fixture();
        assert_eq!(
            db.conn
                .pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
                .unwrap(),
            1
        );
        assert_eq!(
            db.conn
                .pragma_query_value::<String, _>(None, "journal_mode", |r| r.get(0))
                .unwrap(),
            "delete"
        );
        assert_eq!(
            db.conn
                .pragma_query_value::<i64, _>(None, "busy_timeout", |r| r.get(0))
                .unwrap(),
            5000
        );
        assert_eq!(db.save(&req).unwrap(), SaveOutcome::Written);
        let before = read(&db, &req).unwrap();
        drop(db);
        let path = temp.path().join("ai_caption.db");
        let reopened = CaptionDb::open_at(&path).unwrap();
        assert_eq!(read(&reopened, &req), Some(before.clone()));
        assert_eq!(seq(&reopened), before.rev);
        let existing = CaptionDb::open_existing_for_write(&path).unwrap();
        assert_eq!(read(&existing, &req), Some(before));
        let absent = temp.path().join("absent.db");
        assert!(CaptionDb::open_existing_for_write(&absent).is_err());
        assert!(!absent.exists());
    }

    #[test]
    fn read_only_missing_store_does_not_create_file_or_parent() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("missing");
        let path = parent.join("ai_caption.db");
        assert!(CaptionDb::open_read_only(&path).unwrap().is_none());
        assert!(!path.exists());
        assert!(!parent.exists());
    }

    #[test]
    fn read_only_uninitialized_stores_are_unpublished_and_untouched() {
        let temp = tempfile::tempdir().unwrap();
        for sqlite_header in [false, true] {
            let path = temp
                .path()
                .join(format!("uninitialized-{sqlite_header}.db"));
            if sqlite_header {
                let conn = Connection::open(&path).unwrap();
                conn.execute_batch("PRAGMA user_version = 0;").unwrap();
                let tables: i64 = conn
                    .query_row(
                        "SELECT count(*) FROM sqlite_master WHERE type='table'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(tables, 0);
            } else {
                std::fs::write(&path, []).unwrap();
            }
            let before = std::fs::read(&path).unwrap();
            assert!(CaptionDb::open_read_only(&path).unwrap().is_none());
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert!(!path.with_extension("db-journal").exists());

            // The same file becomes visible after the real initializer commits.
            let writer = CaptionDb::open_at(&path).unwrap();
            let reader = CaptionDb::open_read_only(&path).unwrap().unwrap();
            assert!(reader.get("c:/missing.jpg").unwrap().is_none());
            drop(reader);
            drop(writer);
        }
    }

    #[test]
    fn read_only_does_not_publish_an_uncommitted_schema_version() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("initializing.db");
        let conn = Connection::open(&path).unwrap();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).unwrap();
        tx.execute_batch("CREATE TABLE initializing (value TEXT); PRAGMA user_version = 1;")
            .unwrap();
        // A separate connection sees the committed version zero, without a retry/wait.
        assert!(CaptionDb::open_read_only(&path).unwrap().is_none());
        tx.rollback().unwrap();
        CaptionDb::ensure_schema(&conn).unwrap();
        assert!(
            CaptionDb::open_read_only(&path)
                .unwrap()
                .unwrap()
                .get("c:/missing.jpg")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn read_only_existing_store_reads_without_schema_changes_or_writes() {
        let (temp, db, req) = fixture();
        db.save(&req).unwrap();
        let path = temp.path().join("ai_caption.db");
        let expected = read(&db, &req);
        let reader = CaptionDb::open_read_only(&path).unwrap().unwrap();
        assert_eq!(read(&reader, &req), expected);
        assert!(reader.delete(req.item_path.to_str().unwrap()).is_err());
        assert_eq!(read(&db, &req), expected);
        assert!(reader.conn.is_autocommit());
        drop(reader);
        drop(db);

        let unknown = temp.path().join("unknown-version.db");
        let conn = Connection::open(&unknown).unwrap();
        conn.execute_batch("CREATE TABLE unrelated (value TEXT); PRAGMA user_version=27;")
            .unwrap();
        drop(conn);
        let before = std::fs::read(&unknown).unwrap();
        assert!(matches!(
            CaptionDb::open_read_only(&unknown),
            Err(rusqlite::Error::InvalidQuery)
        ));
        assert_eq!(std::fs::read(&unknown).unwrap(), before);
        let reader =
            Connection::open_with_flags(&unknown, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert_eq!(
            reader
                .query_row::<i64, _, _>(
                    "SELECT count(*) FROM sqlite_master WHERE name='captions'",
                    [],
                    |row| row.get(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            reader
                .pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))
                .unwrap(),
            27
        );
    }

    #[test]
    fn save_reads_signature_path_and_preserves_all_result_fields() {
        let (temp, db, mut req) = fixture();
        // A folder book stores its row at the folder, checks the actual cover file.
        req.item_path = temp.path().join("book");
        req.cover_ref = "chapter/cover.jpg".into();
        assert_eq!(db.save(&req).unwrap(), SaveOutcome::Written);
        let row = read(&db, &req).unwrap();
        assert_eq!(row.signature, req.signature);
        assert_eq!(row.cover_ref, req.cover_ref);
        assert_eq!(row.model_id, req.model_id);
        assert_eq!(row.prompt_version, req.prompt_version);
        assert_eq!(row.lang, req.lang);
        assert_eq!(row.caption, "青い空");
        assert_eq!(row.visible_text, "表紙の文字");
        assert_eq!(row.keywords, ["空", "青"]);
        assert_eq!(row.status, CaptionStatus::Ok);
        assert!(row.created_at > 0 && row.rev > 0);
        assert!(!row.indexed);
        let variant = req
            .item_path
            .to_str()
            .unwrap()
            .to_uppercase()
            .replace('\\', "/");
        assert_eq!(db.get(&variant).unwrap(), Some(row));
    }

    #[test]
    fn save_discards_missing_size_changed_mtime_changed_and_unreadable_path() {
        let (_temp, db, mut req) = fixture();
        // A discarded attempt must not mutate an existing row or its trigger state.
        db.save(&req).unwrap();
        let original = read(&db, &req).unwrap();
        let actual = req.signature;
        req.signature.size += 1;
        assert_eq!(db.save(&req).unwrap(), SaveOutcome::Discarded);
        req.signature = actual;
        req.signature.mtime_ms += 1;
        assert_eq!(db.save(&req).unwrap(), SaveOutcome::Discarded);
        req.signature = actual;
        std::fs::File::options()
            .write(true)
            .open(&req.signature_path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(100)),
            )
            .unwrap();
        assert_eq!(db.save(&req).unwrap(), SaveOutcome::Discarded);
        std::fs::remove_file(&req.signature_path).unwrap();
        assert_eq!(db.save(&req).unwrap(), SaveOutcome::Discarded);
        // A path beneath a regular file cannot be stat'ed (not just NotFound).
        std::fs::write(&req.signature_path, b"image").unwrap();
        req.signature_path = req.signature_path.join("child");
        assert_eq!(db.save(&req).unwrap(), SaveOutcome::Discarded);
        assert_eq!(read(&db, &req), Some(original.clone()));
        assert_eq!(seq(&db), original.rev);
        assert!(system_time_ms(UNIX_EPOCH - Duration::from_secs(1)).is_none());
    }

    #[test]
    fn save_ok_and_failed_follow_overwrite_policy() {
        let (_temp, db, mut req) = fixture();
        let success = req.result.clone();
        req.result = CaptionResult::Failed;
        assert_eq!(db.save(&req).unwrap(), SaveOutcome::Written);
        let failed = read(&db, &req).unwrap();
        assert_eq!(failed.status, CaptionStatus::Failed);
        assert!(
            failed.caption.is_empty()
                && failed.visible_text.is_empty()
                && failed.keywords.is_empty()
        );
        req.model_id = "new model".into();
        assert_eq!(db.save(&req).unwrap(), SaveOutcome::Written);
        let updated_failed = read(&db, &req).unwrap();
        assert!(updated_failed.rev > failed.rev);
        assert_eq!(updated_failed.model_id, req.model_id);
        req.result = success.clone();
        assert_eq!(db.save(&req).unwrap(), SaveOutcome::Written);
        let ok = read(&db, &req).unwrap();
        assert_eq!(ok.status, CaptionStatus::Ok);
        req.result = CaptionResult::Failed;
        req.model_id = "failed model must not replace ok".into();
        assert_eq!(db.save(&req).unwrap(), SaveOutcome::KeptExistingOk);
        assert_eq!(read(&db, &req), Some(ok.clone()));
        assert_eq!(seq(&db), ok.rev);
        req.result = success;
        assert_eq!(db.save(&req).unwrap(), SaveOutcome::Written);
        assert!(read(&db, &req).unwrap().rev > ok.rev);
        assert_eq!(
            db.conn
                .query_row::<i64, _, _>("SELECT count(*) FROM caption_removed", [], |r| r.get(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn triggers_allocate_global_revisions_and_ignore_acknowledgement_columns() {
        let (_temp, db, req) = fixture();
        db.save(&req).unwrap();
        let initial = read(&db, &req).unwrap();
        assert_eq!(initial.rev, 1);
        db.conn
            .execute("UPDATE captions SET indexed=1", [])
            .unwrap();
        assert_eq!(seq(&db), 1);
        db.conn
            .execute("UPDATE captions SET caption='new text'", [])
            .unwrap();
        let updated = read(&db, &req).unwrap();
        assert_eq!(updated.rev, 2);
        assert!(!updated.indexed);
        db.conn
            .execute("UPDATE captions SET rev=100, indexed=1", [])
            .unwrap();
        assert_eq!(seq(&db), 2);
        let other = CaptionDb::normalize_path(Path::new(r"C:\other.jpg"));
        db.conn
            .execute("UPDATE captions SET item_key=?1", [&other])
            .unwrap();
        let moved = db.get(&other).unwrap().unwrap();
        assert_eq!(moved.rev, 3);
        assert!(!moved.indexed);
        let old_key = CaptionDb::normalize_path(&req.item_path);
        assert_eq!(
            db.conn
                .query_row::<i64, _, _>(
                    "SELECT rev FROM caption_removed WHERE item_key=?1",
                    [old_key],
                    |r| r.get(0)
                )
                .unwrap(),
            3
        );
        assert_eq!(db.delete(&other).unwrap(), 1);
        assert_eq!(seq(&db), 4);
        assert_eq!(
            db.conn
                .query_row::<i64, _, _>(
                    "SELECT rev FROM caption_removed WHERE item_key=?1",
                    [other],
                    |r| r.get(0)
                )
                .unwrap(),
            4
        );
    }

    #[test]
    fn copied_indexed_row_is_a_new_pending_revision() {
        let (_temp, db, req) = fixture();
        db.save(&req).unwrap();
        db.conn
            .execute("UPDATE captions SET indexed=1", [])
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO captions SELECT 'c:/copy.jpg', mtime_ms, size,
            model_id, prompt_version, lang, caption, visible_text, cover_ref, keywords,
            status, created_at, rev, indexed FROM captions",
                [],
            )
            .unwrap();
        let copied = db.get("c:/copy.jpg").unwrap().unwrap();
        assert_eq!(copied.rev, 2);
        assert!(!copied.indexed);
        assert!(read(&db, &req).unwrap().indexed);
    }

    fn seed_caption_key(db: &CaptionDb, key: &str) {
        db.conn
            .execute(
                "INSERT INTO captions SELECT ?1, mtime_ms, size, model_id, prompt_version,
             lang, caption, visible_text, cover_ref, keywords, status, created_at, rev, indexed
             FROM captions LIMIT 1",
                [key],
            )
            .unwrap();
    }

    #[test]
    fn pending_caption_ranges_respect_ownership_boundaries_and_limit() {
        let (_temp, db, req) = fixture();
        db.save(&req).unwrap();
        for key in [
            "c:/fav",
            "c:/fav/a.jpg",
            "c:/fav/child",
            "c:/fav/child/a.jpg",
            "c:/fav/excluded/a.jpg",
            "c:/favorite/a.jpg",
            "c:/outside/a.jpg",
            "c:/fav/removed.jpg",
            "c:/fav/child/removed.jpg",
        ] {
            seed_caption_key(&db, key);
        }
        db.delete("c:/fav/removed.jpg").unwrap();
        db.delete("c:/fav/child/removed.jpg").unwrap();
        let scope = crate::metadata_ownership::OwnedRange {
            root: "c:/fav".into(),
            exclusions: vec!["c:/fav/child".into(), "c:/fav/excluded".into()],
        };
        let pending = db.pending_in_ranges(&scope.sql_ranges(), 100).unwrap();
        let keys: std::collections::BTreeSet<_> = pending
            .iter()
            .map(|(key, _, removed)| (key.as_str(), *removed))
            .collect();
        assert_eq!(
            keys,
            std::collections::BTreeSet::from([
                ("c:/fav", false),
                ("c:/fav/a.jpg", false),
                ("c:/fav/removed.jpg", true),
            ])
        );
        assert_eq!(
            db.pending_in_ranges(&scope.sql_ranges(), 1).unwrap().len(),
            1
        );
        assert!(
            db.pending_in_ranges(&scope.sql_ranges(), 0)
                .unwrap()
                .is_empty()
        );
        assert!(db.pending_in_ranges(&[], 100).unwrap().is_empty());
        db.mark_reflected(&pending).unwrap();
        assert!(
            db.pending_in_ranges(&scope.sql_ranges(), 100)
                .unwrap()
                .is_empty()
        );
        assert!(!db.get("c:/fav/child/a.jpg").unwrap().unwrap().indexed);
    }

    #[test]
    fn caption_acknowledgements_preserve_new_content_and_new_tombstones() {
        let (_temp, db, req) = fixture();
        db.save(&req).unwrap();
        let key = CaptionDb::normalize_path(&req.item_path);
        let ranges = crate::metadata_ownership::OwnedRange {
            root: key.clone(),
            exclusions: Vec::new(),
        }
        .sql_ranges();
        let old = db.pending_in_ranges(&ranges, 100).unwrap();
        db.conn
            .execute("UPDATE captions SET caption='replacement'", [])
            .unwrap();
        db.mark_reflected(&old).unwrap();
        assert!(!read(&db, &req).unwrap().indexed);
        let updated = db.pending_in_ranges(&ranges, 100).unwrap();
        assert_ne!(old, updated);
        db.mark_reflected(&updated).unwrap();
        assert!(read(&db, &req).unwrap().indexed);
        db.delete(&key).unwrap();
        let removed = db.pending_in_ranges(&ranges, 100).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(removed[0].2);
        db.save(&req).unwrap();
        db.delete(&key).unwrap();
        db.mark_reflected(&removed).unwrap();
        let latest = db.pending_in_ranges(&ranges, 100).unwrap();
        assert_eq!(latest.len(), 1);
        assert!(latest[0].2 && latest[0].1 > removed[0].1);
        db.mark_reflected(&latest).unwrap();
        assert!(db.pending_in_ranges(&ranges, 100).unwrap().is_empty());
    }

    #[test]
    fn caption_tombstone_cleanup_preserves_all_favorite_root_ranges() {
        let (_temp, db, req) = fixture();
        db.save(&req).unwrap();
        for key in [
            "c:/fav",
            "c:/fav/child/a.jpg",
            "c:/disabled/a.jpg",
            "c:/favorite/a.jpg",
            "c:/temp",
        ] {
            seed_caption_key(&db, key);
            db.delete(key).unwrap();
        }
        let ranges: Vec<_> = ["c:/fav", "c:/disabled"]
            .into_iter()
            .flat_map(|root| {
                crate::metadata_ownership::OwnedRange {
                    root: root.into(),
                    exclusions: Vec::new(),
                }
                .sql_ranges()
            })
            .collect();
        assert_eq!(db.cleanup_removed_outside_ranges(&ranges).unwrap(), 2);
        assert_eq!(db.pending_in_ranges(&ranges, 100).unwrap().len(), 3);
        assert!(read(&db, &req).is_some());
        assert_eq!(db.cleanup_removed_outside_ranges(&[]).unwrap(), 3);
        assert!(read(&db, &req).is_some());
    }

    thread_local! {
        static BUSY_FENCE: std::cell::RefCell<Option<(
            std::sync::mpsc::SyncSender<()>, std::sync::mpsc::Receiver<()>
        )>> = const { std::cell::RefCell::new(None) };
    }

    fn wait_for_renamer(_: i32) -> bool {
        BUSY_FENCE.with(|fence| {
            if let Some((entered, committed)) = fence.borrow_mut().take() {
                entered.send(()).unwrap();
                committed.recv_timeout(Duration::from_secs(5)).unwrap();
                true
            } else {
                false
            }
        })
    }

    #[test]
    fn saver_waits_for_writer_then_rechecks_file_after_rename_commit() {
        let (temp, db, req) = fixture();
        db.save(&req).unwrap();
        let saving = CaptionDb::open_existing_for_write(temp.path().join("ai_caption.db")).unwrap();
        let lock = Transaction::new_unchecked(&db.conn, TransactionBehavior::Immediate).unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(0);
        let (committed_tx, committed_rx) = std::sync::mpsc::sync_channel(0);
        let old = req.item_path.clone();
        let worker = std::thread::spawn(move || {
            BUSY_FENCE.with(|fence| *fence.borrow_mut() = Some((entered_tx, committed_rx)));
            saving.conn.busy_handler(Some(wait_for_renamer)).unwrap();
            saving.save(&req)
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let new = temp.path().join("renamed.jpg");
        std::fs::rename(&old, &new).unwrap();
        let new_key = CaptionDb::normalize_path(&new);
        lock.execute("UPDATE captions SET item_key=?1", [&new_key])
            .unwrap();
        lock.commit().unwrap();
        committed_tx.send(()).unwrap();
        assert_eq!(worker.join().unwrap().unwrap(), SaveOutcome::Discarded);
        assert!(db.get(old.to_str().unwrap()).unwrap().is_none());
        assert!(db.get(&new_key).unwrap().is_some());
        assert_eq!(seq(&db), 2);
    }

    #[test]
    fn busy_failure_is_returned_without_mutation() {
        let (temp, db, req) = fixture();
        let saving = CaptionDb::open_existing_for_write(temp.path().join("ai_caption.db")).unwrap();
        saving.conn.busy_timeout(Duration::from_millis(1)).unwrap();
        let lock = Transaction::new_unchecked(&db.conn, TransactionBehavior::Immediate).unwrap();
        assert!(saving.save(&req).is_err());
        lock.rollback().unwrap();
        assert!(read(&db, &req).is_none());
        assert_eq!(seq(&db), 0);
    }

    #[test]
    fn caption_limits_count_unicode_characters() {
        assert!(
            validate_caption(
                &"あ".repeat(400),
                &"字".repeat(400),
                &vec!["語".repeat(32); 20]
            )
            .is_ok()
        );
        assert!(validate_caption(&"あ".repeat(401), "", &[]).is_err());
        assert!(validate_caption("", &"字".repeat(401), &[]).is_err());
        assert!(validate_caption("", "", &vec![String::new(); 21]).is_err());
        assert!(validate_caption("", "", &["語".repeat(33)]).is_err());
    }
}
