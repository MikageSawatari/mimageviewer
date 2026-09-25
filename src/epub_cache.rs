//! Immutable EPUB → PDF generations and the process-lifetime deletion gate.
//! All database operations here are for startup or background workers, never egui update.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceState {
    pub size: u64,
    /// Windows FILETIME 100 ns ticks, without conversion through Unix nanoseconds.
    pub mtime_ticks: u64,
}

pub fn source_state(metadata: &fs::Metadata) -> SourceState {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        SourceState {
            size: metadata.file_size(),
            mtime_ticks: metadata.last_write_time(),
        }
    }
    #[cfg(not(windows))]
    {
        const EPOCH_DIFF: u64 = 116_444_736_000_000_000;
        let ticks = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| EPOCH_DIFF + duration.as_nanos() as u64 / 100);
        SourceState {
            size: metadata.len(),
            mtime_ticks: ticks,
        }
    }
}

/// The handle remains alive through `publish`'s commit. Fakes can supply states in tests.
pub trait SourceGuard {
    fn state(&self) -> io::Result<SourceState>;
}

pub struct WriteDenyingSource(File);

impl WriteDenyingSource {
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            options.share_mode(0x0000_0001); // FILE_SHARE_READ only
        }
        options.open(path).map(Self)
    }

    pub fn file(&mut self) -> &mut File {
        &mut self.0
    }
}

impl SourceGuard for WriteDenyingSource {
    fn state(&self) -> io::Result<SourceState> {
        self.0.metadata().map(|metadata| source_state(&metadata))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationRow {
    pub generation_id: i64,
    pub src_path_key: String,
    pub src_path: PathBuf,
    pub src_state: SourceState,
    pub src_sha256: String,
    pub src_head_hash: String,
    pub pdf_file: PathBuf,
    pub pdf_size: u64,
    pub page_count: u32,
    pub direction: String,
    pub profile: String,
    pub created_at: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PublishOutcome {
    Published,
    Adopted(Box<GenerationRow>),
    Stale,
}

#[derive(Debug)]
pub enum CacheError {
    Io(io::Error),
    Sql(rusqlite::Error),
    UnsafePath(PathBuf),
    InvalidState(&'static str),
}

impl From<io::Error> for CacheError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<rusqlite::Error> for CacheError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sql(value)
    }
}

pub fn src_key(path: &Path) -> String {
    crate::path_key::normalize(path)
}

pub fn generation_file(data_dir: &Path, source: &Path, id: i64) -> PathBuf {
    let data_dir = fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_owned());
    let hash = format!("{:x}", Sha256::digest(src_key(source).as_bytes()));
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("book");
    // A Windows filename is one component; a hostile/malformed stem must not add components.
    let stem = stem.replace(['/', '\\', ':'], "_");
    data_dir
        .join("epub_cache")
        .join(&hash[..2])
        .join(&hash)
        .join(format!("{stem}.g{id}.pdf"))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub struct EpubCache {
    conn: Connection,
    data_dir: PathBuf,
}

impl EpubCache {
    #[cfg(test)]
    pub(crate) fn conn_for_tests_retired_count(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM retired", [], |row| row.get(0))
            .unwrap()
    }
    pub fn open_at(data_dir: &Path) -> Result<Self, CacheError> {
        fs::create_dir_all(data_dir)?;
        let data_dir = fs::canonicalize(data_dir)?;
        let conn = Connection::open(data_dir.join("epub_cache.db"))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS generation_ids (
                generation_id INTEGER PRIMARY KEY AUTOINCREMENT, reserved_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS generations (
                generation_id INTEGER PRIMARY KEY, src_path_key TEXT NOT NULL,
                src_path TEXT NOT NULL, src_size INTEGER NOT NULL, src_mtime_ticks INTEGER NOT NULL,
                src_sha256 TEXT NOT NULL, src_head_hash TEXT NOT NULL, pdf_file TEXT NOT NULL,
                pdf_size INTEGER NOT NULL, page_count INTEGER NOT NULL, direction TEXT NOT NULL,
                profile TEXT NOT NULL, created_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS current (
                src_path_key TEXT PRIMARY KEY, generation_id INTEGER NOT NULL,
                last_access_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS retired (
                generation_id INTEGER PRIMARY KEY, retired_at INTEGER NOT NULL);",
        )?;
        validate_schema(&conn)?;
        Ok(Self { conn, data_dir })
    }

    pub fn reserve_generation_id(&mut self) -> Result<i64, CacheError> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO generation_ids (reserved_at) VALUES (?1)",
            [now()],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    pub fn publish<G: SourceGuard>(
        &mut self,
        candidate: &GenerationRow,
        guard: &G,
    ) -> Result<PublishOutcome, CacheError> {
        if candidate.src_path_key != src_key(&candidate.src_path)
            || candidate.pdf_file
                != generation_file(&self.data_dir, &candidate.src_path, candidate.generation_id)
        {
            return Err(CacheError::InvalidState("candidate path or file"));
        }
        validate_payload_file(&self.data_dir.join("epub_cache"), &candidate.pdf_file)?;
        if !candidate.pdf_file.is_file() {
            return Err(CacheError::InvalidState("candidate file missing"));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO generations VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                candidate.generation_id,
                candidate.src_path_key,
                candidate.src_path.to_string_lossy(),
                candidate.src_state.size as i64,
                candidate.src_state.mtime_ticks as i64,
                candidate.src_sha256,
                candidate.src_head_hash,
                candidate.pdf_file.to_string_lossy(),
                candidate.pdf_size as i64,
                candidate.page_count,
                candidate.direction,
                candidate.profile,
                candidate.created_at
            ],
        )?;
        let previous_id: Option<i64> = tx
            .query_row(
                "SELECT generation_id FROM current WHERE src_path_key=?1",
                [&candidate.src_path_key],
                |row| row.get(0),
            )
            .optional()?;
        let existing = if let Some(id) = previous_id {
            generation_in(&tx, id)?
        } else {
            None
        };
        let outcome = if guard.state()? != candidate.src_state {
            tx.execute(
                "INSERT OR IGNORE INTO retired VALUES (?1,?2)",
                params![candidate.generation_id, now()],
            )?;
            PublishOutcome::Stale
        } else if let Some(existing) = existing {
            if existing.src_state == candidate.src_state && existing.pdf_file.is_file() {
                tx.execute(
                    "INSERT OR IGNORE INTO retired VALUES (?1,?2)",
                    params![candidate.generation_id, now()],
                )?;
                PublishOutcome::Adopted(Box::new(existing))
            } else {
                publish_as_current(&tx, candidate, previous_id)?;
                PublishOutcome::Published
            }
        } else {
            publish_as_current(&tx, candidate, previous_id)?;
            PublishOutcome::Published
        };
        tx.commit()?;
        Ok(outcome)
    }

    pub fn retire_current(&mut self, key: &str) -> Result<(), CacheError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT OR IGNORE INTO retired SELECT generation_id, ?2 FROM current WHERE src_path_key=?1",
            params![key, now()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn detach_missing(&mut self, key: &str, id: i64) -> Result<(), CacheError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = tx.execute(
            "DELETE FROM current WHERE src_path_key=?1 AND generation_id=?2",
            params![key, id],
        )?;
        if removed != 0 {
            tx.execute(
                "INSERT OR IGNORE INTO retired VALUES (?1,?2)",
                params![id, now()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn current_generation(&self, key: &str) -> Result<Option<GenerationRow>, CacheError> {
        self.conn.query_row("SELECT g.* FROM current c JOIN generations g ON g.generation_id=c.generation_id WHERE c.src_path_key=?1",
            [key], decode_generation).optional().map_err(Into::into)
    }

    pub fn generation(&self, id: i64) -> Result<Option<GenerationRow>, CacheError> {
        generation_in(&self.conn, id).map_err(Into::into)
    }

    pub fn list_current(&self) -> Result<Vec<(GenerationRow, bool)>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT g.*, r.generation_id IS NOT NULL FROM current c JOIN generations g ON g.generation_id=c.generation_id LEFT JOIN retired r ON r.generation_id=g.generation_id ORDER BY c.last_access_at DESC")?;
        let rows = stmt.query_map([], |row| Ok((decode_generation(row)?, row.get(13)?)))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn collect_retired(&mut self) -> Result<(), CacheError> {
        let retired: Vec<(i64, PathBuf)> = {
            let mut stmt = self.conn.prepare("SELECT g.generation_id,g.pdf_file FROM retired r JOIN generations g USING(generation_id)")?;
            stmt.query_map([], |row| {
                Ok((row.get(0)?, PathBuf::from(row.get::<_, String>(1)?)))
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        for (id, path) in retired {
            validate_retired_file(&self.data_dir.join("epub_cache"), &path)?;
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("DELETE FROM current WHERE generation_id=?1", [id])?;
            tx.execute("DELETE FROM generations WHERE generation_id=?1", [id])?;
            tx.execute("DELETE FROM retired WHERE generation_id=?1", [id])?;
            tx.commit()?;
        }
        Ok(())
    }

    fn collect_orphans(&self) -> Result<(), CacheError> {
        let referenced: HashSet<PathBuf> = {
            let mut stmt = self.conn.prepare("SELECT pdf_file FROM generations")?;
            stmt.query_map([], |row| row.get::<_, String>(0).map(PathBuf::from))?
                .collect::<Result<HashSet<_>, _>>()?
        };
        let root = self.data_dir.join("epub_cache");
        fn visit(root: &Path, dir: &Path, refs: &HashSet<PathBuf>) -> Result<(), CacheError> {
            validate_real_dir(root, dir)?;
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                let meta = fs::symlink_metadata(&path)?;
                if reparse(&meta) {
                    continue;
                }
                if meta.is_dir() {
                    visit(root, &path, refs)?;
                } else if meta.is_file() && orphan_name(&path) && !refs.contains(&path) {
                    validate_payload_file(root, &path)?;
                    fs::remove_file(path)?;
                }
            }
            Ok(())
        }
        visit(&root, &root, &referenced)
    }
}

fn validate_schema(conn: &Connection) -> Result<(), CacheError> {
    for query in [
        "SELECT generation_id,reserved_at FROM generation_ids LIMIT 0",
        "SELECT generation_id,src_path_key,src_path,src_size,src_mtime_ticks,src_sha256,src_head_hash,pdf_file,pdf_size,page_count,direction,profile,created_at FROM generations LIMIT 0",
        "SELECT src_path_key,generation_id,last_access_at FROM current LIMIT 0",
        "SELECT generation_id,retired_at FROM retired LIMIT 0",
    ] {
        conn.prepare(query)?;
    }
    Ok(())
}

fn publish_as_current(
    tx: &rusqlite::Transaction<'_>,
    candidate: &GenerationRow,
    previous: Option<i64>,
) -> Result<(), CacheError> {
    tx.execute("INSERT INTO current VALUES (?1,?2,?3) ON CONFLICT(src_path_key) DO UPDATE SET generation_id=excluded.generation_id,last_access_at=excluded.last_access_at",
        params![candidate.src_path_key, candidate.generation_id, now()])?;
    if let Some(id) = previous {
        tx.execute(
            "INSERT OR IGNORE INTO retired VALUES (?1,?2)",
            params![id, now()],
        )?;
    }
    Ok(())
}

fn decode_generation(row: &rusqlite::Row<'_>) -> rusqlite::Result<GenerationRow> {
    Ok(GenerationRow {
        generation_id: row.get(0)?,
        src_path_key: row.get(1)?,
        src_path: PathBuf::from(row.get::<_, String>(2)?),
        src_state: SourceState {
            size: row.get::<_, i64>(3)? as u64,
            mtime_ticks: row.get::<_, i64>(4)? as u64,
        },
        src_sha256: row.get(5)?,
        src_head_hash: row.get(6)?,
        pdf_file: PathBuf::from(row.get::<_, String>(7)?),
        pdf_size: row.get::<_, i64>(8)? as u64,
        page_count: row.get(9)?,
        direction: row.get(10)?,
        profile: row.get(11)?,
        created_at: row.get(12)?,
    })
}

fn generation_in(conn: &Connection, id: i64) -> rusqlite::Result<Option<GenerationRow>> {
    conn.query_row(
        "SELECT * FROM generations WHERE generation_id=?1",
        [id],
        decode_generation,
    )
    .optional()
}

fn orphan_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let generation = name.ends_with(".pdf")
        && name.rsplit_once(".g").is_some_and(|(_, suffix)| {
            suffix
                .strip_suffix(".pdf")
                .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        });
    generation || name.ends_with(".part") || name.contains(".part.tmp-")
}

fn reparse(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn validate_real_dir(root: &Path, path: &Path) -> Result<(), CacheError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| CacheError::UnsafePath(path.to_owned()))?;
    if !relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
        && path != root
    {
        return Err(CacheError::UnsafePath(path.to_owned()));
    }
    let root_meta = fs::symlink_metadata(root)?;
    if !root_meta.is_dir() || reparse(&root_meta) {
        return Err(CacheError::UnsafePath(root.to_owned()));
    }
    let mut current = root.to_owned();
    for part in relative.components() {
        current.push(part);
        let meta = fs::symlink_metadata(&current)?;
        if !meta.is_dir() || reparse(&meta) {
            return Err(CacheError::UnsafePath(current));
        }
    }
    Ok(())
}

fn validate_payload_file(root: &Path, path: &Path) -> Result<(), CacheError> {
    let parent = path
        .parent()
        .ok_or_else(|| CacheError::UnsafePath(path.to_owned()))?;
    validate_real_dir(root, parent)?;
    if !orphan_name(path) {
        return Err(CacheError::UnsafePath(path.to_owned()));
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !reparse(&meta) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        _ => Err(CacheError::UnsafePath(path.to_owned())),
    }
}

fn validate_retired_file(root: &Path, path: &Path) -> Result<(), CacheError> {
    match fs::symlink_metadata(path) {
        Ok(_) => return validate_payload_file(root, path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| CacheError::UnsafePath(path.to_owned()))?;
    if !orphan_name(path)
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(CacheError::UnsafePath(path.to_owned()));
    }
    validate_real_dir(root, root)?;
    let mut current = root.to_owned();
    for part in relative
        .components()
        .take(relative.components().count().saturating_sub(1))
    {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() && !reparse(&meta) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            _ => return Err(CacheError::UnsafePath(current)),
        }
    }
    Ok(())
}

#[derive(Debug)]
pub enum GateReason {
    Lock(io::Error),
    Schema(CacheError),
    Cleanup(CacheError),
}

pub enum GateOutcome {
    Enabled { guard: AliveGuard, cleaned: bool },
    Disabled(GateReason),
}

pub struct AliveGuard {
    file: File,
    data_dir: PathBuf,
}

impl AliveGuard {
    pub(crate) fn authorizes(&self, data_dir: &Path) -> bool {
        fs::canonicalize(data_dir).is_ok_and(|path| self.data_dir == path)
    }
}

impl Drop for AliveGuard {
    fn drop(&mut self) {
        let _ = unlock(&self.file);
    }
}

#[cfg(windows)]
fn lock(file: &File, exclusive: bool, immediate: bool) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle as _;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };
    use windows::Win32::System::IO::OVERLAPPED;
    let flags = (if exclusive {
        LOCKFILE_EXCLUSIVE_LOCK
    } else {
        Default::default()
    }) | (if immediate {
        LOCKFILE_FAIL_IMMEDIATELY
    } else {
        Default::default()
    });
    let mut overlapped = OVERLAPPED::default();
    unsafe {
        LockFileEx(
            HANDLE(file.as_raw_handle()),
            flags,
            None,
            1,
            0,
            &mut overlapped,
        )
    }
    .map_err(|e| io::Error::other(e.to_string()))
}

#[cfg(windows)]
fn unlock(file: &File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle as _;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::UnlockFileEx;
    use windows::Win32::System::IO::OVERLAPPED;
    let mut overlapped = OVERLAPPED::default();
    unsafe { UnlockFileEx(HANDLE(file.as_raw_handle()), None, 1, 0, &mut overlapped) }
        .map_err(|e| io::Error::other(e.to_string()))
}

#[cfg(not(windows))]
fn lock(_file: &File, _exclusive: bool, _immediate: bool) -> io::Result<()> {
    Ok(())
}
#[cfg(not(windows))]
fn unlock(_file: &File) -> io::Result<()> {
    Ok(())
}

pub fn startup_gate(data_dir: &Path) -> GateOutcome {
    let result = (|| -> Result<(AliveGuard, bool), GateReason> {
        fs::create_dir_all(data_dir).map_err(GateReason::Lock)?;
        let data_dir = fs::canonicalize(data_dir).map_err(GateReason::Lock)?;
        let root = data_dir.join("epub_cache");
        fs::create_dir_all(&root).map_err(GateReason::Lock)?;
        validate_real_dir(&root, &root).map_err(GateReason::Cleanup)?;
        let alive_path = root.join(".alive");
        if let Ok(meta) = fs::symlink_metadata(&alive_path)
            && (!meta.is_file() || reparse(&meta))
        {
            return Err(GateReason::Cleanup(CacheError::UnsafePath(alive_path)));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(alive_path)
            .map_err(GateReason::Lock)?;
        let exclusive = lock(&file, true, true).is_ok();
        if exclusive {
            let cleanup = (|| -> Result<(), GateReason> {
                let mut db = EpubCache::open_at(&data_dir).map_err(GateReason::Schema)?;
                db.collect_retired().map_err(GateReason::Cleanup)?;
                db.collect_orphans().map_err(GateReason::Cleanup)?;
                Ok(())
            })();
            unlock(&file).map_err(GateReason::Lock)?;
            cleanup?;
        }
        lock(&file, false, false).map_err(GateReason::Lock)?;
        if !exclusive {
            // Another process owns this profile: do not initialize its schema or scan files.
            let db_path = data_dir.join("epub_cache.db");
            if !db_path.exists() {
                let _ = unlock(&file);
                return Err(GateReason::Schema(CacheError::InvalidState(
                    "schema missing",
                )));
            }
            let conn =
                Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .map_err(|e| GateReason::Schema(e.into()))?;
            validate_schema(&conn).map_err(GateReason::Schema)?;
        }
        Ok((AliveGuard { file, data_dir }, exclusive))
    })();
    match result {
        Ok((guard, cleaned)) => {
            crate::logger::log(format!(
                "epub_cache: startup gate enabled; cleanup={cleaned}"
            ));
            GateOutcome::Enabled { guard, cleaned }
        }
        Err(reason) => {
            crate::logger::log(format!("epub_cache: startup gate disabled: {reason:?}"));
            GateOutcome::Disabled(reason)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    struct FakeGuard(SourceState);
    impl SourceGuard for FakeGuard {
        fn state(&self) -> io::Result<SourceState> {
            Ok(self.0)
        }
    }

    fn candidate(db: &mut EpubCache, root: &Path, src: &Path, state: SourceState) -> GenerationRow {
        let id = db.reserve_generation_id().unwrap();
        let pdf = generation_file(root, src, id);
        fs::create_dir_all(pdf.parent().unwrap()).unwrap();
        fs::write(&pdf, b"%PDF-1.4\n").unwrap();
        GenerationRow {
            generation_id: id,
            src_path_key: src_key(src),
            src_path: src.to_owned(),
            src_state: state,
            src_sha256: "full".into(),
            src_head_hash: "head".into(),
            pdf_file: pdf,
            pdf_size: 9,
            page_count: 1,
            direction: "rtl".into(),
            profile: "reflow-v1".into(),
            created_at: 1,
        }
    }
    fn state(n: u64) -> SourceState {
        SourceState {
            size: n,
            mtime_ticks: n * 10,
        }
    }
    fn retired(db: &EpubCache, id: i64) -> bool {
        db.conn
            .query_row("SELECT 1 FROM retired WHERE generation_id=?1", [id], |_| {
                Ok(())
            })
            .optional()
            .unwrap()
            .is_some()
    }

    #[test]
    fn epub_cache_i8_stale_inserts_immutable_row_and_retires_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let src = tmp.path().join("book.epub");
        let a = candidate(&mut db, tmp.path(), &src, state(1));
        assert_eq!(
            db.publish(&a, &FakeGuard(state(2))).unwrap(),
            PublishOutcome::Stale
        );
        assert_eq!(db.generation(a.generation_id).unwrap(), Some(a.clone()));
        assert!(retired(&db, a.generation_id));
        assert!(db.current_generation(&a.src_path_key).unwrap().is_none());
    }

    #[test]
    fn epub_cache_i8_adopts_existing_and_retires_loser() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let src = tmp.path().join("book.epub");
        let a = candidate(&mut db, tmp.path(), &src, state(1));
        let b = candidate(&mut db, tmp.path(), &src, state(1));
        assert_eq!(
            db.publish(&a, &FakeGuard(state(1))).unwrap(),
            PublishOutcome::Published
        );
        assert_eq!(
            db.publish(&b, &FakeGuard(state(1))).unwrap(),
            PublishOutcome::Adopted(Box::new(a.clone()))
        );
        assert!(retired(&db, b.generation_id));
        assert_eq!(db.current_generation(&a.src_path_key).unwrap(), Some(a));
    }

    #[test]
    fn epub_cache_i8_missing_or_changed_current_is_replaced_and_retired() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let src = tmp.path().join("book.epub");
        let a = candidate(&mut db, tmp.path(), &src, state(1));
        let b = candidate(&mut db, tmp.path(), &src, state(1));
        db.publish(&a, &FakeGuard(state(1))).unwrap();
        fs::remove_file(&a.pdf_file).unwrap();
        assert_eq!(
            db.publish(&b, &FakeGuard(state(1))).unwrap(),
            PublishOutcome::Published
        );
        assert!(retired(&db, a.generation_id));
        assert_eq!(db.current_generation(&a.src_path_key).unwrap(), Some(b));
    }

    #[test]
    fn epub_cache_mtime_ticks_round_trip_full_u64() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let src = tmp.path().join("book.epub");
        let extreme = SourceState {
            size: 1,
            mtime_ticks: u64::MAX,
        };
        let row = candidate(&mut db, tmp.path(), &src, extreme);
        db.publish(&row, &FakeGuard(extreme)).unwrap();
        assert_eq!(
            db.generation(row.generation_id).unwrap().unwrap().src_state,
            extreme
        );
    }

    #[test]
    fn epub_cache_two_connections_race_first_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_owned();
        let src = root.join("book.epub");
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let (root, src, barrier) = (root.clone(), src.clone(), Arc::clone(&barrier));
                std::thread::spawn(move || {
                    let mut db = EpubCache::open_at(&root).unwrap();
                    let row = candidate(&mut db, &root, &src, state(1));
                    barrier.wait();
                    let result = db.publish(&row, &FakeGuard(state(1))).unwrap();
                    (row, result)
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(
            results
                .iter()
                .filter(|(_, outcome)| matches!(outcome, PublishOutcome::Published))
                .count(),
            1
        );
        let db = EpubCache::open_at(&root).unwrap();
        let current = db.current_generation(&src_key(&src)).unwrap().unwrap();
        for (row, outcome) in results {
            if row.generation_id != current.generation_id {
                assert!(matches!(outcome, PublishOutcome::Adopted(_)));
                assert!(retired(&db, row.generation_id));
            }
        }
    }

    #[test]
    fn epub_cache_retire_does_not_capture_later_publish() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("book.epub");
        let mut a_db = EpubCache::open_at(tmp.path()).unwrap();
        let mut b_db = EpubCache::open_at(tmp.path()).unwrap();
        let a = candidate(&mut a_db, tmp.path(), &src, state(1));
        a_db.publish(&a, &FakeGuard(state(1))).unwrap();
        a_db.retire_current(&a.src_path_key).unwrap();
        let b = candidate(&mut b_db, tmp.path(), &src, state(2));
        assert_eq!(
            b_db.publish(&b, &FakeGuard(state(2))).unwrap(),
            PublishOutcome::Published
        );
        assert!(retired(&b_db, a.generation_id));
        assert!(!retired(&b_db, b.generation_id));
        assert_eq!(a_db.current_generation(&a.src_path_key).unwrap(), Some(b));
    }

    #[test]
    fn epub_cache_detach_only_if_still_current_and_ids_never_reused() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("book.epub");
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let a = candidate(&mut db, tmp.path(), &src, state(1));
        db.publish(&a, &FakeGuard(state(1))).unwrap();
        db.detach_missing(&a.src_path_key, a.generation_id + 1)
            .unwrap();
        assert_eq!(
            db.current_generation(&a.src_path_key).unwrap(),
            Some(a.clone())
        );
        db.detach_missing(&a.src_path_key, a.generation_id).unwrap();
        assert!(db.current_generation(&a.src_path_key).unwrap().is_none());
        assert!(retired(&db, a.generation_id));
        db.conn.execute("DELETE FROM generations", []).unwrap();
        db.conn.execute("DELETE FROM generation_ids", []).unwrap();
        assert!(db.reserve_generation_id().unwrap() > a.generation_id);
    }

    #[test]
    fn epub_cache_gate_keeps_current_generation_across_data_dir_spellings() {
        let tmp = tempfile::tempdir().unwrap();
        let alias = tmp.path().join("epub_cache").join("..");
        fs::create_dir_all(tmp.path().join("epub_cache")).unwrap();
        let src = tmp.path().join("book.epub");
        let mut db = EpubCache::open_at(&alias).unwrap();
        let row = candidate(&mut db, &alias, &src, state(1));
        db.publish(&row, &FakeGuard(state(1))).unwrap();
        drop(db);
        let gate = startup_gate(tmp.path());
        assert!(matches!(gate, GateOutcome::Enabled { cleaned: true, .. }));
        assert!(row.pdf_file.exists());
        assert!(
            EpubCache::open_at(tmp.path())
                .unwrap()
                .current_generation(&row.src_path_key)
                .unwrap()
                .is_some()
        );
    }

    #[cfg(windows)]
    #[test]
    fn epub_cache_gate_skips_cleanup_with_shared_holder_then_cleans() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let src = tmp.path().join("book.epub");
        let row = candidate(&mut db, tmp.path(), &src, state(1));
        db.publish(&row, &FakeGuard(state(1))).unwrap();
        db.retire_current(&row.src_path_key).unwrap();
        let root = tmp.path().join("epub_cache");
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(root.join(".alive"))
            .unwrap();
        lock(&lock_file, false, false).unwrap();
        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join(".alive"))
            .unwrap();
        assert!(
            lock(&second, true, true).is_err(),
            "shared holder prevents exclusive cleanup"
        );
        let gate = startup_gate(tmp.path());
        assert!(matches!(&gate, GateOutcome::Enabled { cleaned: false, .. }));
        assert!(row.pdf_file.exists());
        drop(gate);
        unlock(&lock_file).unwrap();
        let gate = startup_gate(tmp.path());
        assert!(matches!(gate, GateOutcome::Enabled { cleaned: true, .. }));
        assert!(!row.pdf_file.exists());
        assert!(
            EpubCache::open_at(tmp.path())
                .unwrap()
                .generation(row.generation_id)
                .unwrap()
                .is_none()
        );
    }

    #[cfg(windows)]
    #[test]
    fn epub_cache_gate_disables_when_shared_holder_has_no_valid_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("epub_cache");
        fs::create_dir(&root).unwrap();
        let orphan = root.join("orphan.g1.pdf");
        fs::write(&orphan, b"keep").unwrap();
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(root.join(".alive"))
            .unwrap();
        lock(&lock_file, false, false).unwrap();
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Disabled(GateReason::Schema(_))
        ));
        assert!(
            orphan.exists(),
            "skipped gate must not scan or delete payload"
        );
        let conn = Connection::open(tmp.path().join("epub_cache.db")).unwrap();
        conn.execute_batch("CREATE TABLE generation_ids(generation_id); CREATE TABLE generations(generation_id); CREATE TABLE current(src_path_key); CREATE TABLE retired(generation_id);").unwrap();
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Disabled(GateReason::Schema(_))
        ));
        assert!(orphan.exists());
        unlock(&lock_file).unwrap();
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Disabled(GateReason::Schema(_))
        ));
        assert!(orphan.exists());
    }

    #[test]
    fn epub_cache_gate_collects_orphan_and_parts_without_touching_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("epub_cache");
        let nested = root.join("aa").join("bb");
        fs::create_dir_all(&nested).unwrap();
        for name in ["a.g123.pdf", "b.pdf.part", "b.pdf.part.tmp-7-1"] {
            fs::write(nested.join(name), b"junk").unwrap();
        }
        let outside = tmp.path().join("outside.g1.pdf");
        fs::write(&outside, b"keep").unwrap();
        let gate = startup_gate(tmp.path());
        assert!(matches!(gate, GateOutcome::Enabled { .. }));
        assert!(fs::read_dir(&nested).unwrap().next().is_none());
        assert_eq!(fs::read(outside).unwrap(), b"keep");
    }

    #[test]
    fn epub_cache_gate_retires_row_when_payload_parent_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let src = tmp.path().join("book.epub");
        let row = candidate(&mut db, tmp.path(), &src, state(1));
        db.publish(&row, &FakeGuard(state(1))).unwrap();
        db.retire_current(&row.src_path_key).unwrap();
        fs::remove_dir_all(row.pdf_file.parent().unwrap()).unwrap();
        drop(db);
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Enabled { cleaned: true, .. }
        ));
        let db = EpubCache::open_at(tmp.path()).unwrap();
        assert!(db.generation(row.generation_id).unwrap().is_none());
        assert!(db.current_generation(&row.src_path_key).unwrap().is_none());
    }

    #[test]
    fn epub_cache_rejects_outside_or_reparse_deletion() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("epub_cache");
        fs::create_dir(&root).unwrap();
        let outside = tmp.path().join("outside.g1.pdf");
        fs::write(&outside, b"keep").unwrap();
        assert!(validate_payload_file(&root, &outside).is_err());
        #[cfg(windows)]
        {
            let link = root.join("linked");
            if std::os::windows::fs::symlink_dir(tmp.path(), &link).is_ok() {
                assert!(validate_payload_file(&root, &link.join("outside.g1.pdf")).is_err());
            }
        }
        assert_eq!(fs::read(outside).unwrap(), b"keep");
    }
}
