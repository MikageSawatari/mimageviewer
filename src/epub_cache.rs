//! Immutable EPUB → PDF generations and the process-lifetime deletion gate.
//! All database operations here are for startup or background workers, never egui update.
//! A process able to change the user's data directory between our path checks and
//! deletion is out of scope: it could delete these files directly. The checks
//! ensure our own deletes never follow links, including user-created links.

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
    pub output_version: i64,
}

/// Bump whenever the worker's PDF output format changes (metadata, links, or pages).
/// Older generations remain viewable, but must not be copied to a sibling PDF.
pub const CONVERTER_OUTPUT_VERSION: i64 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentGenerationEntry {
    pub generation: GenerationRow,
    pub retired: bool,
    pub last_access_at: i64,
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
    crate::path_key::normalize_keep_drive(path)
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

/// Capability for the `.part` file recorded by a committed generation reservation.
/// Only `EpubCache::reserve_output` can construct it.
#[derive(Debug, PartialEq, Eq)]
pub struct ReservedOutput {
    generation_id: i64,
    final_path: PathBuf,
    part_path: PathBuf,
}

impl ReservedOutput {
    pub fn generation_id(&self) -> i64 {
        self.generation_id
    }

    pub fn final_path(&self) -> &Path {
        &self.final_path
    }

    pub fn part_path(&self) -> &Path {
        &self.part_path
    }
}

#[derive(Debug)]
pub struct ReservedSiblingOutput {
    id: i64,
    destination: PathBuf,
    temp_path: PathBuf,
    token: String,
    ownership: SiblingOwnership,
}

#[derive(Debug)]
enum SiblingOwnership {
    Reserved,
    Created(FileIdentity),
    Unprovable,
}

impl ReservedSiblingOutput {
    pub fn temp_path(&self) -> &Path {
        &self.temp_path
    }

    pub fn was_created(&self) -> bool {
        !matches!(self.ownership, SiblingOwnership::Reserved)
    }

    pub fn create_file(&self) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            // The creator retains read/write/delete access, but other handles may only read.
            options.access_mode(0x8000_0000 | 0x4000_0000 | 0x0001_0000);
            options.share_mode(0x0000_0001);
            // Hidden is best effort: some shares reject it, so retry create_new without it.
            options.attributes(0x0000_0002);
            match options.open(&self.temp_path) {
                Ok(file) => return Ok(file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Err(error),
                Err(_) => options.attributes(0x0000_0080),
            };
        }
        options.open(&self.temp_path)
    }
}

/// Identity read from an open file handle; the name alone never authorizes deletion.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileIdentity(Vec<u8>);

struct RecordedSiblingRow {
    destination: String,
    temp_file: String,
    token: String,
    phase: String,
    identity: Option<Vec<u8>>,
}

fn file_identity(file: &File) -> io::Result<Option<FileIdentity>> {
    #[cfg(windows)]
    {
        use std::mem::size_of;
        use std::os::windows::io::AsRawHandle as _;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{
            FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
        };
        let mut info = FILE_ID_INFO::default();
        unsafe {
            GetFileInformationByHandleEx(
                HANDLE(file.as_raw_handle()),
                FileIdInfo,
                (&raw mut info).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        }
        .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(file_id_info_identity(
            info.VolumeSerialNumber,
            &info.FileId.Identifier,
        ))
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::MetadataExt as _;
        let metadata = file.metadata()?;
        let mut bytes = Vec::with_capacity(16);
        bytes.extend_from_slice(&metadata.dev().to_le_bytes());
        bytes.extend_from_slice(&metadata.ino().to_le_bytes());
        Ok(Some(FileIdentity(bytes)))
    }
}

#[cfg(windows)]
fn file_id_info_identity(volume_serial: u64, file_id: &[u8; 16]) -> Option<FileIdentity> {
    // MS-FSCC: an all-zero 128-bit ID means this filesystem could not provide one.
    if file_id.iter().all(|byte| *byte == 0) {
        return None;
    }
    let mut bytes = Vec::with_capacity(24);
    bytes.extend_from_slice(&volume_serial.to_le_bytes());
    bytes.extend_from_slice(file_id);
    Some(FileIdentity(bytes))
}

fn recorded_identity(bytes: &[u8]) -> Option<FileIdentity> {
    #[cfg(windows)]
    if bytes.len() != 24 || bytes[8..].iter().all(|byte| *byte == 0) {
        return None;
    }
    #[cfg(not(windows))]
    if bytes.len() != 16 {
        return None;
    }
    Some(FileIdentity(bytes.to_vec()))
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
    #[cfg(test)]
    pub(crate) fn outstanding_sibling_count_for_test(&self) -> i64 {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM outstanding_sibling_outputs",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }
    pub fn open_at(data_dir: &Path) -> Result<Self, CacheError> {
        fs::create_dir_all(data_dir)?;
        let data_dir = fs::canonicalize(data_dir)?;
        let conn = Connection::open(data_dir.join("epub_cache.db"))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS generation_ids (
                generation_id INTEGER PRIMARY KEY AUTOINCREMENT, reserved_at INTEGER NOT NULL,
                pdf_file TEXT NOT NULL, closed_at INTEGER);
            CREATE INDEX IF NOT EXISTS generation_ids_pending
                ON generation_ids(generation_id) WHERE closed_at IS NULL;
            CREATE INDEX IF NOT EXISTS generation_ids_closed
                ON generation_ids(generation_id) WHERE closed_at IS NOT NULL;
            CREATE TABLE IF NOT EXISTS generations (
                generation_id INTEGER PRIMARY KEY, src_path_key TEXT NOT NULL,
                src_path TEXT NOT NULL, src_size INTEGER NOT NULL, src_mtime_ticks INTEGER NOT NULL,
                src_sha256 TEXT NOT NULL, src_head_hash TEXT NOT NULL, pdf_file TEXT NOT NULL,
                pdf_size INTEGER NOT NULL, page_count INTEGER NOT NULL, direction TEXT NOT NULL,
                profile TEXT NOT NULL, created_at INTEGER NOT NULL,
                output_version INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS current (
                src_path_key TEXT PRIMARY KEY, generation_id INTEGER NOT NULL,
                last_access_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS retired (
                generation_id INTEGER PRIMARY KEY, retired_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS outstanding_sibling_outputs (
                id INTEGER PRIMARY KEY AUTOINCREMENT, destination TEXT NOT NULL,
                temp_file TEXT NOT NULL, reserved_at INTEGER NOT NULL,
                token TEXT NOT NULL DEFAULT '', phase TEXT NOT NULL DEFAULT 'reserved',
                file_identity BLOB);",
        )?;
        let has_output_version = {
            let mut stmt = conn.prepare("PRAGMA table_info(generations)")?;
            stmt.query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .any(|name| name == "output_version")
        };
        if !has_output_version {
            conn.execute(
                "ALTER TABLE generations ADD COLUMN output_version INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        let sibling_columns = {
            let mut stmt = conn.prepare("PRAGMA table_info(outstanding_sibling_outputs)")?;
            stmt.query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (name, declaration) in [
            ("token", "TEXT NOT NULL DEFAULT ''"),
            ("phase", "TEXT NOT NULL DEFAULT 'reserved'"),
            ("file_identity", "BLOB"),
        ] {
            if !sibling_columns.iter().any(|column| column == name) {
                conn.execute(
                    &format!(
                        "ALTER TABLE outstanding_sibling_outputs ADD COLUMN {name} {declaration}"
                    ),
                    [],
                )?;
            }
        }
        validate_schema(&conn)?;
        Ok(Self { conn, data_dir })
    }

    pub fn reserve_output(&mut self, source: &Path) -> Result<ReservedOutput, CacheError> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO generation_ids (reserved_at,pdf_file) VALUES (?1,'')",
            [now()],
        )?;
        let id = tx.last_insert_rowid();
        let path = generation_file(&self.data_dir, source, id);
        tx.execute(
            "UPDATE generation_ids SET pdf_file=?2 WHERE generation_id=?1",
            params![id, path.to_string_lossy()],
        )?;
        tx.commit()?;
        Ok(ReservedOutput {
            generation_id: id,
            part_path: path.with_extension("pdf.part"),
            final_path: path,
        })
    }

    /// Commit the cleanup record before the destination temp file is created.
    pub fn reserve_sibling_output(
        &mut self,
        destination: &Path,
    ) -> Result<ReservedSiblingOutput, CacheError> {
        let parent = destination
            .parent()
            .ok_or_else(|| CacheError::UnsafePath(destination.to_owned()))?;
        let parent = fs::canonicalize(parent)?;
        if !validate_real_absolute_dir(&parent)? {
            return Err(CacheError::UnsafePath(parent));
        }
        let destination = parent.join(
            destination
                .file_name()
                .ok_or_else(|| CacheError::UnsafePath(destination.to_owned()))?,
        );
        if !destination
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
        {
            return Err(CacheError::UnsafePath(destination));
        }
        // Two independent UUID v4 values provide 244 random bits in the filename.
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let temp_path = parent.join(format!(".miv-part-{token}.tmp"));
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO outstanding_sibling_outputs(destination,temp_file,reserved_at,token,phase) VALUES (?1,?2,?3,?4,'reserved')",
            params![destination.to_string_lossy(), temp_path.to_string_lossy(), now(), token],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(ReservedSiblingOutput {
            id,
            destination,
            temp_path,
            token,
            ownership: SiblingOwnership::Reserved,
        })
    }

    /// Call only after `create_new` succeeds, while its handle remains open.
    pub fn mark_sibling_created(
        &mut self,
        reserved: &mut ReservedSiblingOutput,
        file: &File,
    ) -> Result<(), CacheError> {
        let identity = match file_identity(file) {
            Ok(identity) => identity,
            Err(error) => {
                crate::logger::log(format!("epub sibling file identity unavailable: {error}"));
                None
            }
        };
        self.mark_sibling_created_with_identity(reserved, identity)
    }

    fn mark_sibling_created_with_identity(
        &mut self,
        reserved: &mut ReservedSiblingOutput,
        identity: Option<FileIdentity>,
    ) -> Result<(), CacheError> {
        let phase = if identity.is_some() {
            "created"
        } else {
            "unprovable"
        };
        let updated = self.conn.execute(
            "UPDATE outstanding_sibling_outputs SET phase=?3,file_identity=?4 WHERE id=?1 AND token=?2 AND phase='reserved'",
            params![reserved.id, reserved.token, phase, identity.as_ref().map(|id| &id.0)],
        )?;
        if updated != 1 {
            return Err(CacheError::InvalidState(
                "sibling output reservation missing or changed",
            ));
        }
        reserved.ownership = match identity {
            Some(identity) => SiblingOwnership::Created(identity),
            None => SiblingOwnership::Unprovable,
        };
        Ok(())
    }

    /// A failed `create_new` never grants ownership of an existing path.
    pub fn abandon_sibling_reservation(
        &mut self,
        reserved: &ReservedSiblingOutput,
    ) -> Result<(), CacheError> {
        if reserved.was_created() {
            return Err(CacheError::InvalidState(
                "created sibling cannot be abandoned",
            ));
        }
        self.conn.execute(
            "DELETE FROM outstanding_sibling_outputs WHERE id=?1 AND token=?2 AND phase='reserved'",
            params![reserved.id, reserved.token],
        )?;
        Ok(())
    }

    pub fn finish_sibling_output(
        &mut self,
        reserved: &ReservedSiblingOutput,
    ) -> Result<(), CacheError> {
        #[cfg(test)]
        if FAIL_NEXT_SIBLING_FINISH.with(|flag| flag.replace(false)) {
            return Err(CacheError::Sql(rusqlite::Error::InvalidQuery));
        }
        let recorded: Option<RecordedSiblingRow> = self
            .conn
            .query_row(
                "SELECT destination,temp_file,token,phase,file_identity FROM outstanding_sibling_outputs WHERE id=?1",
                [reserved.id],
                |row| Ok(RecordedSiblingRow {
                    destination: row.get(0)?,
                    temp_file: row.get(1)?,
                    token: row.get(2)?,
                    phase: row.get(3)?,
                    identity: row.get(4)?,
                }),
            )
            .optional()?;
        let Some(recorded) = recorded else {
            return Err(CacheError::InvalidState(
                "sibling output reservation missing or changed",
            ));
        };
        if recorded.destination != reserved.destination.to_string_lossy()
            || recorded.temp_file != reserved.temp_path.to_string_lossy()
            || recorded.token != reserved.token
            || !matches!(
                recorded.phase.as_str(),
                "reserved" | "created" | "unprovable"
            )
        {
            return Err(CacheError::InvalidState(
                "sibling output reservation missing or changed",
            ));
        }
        if recorded.phase == "created" {
            let identity = recorded.identity.ok_or(CacheError::InvalidState(
                "created sibling has no file identity",
            ))?;
            let Some(identity) = recorded_identity(&identity) else {
                // Old/unsupported filesystem IDs prove nothing: leave the random-named file.
                self.conn.execute(
                    "DELETE FROM outstanding_sibling_outputs WHERE id=?1",
                    [reserved.id],
                )?;
                return Ok(());
            };
            if matches!(&reserved.ownership, SiblingOwnership::Created(value) if value != &identity)
            {
                return Err(CacheError::InvalidState("created sibling identity changed"));
            }
            delete_sibling_file(reserved, &identity)?;
        }
        self.conn.execute(
            "DELETE FROM outstanding_sibling_outputs WHERE id=?1",
            [reserved.id],
        )?;
        Ok(())
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
            "INSERT INTO generations VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
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
                candidate.created_at,
                candidate.output_version
            ],
        )?;
        if tx.execute(
            "UPDATE generation_ids SET closed_at=?3 WHERE generation_id=?1 AND pdf_file=?2 AND closed_at IS NULL",
            params![candidate.generation_id, candidate.pdf_file.to_string_lossy(), now()],
        )? != 1 {
            return Err(CacheError::InvalidState("generation reservation missing or closed"));
        }
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
            if existing.src_state == candidate.src_state
                && existing.output_version == candidate.output_version
                && existing.pdf_file.is_file()
            {
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

    /// Retire the generation the user actually selected, never a later publish.
    pub fn retire_generation(&mut self, id: i64) -> Result<bool, CacheError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO retired SELECT generation_id, ?2 FROM current WHERE generation_id=?1",
            params![id, now()],
        )?;
        tx.commit()?;
        Ok(inserted != 0)
    }

    pub fn retire_all_current(&mut self) -> Result<usize, CacheError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO retired SELECT generation_id, ?1 FROM current",
            [now()],
        )?;
        tx.commit()?;
        Ok(inserted)
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

    pub(crate) fn validate_generation_pdf(&self, row: &GenerationRow) -> Result<(), CacheError> {
        validate_payload_file(&self.data_dir.join("epub_cache"), &row.pdf_file)
    }

    pub fn touch_current(&self, key: &str, id: i64) -> Result<(), CacheError> {
        self.conn.execute(
            "UPDATE current SET last_access_at=?3 WHERE src_path_key=?1 AND generation_id=?2",
            params![key, id, now()],
        )?;
        Ok(())
    }

    pub fn generation(&self, id: i64) -> Result<Option<GenerationRow>, CacheError> {
        generation_in(&self.conn, id).map_err(Into::into)
    }

    pub fn list_current(&self) -> Result<Vec<CurrentGenerationEntry>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT g.*, r.generation_id IS NOT NULL, c.last_access_at FROM current c JOIN generations g ON g.generation_id=c.generation_id LEFT JOIN retired r ON r.generation_id=g.generation_id ORDER BY c.last_access_at DESC")?;
        let rows = stmt.query_map([], |row| {
            Ok(CurrentGenerationEntry {
                generation: decode_generation(row)?,
                retired: row.get(14)?,
                last_access_at: row.get(15)?,
            })
        })?;
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
            delete_payload_file(&self.data_dir.join("epub_cache"), &path)?;
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

    fn collect_pending(&mut self) -> Result<(), CacheError> {
        let pending: Vec<(i64, PathBuf)> = {
            let mut stmt = self.conn.prepare("SELECT i.generation_id,i.pdf_file FROM generation_ids i WHERE i.closed_at IS NULL AND NOT EXISTS (SELECT 1 FROM generations g WHERE g.generation_id=i.generation_id)")?;
            stmt.query_map([], |row| {
                Ok((row.get(0)?, PathBuf::from(row.get::<_, String>(1)?)))
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        let root = self.data_dir.join("epub_cache");
        for (id, path) in pending {
            if path.as_os_str().is_empty() {
                return Err(CacheError::InvalidState("reservation path missing"));
            }
            delete_payload_file(&root, &path)?;
            let part = path.with_extension("pdf.part");
            delete_payload_file(&root, &part)?;
            let parent = path
                .parent()
                .ok_or_else(|| CacheError::UnsafePath(path.clone()))?;
            if parent.exists() {
                validate_real_dir(&root, parent)?;
                let prefix = format!("{}.tmp-", part.file_name().unwrap().to_string_lossy());
                for entry in fs::read_dir(parent)? {
                    let entry = entry?;
                    if entry.file_name().to_string_lossy().starts_with(&prefix) {
                        delete_payload_file(&root, &entry.path())?;
                    }
                }
            }
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("UPDATE generation_ids SET closed_at=?2 WHERE generation_id=?1 AND closed_at IS NULL",
                params![id, now()])?;
            tx.commit()?;
        }
        Ok(())
    }

    fn outstanding_sibling_ids(&self) -> Result<Vec<i64>, CacheError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM outstanding_sibling_outputs")?;
        Ok(stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?)
    }

    fn collect_outstanding_siblings(&mut self, ids: &[i64]) {
        for id in ids {
            let row = self.conn.query_row(
                "SELECT id,destination,temp_file,token,file_identity FROM outstanding_sibling_outputs WHERE id=?1",
                [id],
                |row| {
                    Ok(ReservedSiblingOutput {
                        id: row.get(0)?,
                        destination: PathBuf::from(row.get::<_, String>(1)?),
                        temp_path: PathBuf::from(row.get::<_, String>(2)?),
                        token: row.get(3)?,
                        ownership: match row.get::<_, Option<Vec<u8>>>(4)? {
                            Some(bytes) => recorded_identity(&bytes).map_or(SiblingOwnership::Unprovable, SiblingOwnership::Created),
                            None => SiblingOwnership::Reserved,
                        },
                    })
                },
            ).optional();
            match row {
                Ok(Some(row)) => {
                    if let Err(error) = self.finish_sibling_output(&row) {
                        crate::logger::log(format!(
                            "epub sibling leftover cleanup failed (id={id}): {error:?}"
                        ));
                    }
                }
                Ok(None) => {}
                Err(error) => crate::logger::log(format!(
                    "epub sibling leftover row unreadable (id={id}): {error:?}"
                )),
            }
        }
    }

    fn prune_closed_reservations(&mut self) -> Result<(), CacheError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // AUTOINCREMENT's sqlite_sequence retains the high-water mark.
        tx.execute("DELETE FROM generation_ids WHERE closed_at IS NOT NULL", [])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    static FAIL_NEXT_SIBLING_FINISH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn fail_next_sibling_finish_for_test() {
    FAIL_NEXT_SIBLING_FINISH.with(|flag| flag.set(true));
}

fn validate_schema(conn: &Connection) -> Result<(), CacheError> {
    for query in [
        "SELECT generation_id,reserved_at,pdf_file,closed_at FROM generation_ids LIMIT 0",
        "SELECT generation_id,src_path_key,src_path,src_size,src_mtime_ticks,src_sha256,src_head_hash,pdf_file,pdf_size,page_count,direction,profile,created_at,output_version FROM generations LIMIT 0",
        "SELECT src_path_key,generation_id,last_access_at FROM current LIMIT 0",
        "SELECT generation_id,retired_at FROM retired LIMIT 0",
        "SELECT id,destination,temp_file,reserved_at,token,phase,file_identity FROM outstanding_sibling_outputs LIMIT 0",
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
        output_version: row.get(13)?,
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

/// Returns false when a real ancestor is missing; deletion then has nothing to do.
fn validate_real_absolute_dir(path: &Path) -> Result<bool, CacheError> {
    if !path.is_absolute() {
        return Err(CacheError::UnsafePath(path.to_owned()));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        if matches!(component, Component::CurDir | Component::ParentDir) {
            return Err(CacheError::UnsafePath(path.to_owned()));
        }
        current.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let meta = match fs::symlink_metadata(&current) {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if !meta.is_dir() || reparse(&meta) {
            return Err(CacheError::UnsafePath(current));
        }
    }
    Ok(true)
}

fn delete_sibling_file(
    reserved: &ReservedSiblingOutput,
    identity: &FileIdentity,
) -> Result<(), CacheError> {
    let destination = &reserved.destination;
    let path = &reserved.temp_path;
    let parent = destination
        .parent()
        .ok_or_else(|| CacheError::UnsafePath(destination.clone()))?;
    if reserved.id <= 0
        || reserved.token.len() != 64
        || !reserved.token.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !destination.is_absolute()
        || destination
            .extension()
            .is_none_or(|ext| !ext.eq_ignore_ascii_case("pdf"))
        || path.parent() != Some(parent)
        || (path != &parent.join(format!(".miv-part-{}.tmp", reserved.token))
            && path != &parent.join(format!(".miv-part-{}.pdf", reserved.token)))
    {
        return Err(CacheError::UnsafePath(path.clone()));
    }
    if !validate_real_absolute_dir(parent)? {
        return Err(CacheError::InvalidState(
            "sibling parent missing or unreachable",
        ));
    }
    #[cfg(windows)]
    {
        delete_sibling_file_by_handle(path, identity)
    }
    #[cfg(not(windows))]
    {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_file() && !reparse(&meta) => {
                let file = File::open(path)?;
                if file_identity(&file)?.as_ref() != Some(identity) {
                    return Err(CacheError::InvalidState("sibling file identity mismatch"));
                }
                fs::remove_file(path).map_err(Into::into)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Ok(_) => Err(CacheError::UnsafePath(path.clone())),
            Err(error) => Err(error.into()),
        }
    }
}

/// Used only in the create-to-record gap, when the still-open handle itself proves ownership.
pub fn discard_unrecorded_sibling_file(file: &File, path: &Path) -> Result<(), CacheError> {
    #[cfg(windows)]
    {
        use std::mem::size_of;
        use std::os::windows::io::AsRawHandle as _;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{
            FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
        };
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        unsafe {
            SetFileInformationByHandle(
                HANDLE(file.as_raw_handle()),
                FileDispositionInfo,
                (&raw const disposition).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        }
        .map_err(|error| CacheError::Io(io::Error::other(error.to_string())))?;
        let _ = path;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::remove_file(path).map_err(Into::into)
    }
}

/// Publish the still-open destination temp handle without replacing an existing PDF.
#[cfg(windows)]
pub fn publish_sibling_file(file: &File, _temp_path: &Path, destination: &Path) -> io::Result<()> {
    use std::mem::{offset_of, size_of};
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_NORMAL, FILE_BASIC_INFO, FILE_RENAME_INFO, FileBasicInfo, FileRenameInfo,
        SetFileInformationByHandle,
    };

    let handle = HANDLE(file.as_raw_handle());
    // The temp is hidden while in progress; the final PDF must be visible.
    let basic = FILE_BASIC_INFO {
        FileAttributes: FILE_ATTRIBUTE_NORMAL.0,
        ..Default::default()
    };
    unsafe {
        SetFileInformationByHandle(
            handle,
            FileBasicInfo,
            (&raw const basic).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    }
    .map_err(|error| io::Error::other(error.to_string()))?;

    let name: Vec<u16> = destination.as_os_str().encode_wide().collect();
    let name_bytes = name.len().checked_mul(size_of::<u16>()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "sibling destination name too long",
        )
    })?;
    let name_bytes = u32::try_from(name_bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "sibling destination name too long",
        )
    })?;
    let total = offset_of!(FILE_RENAME_INFO, FileName) + name_bytes as usize;
    let mut storage = vec![0_u64; total.div_ceil(size_of::<u64>())];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = HANDLE::default();
        (*info).FileNameLength = name_bytes;
        std::ptr::copy_nonoverlapping(name.as_ptr(), (*info).FileName.as_mut_ptr(), name.len());
        SetFileInformationByHandle(handle, FileRenameInfo, info.cast(), total as u32)
    }
    .map_err(|error| io::Error::other(error.to_string()))
}

#[cfg(not(windows))]
pub fn publish_sibling_file(_file: &File, temp_path: &Path, destination: &Path) -> io::Result<()> {
    // Windows is the supported production platform; this preserves no-replace semantics in tests.
    fs::hard_link(temp_path, destination)?;
    let _ = fs::remove_file(temp_path);
    Ok(())
}

#[cfg(windows)]
fn delete_sibling_file_by_handle(path: &Path, identity: &FileIdentity) -> Result<(), CacheError> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, DELETE, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FileDispositionInfo,
        GetFileInformationByHandle, OPEN_EXISTING, SetFileInformationByHandle,
    };
    use windows::core::PCWSTR;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let raw = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            DELETE.0 | FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE, // deny delete sharing; keep this exact handle
            None,
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
    };
    let raw = match raw {
        Ok(handle) => handle,
        Err(error) if error.code() == ERROR_FILE_NOT_FOUND.to_hresult() => {
            return Ok(());
        }
        Err(error) => return Err(CacheError::Io(io::Error::other(error.to_string()))),
    };
    let file = unsafe { File::from_raw_handle(raw.0) };
    let handle = HANDLE(file.as_raw_handle());
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(handle, &mut info) }
        .map_err(|error| CacheError::Io(io::Error::other(error.to_string())))?;
    if info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_DIRECTORY.0) != 0 {
        return Err(CacheError::UnsafePath(path.to_owned()));
    }
    if file_identity(&file)?.as_ref() != Some(identity) {
        return Err(CacheError::InvalidState("sibling file identity mismatch"));
    }
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    unsafe {
        SetFileInformationByHandle(
            handle,
            FileDispositionInfo,
            (&raw const disposition).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    }
    .map_err(|error| CacheError::Io(io::Error::other(error.to_string())))?;
    Ok(())
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

#[cfg(not(windows))]
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

fn delete_payload_file(root: &Path, path: &Path) -> Result<(), CacheError> {
    // Every payload has exactly two hash directories below the cache root.
    let relative = path
        .strip_prefix(root)
        .map_err(|_| CacheError::UnsafePath(path.to_owned()))?;
    let components: Vec<_> = relative.components().collect();
    if !orphan_name(path)
        || components.len() != 3
        || !components
            .iter()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(CacheError::UnsafePath(path.to_owned()));
    }
    #[cfg(windows)]
    {
        delete_payload_file_by_handle(root, path)
    }
    #[cfg(not(windows))]
    {
        validate_retired_file(root, path)?;
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(windows)]
fn delete_payload_file_by_handle(root: &Path, path: &Path) -> Result<(), CacheError> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, DELETE, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_FLAG_DELETE,
        FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX,
        FILE_DISPOSITION_INFO_EX_FLAGS, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FileDispositionInfo, FileDispositionInfoEx, GetFileInformationByHandle, OPEN_EXISTING,
        SetFileInformationByHandle,
    };
    use windows::core::PCWSTR;

    fn open(path: &Path, access: u32) -> Result<OwnedHandle, windows::core::Error> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let raw = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )?
        };
        Ok(unsafe { OwnedHandle::from_raw_handle(raw.0) })
    }
    fn attributes(handle: &OwnedHandle) -> Result<u32, CacheError> {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe { GetFileInformationByHandle(HANDLE(handle.as_raw_handle()), &mut info) }
            .map_err(|error| CacheError::Io(io::Error::other(error.to_string())))?;
        Ok(info.dwFileAttributes)
    }
    fn is_missing(error: &windows::core::Error) -> bool {
        error.code() == ERROR_FILE_NOT_FOUND.to_hresult()
            || error.code() == ERROR_PATH_NOT_FOUND.to_hresult()
    }
    let root_handle = open(root, FILE_READ_ATTRIBUTES.0)
        .map_err(|error| CacheError::Io(io::Error::other(error.to_string())))?;
    let root_attributes = attributes(&root_handle)?;
    if root_attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || root_attributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0
    {
        return Err(CacheError::UnsafePath(root.to_owned()));
    }
    // Keep each directory handle open while checking the next component.
    let mut directories = vec![root_handle];
    let mut parent = root.to_owned();
    let relative = path
        .strip_prefix(root)
        .map_err(|_| CacheError::UnsafePath(path.to_owned()))?;
    for component in relative
        .components()
        .take(relative.components().count().saturating_sub(1))
    {
        parent.push(component.as_os_str());
        let handle = match open(&parent, FILE_READ_ATTRIBUTES.0) {
            Ok(handle) => handle,
            Err(error) if is_missing(&error) => return Ok(()),
            Err(error) => return Err(CacheError::Io(io::Error::other(error.to_string()))),
        };
        let flags = attributes(&handle)?;
        if flags & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 || flags & FILE_ATTRIBUTE_DIRECTORY.0 == 0 {
            return Err(CacheError::UnsafePath(parent));
        }
        directories.push(handle);
    }
    let target = match open(path, DELETE.0 | FILE_READ_ATTRIBUTES.0) {
        Ok(handle) => handle,
        Err(error) if is_missing(&error) => return Ok(()),
        Err(error) => return Err(CacheError::Io(io::Error::other(error.to_string()))),
    };
    if attributes(&target)? & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_DIRECTORY.0) != 0 {
        return Err(CacheError::UnsafePath(path.to_owned()));
    }
    let handle = HANDLE(target.as_raw_handle());
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_INFO_EX_FLAGS(
            FILE_DISPOSITION_FLAG_DELETE.0 | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS.0,
        ),
    };
    let result = unsafe {
        SetFileInformationByHandle(
            handle,
            FileDispositionInfoEx,
            (&raw const disposition).cast(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    };
    if result.is_err() {
        let fallback = FILE_DISPOSITION_INFO { DeleteFile: true };
        unsafe {
            SetFileInformationByHandle(
                handle,
                FileDispositionInfo,
                (&raw const fallback).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        }
        .map_err(|error| CacheError::Io(io::Error::other(error.to_string())))?;
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
    let result = (|| -> Result<(AliveGuard, bool, Vec<i64>), GateReason> {
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
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            options.share_mode(0x0000_0001 | 0x0000_0002); // no FILE_SHARE_DELETE
        }
        let file = options.open(alive_path).map_err(GateReason::Lock)?;
        let exclusive = lock(&file, true, true).is_ok();
        let mut sibling_ids = Vec::new();
        if exclusive {
            let cleanup = (|| -> Result<(), GateReason> {
                let mut db = EpubCache::open_at(&data_dir).map_err(GateReason::Schema)?;
                db.collect_retired().map_err(GateReason::Cleanup)?;
                db.collect_pending().map_err(GateReason::Cleanup)?;
                sibling_ids = match db.outstanding_sibling_ids() {
                    Ok(ids) => ids,
                    Err(error) => {
                        crate::logger::log(format!(
                            "epub sibling leftover listing failed: {error:?}"
                        ));
                        Vec::new()
                    }
                };
                crate::materializer::cleanup_epub_sibling_work_startup(&data_dir);
                db.prune_closed_reservations()
                    .map_err(GateReason::Cleanup)?;
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
        Ok((AliveGuard { file, data_dir }, exclusive, sibling_ids))
    })();
    match result {
        Ok((guard, cleaned, sibling_ids)) => {
            if !sibling_ids.is_empty() {
                let cleanup_dir = guard.data_dir.clone();
                if let Err(error) = std::thread::Builder::new()
                    .name("epub-sibling-cleanup".into())
                    .spawn(move || match EpubCache::open_at(&cleanup_dir) {
                        Ok(mut db) => db.collect_outstanding_siblings(&sibling_ids),
                        Err(error) => crate::logger::log(format!(
                            "epub sibling cleanup DB open failed: {error:?}"
                        )),
                    })
                {
                    crate::logger::log(format!("epub sibling cleanup thread failed: {error}"));
                }
            }
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
pub(crate) struct TestReconvertedEpub {
    pub(crate) source: PathBuf,
    pub(crate) old: GenerationRow,
    pub(crate) current: GenerationRow,
    _pin: crate::pdf_loader::TestEpubPin,
    _root: tempfile::TempDir,
}

#[cfg(test)]
pub(crate) fn reconverted_for_worker_test() -> TestReconvertedEpub {
    struct Source(SourceState);
    impl SourceGuard for Source {
        fn state(&self) -> std::io::Result<SourceState> {
            Ok(self.0)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("worker-book.epub");
    let mut cache = EpubCache::open_at(root.path()).unwrap();
    let mut publish = |bytes: &[u8], page_count: u32| {
        std::fs::write(&source, bytes).unwrap();
        let state = source_state(&std::fs::metadata(&source).unwrap());
        let reserved = cache.reserve_output(&source).unwrap();
        std::fs::create_dir_all(reserved.final_path().parent().unwrap()).unwrap();
        std::fs::write(reserved.final_path(), b"%PDF-1.4\n").unwrap();
        let row = GenerationRow {
            generation_id: reserved.generation_id(),
            src_path_key: src_key(&source),
            src_path: source.clone(),
            src_state: state,
            src_sha256: format!("hash-{page_count}"),
            src_head_hash: format!("head-{page_count}"),
            pdf_file: reserved.final_path().to_path_buf(),
            pdf_size: 9,
            page_count,
            direction: "ltr".into(),
            profile: "test".into(),
            created_at: 1,
            output_version: CONVERTER_OUTPUT_VERSION,
        };
        assert_eq!(
            cache.publish(&row, &Source(state)).unwrap(),
            PublishOutcome::Published
        );
        row
    };
    let old = publish(b"first source", 2);
    let prior_run = crate::pdf_loader::pin_epub_for_test(&source, old.generation_id, old.pdf_size);
    drop(prior_run);
    let current = publish(b"replacement with more pages", 5);
    assert_ne!(old.generation_id, current.generation_id);
    assert_ne!(old.page_count, current.page_count);
    let pin =
        crate::pdf_loader::pin_epub_for_test(&source, current.generation_id, current.pdf_size);
    TestReconvertedEpub {
        source,
        old,
        current,
        _pin: pin,
        _root: root,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    fn wait_for_sibling_rows_to_clear(data_dir: &Path) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let db = EpubCache::open_at(data_dir).unwrap();
            if db.outstanding_sibling_ids().unwrap().is_empty() {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "sibling cleanup thread did not finish"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn epub_cache_source_key_distinguishes_drives() {
        let c = Path::new(r"C:\Books\a.epub");
        let d = Path::new(r"D:\Books\a.epub");
        assert_ne!(src_key(c), src_key(d));
        assert_ne!(
            generation_file(Path::new("cache"), c, 1),
            generation_file(Path::new("cache"), d, 1)
        );
    }

    #[test]
    fn epub_cache_reserved_output_token_matches_recorded_part() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("book.epub");
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let output = db.reserve_output(&source).unwrap();
        let recorded: String = db
            .conn
            .query_row(
                "SELECT pdf_file FROM generation_ids WHERE generation_id=?1",
                [output.generation_id()],
                |row| row.get(0),
            )
            .unwrap();
        let recorded = PathBuf::from(recorded);
        let expected_part = recorded.with_extension("pdf.part");
        assert_eq!(output.final_path(), recorded.as_path());
        assert_eq!(output.part_path(), expected_part.as_path());
    }

    struct FakeGuard(SourceState);
    impl SourceGuard for FakeGuard {
        fn state(&self) -> io::Result<SourceState> {
            Ok(self.0)
        }
    }

    fn candidate(db: &mut EpubCache, root: &Path, src: &Path, state: SourceState) -> GenerationRow {
        let id = db.reserve_output(src).unwrap().generation_id();
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
            output_version: CONVERTER_OUTPUT_VERSION,
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
        drop(db);
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Enabled { .. }
        ));
        assert!(!a.pdf_file.exists());
        assert!(
            EpubCache::open_at(tmp.path())
                .unwrap()
                .generation(a.generation_id)
                .unwrap()
                .is_none()
        );
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
        assert_eq!(
            db.current_generation(&a.src_path_key).unwrap(),
            Some(a.clone())
        );
        drop(db);
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Enabled { .. }
        ));
        assert!(a.pdf_file.exists());
        assert!(!b.pdf_file.exists());
        assert!(
            EpubCache::open_at(tmp.path())
                .unwrap()
                .generation(b.generation_id)
                .unwrap()
                .is_none()
        );
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
        let losers: Vec<_> = results
            .iter()
            .filter(|(row, _)| row.generation_id != current.generation_id)
            .map(|(row, _)| row.clone())
            .collect();
        for (row, outcome) in results {
            if row.generation_id != current.generation_id {
                assert!(matches!(outcome, PublishOutcome::Adopted(_)));
                assert!(retired(&db, row.generation_id));
            }
        }
        drop(db);
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Enabled { .. }
        ));
        let db = EpubCache::open_at(tmp.path()).unwrap();
        assert!(current.pdf_file.exists());
        for loser in losers {
            assert!(!loser.pdf_file.exists());
            assert!(db.generation(loser.generation_id).unwrap().is_none());
            assert!(!retired(&db, loser.generation_id));
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
        assert!(db.reserve_output(&src).unwrap().generation_id() > a.generation_id);
    }

    #[test]
    fn epub_cache_directory_wipe_keeps_ids_and_invalidates_derived_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("book.epub");
        let mut cache = EpubCache::open_at(tmp.path()).unwrap();
        let first = candidate(&mut cache, tmp.path(), &source, state(1));
        cache.publish(&first, &FakeGuard(state(1))).unwrap();
        let catalogs = tmp.path().join("thumbs");
        let catalog = crate::catalog::CatalogDb::open(&catalogs, tmp.path()).unwrap();
        catalog
            .set_pdf_meta(
                "book.epub",
                first.generation_id,
                first.pdf_size as i64,
                3,
                false,
            )
            .unwrap();
        catalog
            .save_with_layout_dims(
                "pdfthumb:book.epub",
                first.generation_id,
                first.pdf_size as i64,
                1,
                1,
                None,
                None,
                b"old",
            )
            .unwrap();

        fs::remove_dir_all(tmp.path().join("epub_cache")).unwrap();
        drop(cache);
        // Simulate the next run: the payload folder is gone, but the ID ledger remains.
        let mut cache = EpubCache::open_at(tmp.path()).unwrap();
        let mut second = candidate(&mut cache, tmp.path(), &source, state(2));
        second.page_count = 8;
        cache.publish(&second, &FakeGuard(state(2))).unwrap();
        assert!(second.generation_id > first.generation_id);
        assert_eq!(
            cache.current_generation(&first.src_path_key).unwrap(),
            Some(second.clone())
        );
        assert_eq!(
            catalog
                .get_pdf_meta("book.epub", second.generation_id, second.pdf_size as i64)
                .unwrap(),
            None
        );
        let thumb = catalog.load_one("pdfthumb:book.epub").unwrap().unwrap();
        assert_ne!(
            (thumb.mtime, thumb.file_size),
            (second.generation_id, second.pdf_size as i64)
        );
        assert_eq!(
            catalog
                .get_pdf_meta("book.epub", first.generation_id, first.pdf_size as i64)
                .unwrap(),
            Some((3, false))
        );
    }

    #[test]
    fn epub_cache_manager_retires_selected_and_all_without_removing_live_files() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = EpubCache::open_at(tmp.path()).unwrap();
        let a_src = tmp.path().join("a.epub");
        let b_src = tmp.path().join("b.epub");
        let a = candidate(&mut cache, tmp.path(), &a_src, state(1));
        let b = candidate(&mut cache, tmp.path(), &b_src, state(2));
        cache.publish(&a, &FakeGuard(state(1))).unwrap();
        cache.publish(&b, &FakeGuard(state(2))).unwrap();
        let rows = cache.list_current().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .all(|row| !row.retired && row.last_access_at > 0)
        );
        assert!(cache.retire_generation(a.generation_id).unwrap());
        assert!(!cache.retire_generation(a.generation_id).unwrap());
        assert_eq!(cache.retire_all_current().unwrap(), 1);
        let rows = cache.list_current().unwrap();
        assert!(rows.iter().all(|row| row.retired));
        assert!(a.pdf_file.exists() && b.pdf_file.exists());
        assert_eq!(cache.current_generation(&a.src_path_key).unwrap(), Some(a));
        assert_eq!(cache.current_generation(&b.src_path_key).unwrap(), Some(b));
    }

    #[cfg(windows)]
    #[test]
    fn epub_source_state_distinguishes_out_of_unix_nanosecond_range_times() {
        use std::fs::FileTimes;
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("book.epub");
        fs::write(&source, b"same size").unwrap();
        let file = File::options().write(true).open(&source).unwrap();
        let old = UNIX_EPOCH
            .checked_sub(Duration::from_secs(11_000_000_000))
            .unwrap();
        file.set_times(FileTimes::new().set_modified(old)).unwrap();
        let before_old = source_state(&file.metadata().unwrap());
        file.set_times(FileTimes::new().set_modified(old + Duration::from_secs(2)))
            .unwrap();
        let after_old = source_state(&file.metadata().unwrap());
        assert_eq!(before_old.size, after_old.size);
        assert_ne!(before_old, after_old);

        let future = UNIX_EPOCH + Duration::from_secs(10_000_000_000);
        file.set_times(FileTimes::new().set_modified(future))
            .unwrap();
        let before_future = source_state(&file.metadata().unwrap());
        file.set_times(FileTimes::new().set_modified(future + Duration::from_secs(2)))
            .unwrap();
        let after_future = source_state(&file.metadata().unwrap());
        assert_eq!(before_future.size, after_future.size);
        assert_ne!(before_future, after_future);
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
            .truncate(false)
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
            .truncate(false)
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
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let src = tmp.path().join("book.epub");
        let id = db.reserve_output(&src).unwrap().generation_id();
        let final_file = generation_file(tmp.path(), &src, id);
        let nested = final_file.parent().unwrap();
        fs::create_dir_all(nested).unwrap();
        let part = final_file.with_extension("pdf.part");
        let tmp_part = nested.join(format!(
            "{}.tmp-7-1",
            part.file_name().unwrap().to_string_lossy()
        ));
        for path in [&final_file, &part, &tmp_part] {
            fs::write(path, b"junk").unwrap();
        }
        let outside = tmp.path().join("outside.g1.pdf");
        fs::write(&outside, b"keep").unwrap();
        drop(db);
        let gate = startup_gate(tmp.path());
        assert!(matches!(gate, GateOutcome::Enabled { .. }));
        assert!(fs::read_dir(nested).unwrap().next().is_none());
        assert_eq!(fs::read(outside).unwrap(), b"keep");
        let db = EpubCache::open_at(tmp.path()).unwrap();
        let reservation: Option<i64> = db
            .conn
            .query_row(
                "SELECT generation_id FROM generation_ids WHERE generation_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert!(reservation.is_none());
    }

    #[test]
    fn epub_cache_gate_removes_only_recorded_sibling_output() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let destination = tmp.path().join("book.pdf");
        let mut reserved = db.reserve_sibling_output(&destination).unwrap();
        let file = reserved.create_file().unwrap();
        db.mark_sibling_created(&mut reserved, &file).unwrap();
        drop(file);
        let unrecorded = tmp.path().join(format!(".miv-part-{}.pdf", "0".repeat(64)));
        fs::write(&unrecorded, b"unrecorded").unwrap();
        drop(db);
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Enabled { cleaned: true, .. }
        ));
        wait_for_sibling_rows_to_clear(tmp.path());
        assert!(!reserved.temp_path().exists());
        assert_eq!(fs::read(unrecorded).unwrap(), b"unrecorded");
        let db = EpubCache::open_at(tmp.path()).unwrap();
        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM outstanding_sibling_outputs",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn epub_cache_reserved_sibling_never_deletes_preexisting_same_name_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let reserved = db
            .reserve_sibling_output(&tmp.path().join("book.pdf"))
            .unwrap();
        fs::write(reserved.temp_path(), b"user file").unwrap();
        assert!(reserved.create_file().is_err());
        db.abandon_sibling_reservation(&reserved).unwrap();
        assert_eq!(fs::read(reserved.temp_path()).unwrap(), b"user file");
        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM outstanding_sibling_outputs",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[cfg(windows)]
    #[test]
    fn sibling_zero_file_id_is_unprovable_and_gate_never_deletes() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let mut reserved = db
            .reserve_sibling_output(&tmp.path().join("book.pdf"))
            .unwrap();
        let file = reserved.create_file().unwrap();
        assert_eq!(file_id_info_identity(17, &[0; 16]), None);
        db.mark_sibling_created_with_identity(&mut reserved, file_id_info_identity(17, &[0; 16]))
            .unwrap();
        assert!(reserved.was_created());
        let phase: String = db
            .conn
            .query_row(
                "SELECT phase FROM outstanding_sibling_outputs WHERE id=?1",
                [reserved.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(phase, "unprovable");
        drop(file);
        drop(db);
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Enabled { .. }
        ));
        wait_for_sibling_rows_to_clear(tmp.path());
        assert!(reserved.temp_path().exists());
    }

    #[cfg(windows)]
    #[test]
    fn sibling_temp_denies_concurrent_write_until_handle_publish() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let reserved = db
            .reserve_sibling_output(&tmp.path().join("book.pdf"))
            .unwrap();
        let file = reserved.create_file().unwrap();
        assert!(
            OpenOptions::new()
                .write(true)
                .open(reserved.temp_path())
                .is_err()
        );
        discard_unrecorded_sibling_file(&file, reserved.temp_path()).unwrap();
        drop(file);
        db.abandon_sibling_reservation(&reserved).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn sibling_handle_publish_never_replaces_pdf_created_after_temp() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let destination = tmp.path().join("book.pdf");
        let mut reserved = db.reserve_sibling_output(&destination).unwrap();
        let mut file = reserved.create_file().unwrap();
        db.mark_sibling_created(&mut reserved, &file).unwrap();
        use std::io::Write as _;
        file.write_all(b"verified PDF bytes").unwrap();
        fs::write(&destination, b"another writer's PDF").unwrap();
        assert!(publish_sibling_file(&file, reserved.temp_path(), &destination).is_err());
        discard_unrecorded_sibling_file(&file, reserved.temp_path()).unwrap();
        drop(file);
        db.finish_sibling_output(&reserved).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"another writer's PDF");
        assert!(!reserved.temp_path().exists());
    }

    #[test]
    fn epub_cache_reserved_row_with_file_is_dropped_without_deletion_at_gate() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let reserved = db
            .reserve_sibling_output(&tmp.path().join("book.pdf"))
            .unwrap();
        fs::write(reserved.temp_path(), b"not proven ours").unwrap();
        drop(db);
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Enabled { .. }
        ));
        wait_for_sibling_rows_to_clear(tmp.path());
        assert_eq!(fs::read(reserved.temp_path()).unwrap(), b"not proven ours");
    }

    #[test]
    fn epub_cache_created_sibling_identity_mismatch_keeps_replacement() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let mut reserved = db
            .reserve_sibling_output(&tmp.path().join("book.pdf"))
            .unwrap();
        let file = reserved.create_file().unwrap();
        db.mark_sibling_created(&mut reserved, &file).unwrap();
        drop(file);
        let moved = tmp.path().join("original-temp");
        fs::rename(reserved.temp_path(), &moved).unwrap();
        fs::write(reserved.temp_path(), b"replacement").unwrap();
        assert!(matches!(
            db.finish_sibling_output(&reserved),
            Err(CacheError::InvalidState("sibling file identity mismatch"))
        ));
        assert_eq!(fs::read(reserved.temp_path()).unwrap(), b"replacement");
        assert_eq!(db.outstanding_sibling_ids().unwrap(), vec![reserved.id]);
    }

    #[test]
    fn epub_cache_created_sibling_matching_identity_is_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let mut reserved = db
            .reserve_sibling_output(&tmp.path().join("book.pdf"))
            .unwrap();
        let file = reserved.create_file().unwrap();
        db.mark_sibling_created(&mut reserved, &file).unwrap();
        drop(file);
        db.finish_sibling_output(&reserved).unwrap();
        assert!(!reserved.temp_path().exists());
        assert!(db.outstanding_sibling_ids().unwrap().is_empty());
    }

    #[test]
    fn epub_cache_unreachable_created_sibling_keeps_row_without_disabling_gate() {
        let tmp = tempfile::tempdir().unwrap();
        let shelf = tmp.path().join("shelf");
        fs::create_dir(&shelf).unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let mut reserved = db.reserve_sibling_output(&shelf.join("book.pdf")).unwrap();
        let file = reserved.create_file().unwrap();
        db.mark_sibling_created(&mut reserved, &file).unwrap();
        drop(file);
        drop(db);
        fs::rename(&shelf, tmp.path().join("temporarily-unreachable")).unwrap();
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Enabled { .. }
        ));
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        db.collect_outstanding_siblings(&[reserved.id]);
        assert_eq!(db.outstanding_sibling_ids().unwrap(), vec![reserved.id]);
    }

    #[test]
    fn epub_cache_old_generation_schema_defaults_output_version_to_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = Connection::open(tmp.path().join("epub_cache.db")).unwrap();
        conn.execute_batch("CREATE TABLE generations (
            generation_id INTEGER PRIMARY KEY, src_path_key TEXT NOT NULL,
            src_path TEXT NOT NULL, src_size INTEGER NOT NULL, src_mtime_ticks INTEGER NOT NULL,
            src_sha256 TEXT NOT NULL, src_head_hash TEXT NOT NULL, pdf_file TEXT NOT NULL,
            pdf_size INTEGER NOT NULL, page_count INTEGER NOT NULL, direction TEXT NOT NULL,
            profile TEXT NOT NULL, created_at INTEGER NOT NULL);
            INSERT INTO generations VALUES (1,'key','book.epub',1,1,'','','book.pdf',1,1,'ltr','old',1);")
            .unwrap();
        drop(conn);
        let db = EpubCache::open_at(tmp.path()).unwrap();
        let version: i64 = db
            .conn
            .query_row(
                "SELECT output_version FROM generations WHERE generation_id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 0);
    }

    #[test]
    fn epub_cache_gate_closes_sibling_record_after_folder_was_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("removed-shelf");
        fs::create_dir(&folder).unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        db.reserve_sibling_output(&folder.join("book.pdf")).unwrap();
        drop(db);
        fs::remove_dir(folder).unwrap();
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Enabled { cleaned: true, .. }
        ));
        wait_for_sibling_rows_to_clear(tmp.path());
        let db = EpubCache::open_at(tmp.path()).unwrap();
        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM outstanding_sibling_outputs",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn epub_cache_gate_removes_dead_sibling_work_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("epub_sibling_work");
        let stale = root.join("epub-0-0");
        let unrecorded = root.join("keep");
        fs::create_dir_all(&stale).unwrap();
        fs::create_dir(&unrecorded).unwrap();
        fs::write(stale.join("finished.pdf.tmp-worker"), b"leftover").unwrap();
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Enabled { cleaned: true, .. }
        ));
        assert!(!stale.exists());
        assert!(unrecorded.exists());
    }

    #[cfg(windows)]
    #[test]
    fn epub_cache_gate_refuses_recorded_sibling_through_junction() {
        let tmp = tempfile::tempdir().unwrap();
        let shelf = tmp.path().join("shelf");
        let real = tmp.path().join("real");
        fs::create_dir(&shelf).unwrap();
        fs::create_dir(&real).unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let mut reserved = db.reserve_sibling_output(&shelf.join("book.pdf")).unwrap();
        let file = reserved.create_file().unwrap();
        db.mark_sibling_created(&mut reserved, &file).unwrap();
        drop(file);
        drop(db);
        let real_temp = real.join(reserved.temp_path().file_name().unwrap());
        fs::write(&real_temp, b"keep").unwrap();
        let moved = tmp.path().join("moved-shelf");
        fs::rename(&shelf, &moved).unwrap();
        let result = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&shelf)
            .arg(&real)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Enabled { .. }
        ));
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        db.collect_outstanding_siblings(&[reserved.id]);
        assert_eq!(db.outstanding_sibling_ids().unwrap(), vec![reserved.id]);
        assert_eq!(fs::read(real_temp).unwrap(), b"keep");
        fs::remove_dir(shelf).unwrap();
    }

    #[test]
    fn epub_cache_gate_does_not_visit_published_payloads() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let mut files = Vec::new();
        for index in 0..100 {
            let src = tmp.path().join(format!("book-{index}.epub"));
            let row = candidate(&mut db, tmp.path(), &src, state(1));
            db.publish(&row, &FakeGuard(state(1))).unwrap();
            files.push(row.pdf_file);
        }
        let unrelated = tmp.path().join("epub_cache").join("unrelated");
        fs::create_dir(&unrelated).unwrap();
        fs::write(unrelated.join("untracked.g999.pdf"), b"keep").unwrap();
        drop(db);
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Enabled { .. }
        ));
        assert!(files.iter().all(|path| path.exists()));
        assert!(unrelated.join("untracked.g999.pdf").exists());
    }

    #[test]
    fn epub_cache_gate_prunes_closed_reservations_without_reusing_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("book.epub");
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let published = candidate(&mut db, tmp.path(), &src, state(1));
        db.publish(&published, &FakeGuard(state(1))).unwrap();
        let pending = db.reserve_output(&src).unwrap().generation_id();
        assert!(pending > published.generation_id);
        drop(db);
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Enabled { cleaned: true, .. }
        ));
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM generation_ids", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
        assert!(db.generation(published.generation_id).unwrap().is_some());
        assert!(published.pdf_file.exists());
        assert!(db.reserve_output(&src).unwrap().generation_id() > pending);
    }

    #[cfg(windows)]
    #[test]
    fn epub_cache_alive_lock_file_cannot_be_renamed_or_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let gate = startup_gate(tmp.path());
        assert!(matches!(gate, GateOutcome::Enabled { .. }));
        let alive = tmp.path().join("epub_cache").join(".alive");
        assert!(fs::rename(&alive, alive.with_extension("moved")).is_err());
        assert!(fs::remove_file(&alive).is_err());
        assert!(alive.exists());
        drop(gate);
    }

    #[cfg(windows)]
    #[test]
    fn epub_cache_gate_refuses_junction_to_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let src = tmp.path().join("book.epub");
        let id = db.reserve_output(&src).unwrap().generation_id();
        let path = generation_file(tmp.path(), &src, id);
        let outside = tmp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let target = outside.join(path.file_name().unwrap());
        fs::write(&target, b"keep").unwrap();
        let junction = path.parent().unwrap();
        fs::create_dir_all(junction.parent().unwrap()).unwrap();
        let result = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(junction)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        drop(db);
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Disabled(GateReason::Cleanup(CacheError::UnsafePath(_)))
        ));
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        fs::remove_dir(junction).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn epub_cache_gate_refuses_junction_to_inside() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = EpubCache::open_at(tmp.path()).unwrap();
        let src = tmp.path().join("book.epub");
        let id = db.reserve_output(&src).unwrap().generation_id();
        let path = generation_file(tmp.path(), &src, id);
        let root = tmp.path().join("epub_cache");
        let inside = root.join("real");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&inside).unwrap();
        let target = inside.join(path.file_name().unwrap());
        fs::write(&target, b"keep").unwrap();
        let junction = path.parent().unwrap();
        fs::create_dir_all(junction.parent().unwrap()).unwrap();
        let result = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(junction)
            .arg(&inside)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        drop(db);
        assert!(matches!(
            startup_gate(tmp.path()),
            GateOutcome::Disabled(GateReason::Cleanup(CacheError::UnsafePath(_)))
        ));
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        fs::remove_dir(junction).unwrap();
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
