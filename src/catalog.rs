use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CATALOG_VERSION: &str = "2";
const PDF_LAYOUT_DIMS_META_KEY: &str = "pdf_layout_dims_version";
const PDF_LAYOUT_DIMS_VERSION: &str = "2";
pub const THUMB_LONG_SIDE: u32 = 512;

/// A known media identity includes empty files and epoch-zero timestamps.
/// Failure to obtain either stamp component is an unknown identity.
pub(crate) fn media_source_identity_from_metadata(
    metadata: &std::fs::Metadata,
) -> Option<(i64, i64)> {
    metadata.modified().ok()?;
    Some((
        crate::ui_helpers::mtime_secs(metadata),
        i64::try_from(metadata.len()).ok()?,
    ))
}

pub(crate) fn media_source_identity(path: &Path) -> Option<(i64, i64)> {
    media_source_identity_from_metadata(&std::fs::metadata(path).ok()?)
}

// -----------------------------------------------------------------------
// DB path helpers
// -----------------------------------------------------------------------

use crate::path_key;

/// `{cache_dir}/{xx}/{sha256}.db` の形式で DB ファイルパスを返す。
/// xx はハッシュ hex 先頭2文字（256サブフォルダに分散）。
pub fn db_path_for(cache_dir: &Path, folder_path: &Path) -> PathBuf {
    // 通常のサブフォルダは従来どおりドライブ文字を捨て、リムーバブルドライブの
    // レター変更でもキャッシュを引き継ぐ。一方でドライブルートだけは `C:\Photos`
    // と `D:\Photos` のような直下同名項目が同じ root catalog / 同じ basename key に
    // 衝突するため、ドライブ文字を保持して DB 自体を分離する。
    let normalized = if path_key::is_drive_or_share_root(folder_path) {
        path_key::normalize_keep_drive(folder_path)
    } else {
        path_key::normalize(folder_path)
    };
    let hash = format!("{:x}", Sha256::digest(normalized.as_bytes()));
    cache_dir.join(&hash[..2]).join(format!("{}.db", hash))
}

// -----------------------------------------------------------------------
// キャッシュエントリ
// -----------------------------------------------------------------------

#[derive(Clone)]
pub struct CacheEntry {
    pub mtime: i64,
    pub file_size: i64,
    pub jpeg_data: Vec<u8>,
    /// 元画像 / PDF thumbnail raster のピクセル寸法 (幅, 高さ)。
    /// 旧バージョンで保存されたエントリには NULL が入るため Option で表現する。
    pub source_dims: Option<(u32, u32)>,
    /// raster の整数丸めに依存しないレイアウト寸法。PDF page box を 1/1000 point で
    /// 保持する。通常画像と旧エントリは NULL。
    pub layout_dims: Option<(u32, u32)>,
    pub folder_provenance: Option<FolderThumbProvenance>,
    pub selection_proof: Option<FolderSelectionProof>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FolderThumbProvenance {
    AutoSelected,
    Seeded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogRevision {
    pub instance_id: String,
    pub revision: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CatalogProofState {
    Present(CatalogRevision),
    Absent,
    Unverifiable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogProofDependency {
    pub directory: PathBuf,
    pub state: CatalogProofState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderSelectionWinner {
    pub path: PathBuf,
    pub mtime: i64,
    pub file_size: i64,
    pub archive_row_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderSelectionProof {
    pub directories: Vec<CatalogProofDependency>,
    /// Required in serialized v3 proofs. Older proofs that lack this field
    /// cannot silently deserialize as "no pin dependency".
    pub pin_store: PinStoreProof,
    pub winner: FolderSelectionWinner,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PinStoreProof {
    NotConsulted,
    Observed(CatalogProofState),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CatalogFileStamp {
    db: FileComponentStamp,
    wal: WalFileStamp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileComponentStamp {
    created: Option<std::time::SystemTime>,
    modified: Option<std::time::SystemTime>,
    len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WalFileStamp {
    Absent,
    Present(FileComponentStamp),
    Unverifiable,
}

impl FileComponentStamp {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            created: metadata.created().ok(),
            modified: metadata.modified().ok(),
            len: metadata.len(),
        }
    }
}

impl CatalogFileStamp {
    fn from_path(db_path: &Path, metadata: &std::fs::Metadata) -> Self {
        let mut wal_name = db_path.as_os_str().to_os_string();
        wal_name.push("-wal");
        let wal = match std::fs::metadata(PathBuf::from(wal_name)) {
            Ok(metadata) => WalFileStamp::Present(FileComponentStamp::from_metadata(&metadata)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => WalFileStamp::Absent,
            Err(_) => WalFileStamp::Unverifiable,
        };
        Self {
            db: FileComponentStamp::from_metadata(metadata),
            wal,
        }
    }
}

// Failed legacy initialization is memoized by path and file identity/stamp for
// this process. The selection worker is the sole caller of this path.
fn failed_legacy_initializations() -> &'static Mutex<HashMap<PathBuf, CatalogFileStamp>> {
    static FAILED: OnceLock<Mutex<HashMap<PathBuf, CatalogFileStamp>>> = OnceLock::new();
    FAILED.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
fn legacy_init_attempts() -> &'static Mutex<HashMap<PathBuf, usize>> {
    static ATTEMPTS: OnceLock<Mutex<HashMap<PathBuf, usize>>> = OnceLock::new();
    ATTEMPTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn valid_dims(width: Option<u32>, height: Option<u32>) -> Option<(u32, u32)> {
    match (width, height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => Some((width, height)),
        _ => None,
    }
}

/// ZIP / 画像のみフォルダ / 変換対象アーカイブのページ数キャッシュ種別。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerPageKind {
    Folder = 1,
    Zip = 2,
    Archive = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContainerPageMeta {
    /// `None` は走査に成功したが、本として扱う対象ではなかったことを表す。
    pub page_count: Option<u32>,
}

/// A definitive media probe result. Interrupted probes are never persisted.
#[derive(Clone, Debug, PartialEq)]
pub enum VideoMeta {
    Read {
        duration_secs: Option<f64>,
        dims: Option<(u32, u32)>,
        codec: Option<String>,
    },
    Unreadable,
}

/// 保存済みサムネのバイト列からヘッダのみで `(w, h)` を取り出す。
/// フォーマットは auto-detect (`with_guessed_format`)。これは旧バージョンが JPEG で
/// 保存していたエントリ ([`decode_thumb_to_color_image`] が "WebP or old JPEG" の
/// 両方を読んでいる) との互換性のため。フルデコードは走らない
/// (`ImageReader::into_dimensions` はチャンクヘッダだけを読む)。
pub fn decode_thumb_dims(data: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

// -----------------------------------------------------------------------
// CatalogDb
// -----------------------------------------------------------------------

// Shared exclusively by catalogs in one cache directory. The owner never holds
// its state lock while acquiring a connection lock or doing filesystem/SQLite I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CatalogAdmission {
    Admitted(u64),
    DisplayOnly(u64),
}

#[derive(Debug)]
pub(crate) enum CatalogDeleteOperation {
    All,
    OlderThan(u64),
    Folder(PathBuf),
}

#[derive(Debug)]
enum CatalogAccessState {
    Accepting(u64),
    Deleting {
        epoch: u64,
        operation: CatalogDeleteOperation,
    },
}

enum AudioArtCacheNotice {
    Fresh,
    Pending(String),
    Shown,
}

struct CatalogAccessInner {
    state: CatalogAccessState,
    active: usize,
    connections: Vec<std::sync::Weak<CatalogConnection>>,
    audio_art_notice: AudioArtCacheNotice,
}

pub struct CatalogAccess {
    inner: Mutex<CatalogAccessInner>,
    changed: std::sync::Condvar,
}

impl CatalogAccess {
    fn new() -> Self {
        Self {
            inner: Mutex::new(CatalogAccessInner {
                state: CatalogAccessState::Accepting(0),
                active: 0,
                connections: Vec::new(),
                audio_art_notice: AudioArtCacheNotice::Fresh,
            }),
            changed: std::sync::Condvar::new(),
        }
    }

    pub fn for_cache_dir(cache_dir: &Path) -> std::sync::Arc<Self> {
        static OWNERS: OnceLock<Mutex<HashMap<String, std::sync::Arc<CatalogAccess>>>> =
            OnceLock::new();
        let mut owners = OWNERS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap();
        let key = path_key::normalize_keep_drive(cache_dir);
        if let Some(owner) = owners.get(&key).cloned() {
            return owner;
        }
        let owner = std::sync::Arc::new(Self::new());
        owners.insert(key, owner.clone());
        owner
    }

    pub fn admit(&self) -> CatalogAdmission {
        match self.inner.lock().unwrap().state {
            CatalogAccessState::Accepting(epoch) => CatalogAdmission::Admitted(epoch),
            CatalogAccessState::Deleting { epoch, .. } => CatalogAdmission::DisplayOnly(epoch),
        }
    }

    /// Expected epoch retirement is not a rare cache fault or a user notice.
    pub fn is_admitted(&self, admission: CatalogAdmission) -> bool {
        matches!((self.admit(), admission), (CatalogAdmission::Admitted(current), CatalogAdmission::Admitted(epoch)) if current == epoch)
    }

    /// One rebuildable-cache notice per process/cache directory; no retry state.
    pub fn record_audio_art_cache_error(&self, detail: impl Into<String>) {
        let mut inner = self.inner.lock().unwrap();
        if matches!(inner.audio_art_notice, AudioArtCacheNotice::Fresh) {
            inner.audio_art_notice = AudioArtCacheNotice::Pending(detail.into());
        }
    }

    /// Memory-only UI polling. A notice can be consumed only once.
    pub fn take_audio_art_notice(&self) -> Option<String> {
        let mut inner = self.inner.lock().unwrap();
        if !matches!(inner.audio_art_notice, AudioArtCacheNotice::Pending(_)) {
            return None;
        }
        let AudioArtCacheNotice::Pending(detail) =
            std::mem::replace(&mut inner.audio_art_notice, AudioArtCacheNotice::Shown)
        else {
            unreachable!()
        };
        Some(detail)
    }

    /// Worker-only continuation of a catalog-only lookup, never an extraction retry.
    pub fn wait_read_admission(&self, cancel: &AtomicBool) -> Option<CatalogAdmission> {
        self.wait_read_admission_with(|| cancel.load(Ordering::Relaxed))
    }

    pub fn wait_read_admission_with(
        &self,
        canceled: impl Fn() -> bool,
    ) -> Option<CatalogAdmission> {
        loop {
            if canceled() {
                return None;
            }
            let inner = self.inner.lock().unwrap();
            if let CatalogAccessState::Accepting(epoch) = inner.state {
                return Some(CatalogAdmission::Admitted(epoch));
            }
            let _wait = self
                .changed
                .wait_timeout(inner, std::time::Duration::from_millis(20))
                .unwrap();
        }
    }

    fn lease(
        self: &std::sync::Arc<Self>,
        admission: CatalogAdmission,
    ) -> rusqlite::Result<CatalogLease> {
        let mut inner = self.inner.lock().unwrap();
        if !matches!((&inner.state, admission), (CatalogAccessState::Accepting(current), CatalogAdmission::Admitted(epoch)) if *current == epoch)
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        inner.active += 1;
        Ok(CatalogLease {
            owner: self.clone(),
        })
    }

    fn close_lease(self: &std::sync::Arc<Self>) -> CatalogLease {
        // Existing registered handles may close during Deleting. Keep the close
        // in the drain count, without permitting any new SQL or connection open.
        self.inner.lock().unwrap().active += 1;
        CatalogLease {
            owner: self.clone(),
        }
    }

    fn register(&self, connection: &std::sync::Arc<CatalogConnection>) {
        let mut inner = self.inner.lock().unwrap();
        inner
            .connections
            .retain(|connection| connection.strong_count() > 0);
        inner
            .connections
            .push(std::sync::Arc::downgrade(connection));
    }

    /// Memory-only invalidation before starting the cache-maint worker.
    pub(crate) fn begin_delete(
        self: &std::sync::Arc<Self>,
        operation: CatalogDeleteOperation,
    ) -> rusqlite::Result<CatalogDeletion> {
        let mut inner = self.inner.lock().unwrap();
        let CatalogAccessState::Accepting(epoch) = inner.state else {
            return Err(rusqlite::Error::InvalidQuery);
        };
        inner.state = CatalogAccessState::Deleting {
            epoch: epoch.wrapping_add(1),
            operation,
        };
        Ok(CatalogDeletion {
            owner: self.clone(),
        })
    }
}

struct CatalogLease {
    owner: std::sync::Arc<CatalogAccess>,
}
impl Drop for CatalogLease {
    fn drop(&mut self) {
        let mut inner = self.owner.inner.lock().unwrap();
        inner.active -= 1;
        if inner.active == 0 {
            self.owner.changed.notify_all();
        }
    }
}

pub(crate) struct CatalogDeletion {
    owner: std::sync::Arc<CatalogAccess>,
}
impl CatalogDeletion {
    pub(crate) fn retire_connections(&self) {
        self.retire_connections_after_drain(|| {});
    }

    fn retire_connections_after_drain(&self, after_drain: impl FnOnce()) {
        let operation = {
            let inner = self.owner.inner.lock().unwrap();
            match &inner.state {
                CatalogAccessState::Deleting {
                    operation: CatalogDeleteOperation::All,
                    ..
                } => "all".to_owned(),
                CatalogAccessState::Deleting {
                    operation: CatalogDeleteOperation::OlderThan(days),
                    ..
                } => format!("older_than_{days}"),
                CatalogAccessState::Deleting {
                    operation: CatalogDeleteOperation::Folder(folder),
                    ..
                } => format!("folder:{}", folder.display()),
                CatalogAccessState::Accepting(_) => return,
            }
        };
        crate::logger::log(format!("catalog maintenance retire operation={operation}"));
        let connections = {
            let mut inner = self.owner.inner.lock().unwrap();
            while inner.active != 0 {
                inner = self.owner.changed.wait(inner).unwrap();
            }
            std::mem::take(&mut inner.connections)
        };
        after_drain();
        for connection in connections
            .into_iter()
            .filter_map(|connection| connection.upgrade())
        {
            let old = std::mem::replace(
                &mut *connection.state.lock().unwrap(),
                CatalogConnectionState::Retired,
            );
            drop(old); // SQLite close is worker-owned, outside both state locks.
        }
        // A final Arc can begin normal Drop after the first drain and take its
        // Connection just before retirement. That close owns a close lease;
        // wait for it before remove_file, even though the registry now sees Retired.
        let mut inner = self.owner.inner.lock().unwrap();
        while inner.active != 0 {
            inner = self.owner.changed.wait(inner).unwrap();
        }
    }
}
impl Drop for CatalogDeletion {
    fn drop(&mut self) {
        let mut inner = self.owner.inner.lock().unwrap();
        if let CatalogAccessState::Deleting { epoch, .. } = inner.state {
            inner.state = CatalogAccessState::Accepting(epoch);
        }
        self.owner.changed.notify_all();
    }
}

enum CatalogConnectionState {
    Open { connection: Connection, epoch: u64 },
    Retired,
}
struct CatalogConnection {
    state: Mutex<CatalogConnectionState>,
}
struct CatalogConnectionGuard<'a>(std::sync::MutexGuard<'a, CatalogConnectionState>);
impl std::ops::Deref for CatalogConnectionGuard<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        match &*self.0 {
            CatalogConnectionState::Open { connection, .. } => connection,
            CatalogConnectionState::Retired => {
                unreachable!("retired connection guards are never issued")
            }
        }
    }
}
impl std::ops::DerefMut for CatalogConnectionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        match &mut *self.0 {
            CatalogConnectionState::Open { connection, .. } => connection,
            CatalogConnectionState::Retired => {
                unreachable!("retired connection guards are never issued")
            }
        }
    }
}
impl CatalogConnection {
    fn new(connection: Connection, admission: CatalogAdmission) -> std::sync::Arc<Self> {
        let CatalogAdmission::Admitted(epoch) = admission else {
            unreachable!()
        };
        std::sync::Arc::new(Self {
            state: Mutex::new(CatalogConnectionState::Open { connection, epoch }),
        })
    }
    fn lock(&self) -> rusqlite::Result<CatalogConnectionGuard<'_>> {
        let guard = self.state.lock().unwrap();
        if matches!(*guard, CatalogConnectionState::Retired) {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(CatalogConnectionGuard(guard))
    }
    #[cfg(test)]
    fn try_lock(&self) -> Result<CatalogConnectionGuard<'_>, ()> {
        self.state
            .try_lock()
            .map(CatalogConnectionGuard)
            .map_err(|_| ())
    }
}

// Declaration order matters: release the connection/transaction before the lease.
struct CatalogPhase<'a> {
    connection: CatalogConnectionGuard<'a>,
    _lease: CatalogLease,
}
impl std::ops::Deref for CatalogPhase<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.connection
    }
}
impl std::ops::DerefMut for CatalogPhase<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }
}

#[derive(Clone, Debug)]
pub struct AudioArtCatalogScope {
    pub parent: PathBuf,
    prefix: String,
}
impl AudioArtCatalogScope {
    pub fn new(parent: &Path) -> Self {
        let normalized = path_key::normalize_keep_drive(parent);
        let hash = format!("{:x}", Sha256::digest(normalized.as_bytes()));
        Self {
            parent: parent.to_owned(),
            prefix: format!("audioart:{hash}:"),
        }
    }
    pub fn key_for(&self, source: &Path) -> String {
        let basename = source
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        format!("{}v1:{basename}", self.prefix)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioArtSourceStamp {
    pub mtime_secs: i64,
    pub file_size: i64,
}
impl AudioArtSourceStamp {
    pub fn read(path: &Path) -> Option<Self> {
        media_source_identity(path).map(|(mtime_secs, file_size)| Self {
            mtime_secs,
            file_size,
        })
    }
}

fn catalog_file_exists(path: &Path) -> rusqlite::Result<bool> {
    path.try_exists()
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

pub enum AudioArtCached {
    Miss,
    Pixels(CacheEntry),
    NoArt,
}

fn ensure_audio_art_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS audio_art_absence (filename TEXT NOT NULL PRIMARY KEY, mtime INTEGER NOT NULL, file_size INTEGER NOT NULL);")
}

/// Worker-only APIs return owned values and drop all handles before source work.
pub fn lookup_audio_art(
    cache_dir: &Path,
    scope: &AudioArtCatalogScope,
    key: &str,
    stamp: AudioArtSourceStamp,
    admission: CatalogAdmission,
) -> rusqlite::Result<AudioArtCached> {
    if !key.starts_with(&format!("{}v1:", scope.prefix)) {
        return Err(rusqlite::Error::InvalidQuery);
    }

    let Some(db) =
        CatalogDb::open_existing_read_only_admitted(cache_dir, &scope.parent, admission)?
    else {
        return Ok(AudioArtCached::Miss);
    };
    let conn = db.phase()?;
    let has_absence: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='audio_art_absence')", [], |row| row.get(0))?;
    let sql = if has_absence {
        "SELECT thumb_data, source_width, source_height FROM thumbnails WHERE filename=?1 AND mtime=?2 AND file_size=?3 UNION ALL SELECT NULL, NULL, NULL FROM audio_art_absence WHERE filename=?1 AND mtime=?2 AND file_size=?3 LIMIT 1"
    } else {
        "SELECT thumb_data, source_width, source_height FROM thumbnails WHERE filename=?1 AND mtime=?2 AND file_size=?3 LIMIT 1"
    };
    let row = conn
        .query_row(
            sql,
            params![key, stamp.mtime_secs, stamp.file_size],
            |row| {
                Ok((
                    row.get::<_, Option<Vec<u8>>>(0)?,
                    row.get::<_, Option<u32>>(1)?,
                    row.get::<_, Option<u32>>(2)?,
                ))
            },
        )
        .optional()?;
    Ok(match row {
        Some((Some(jpeg_data), width, height)) => AudioArtCached::Pixels(CacheEntry {
            mtime: stamp.mtime_secs,
            file_size: stamp.file_size,
            jpeg_data,
            source_dims: valid_dims(width, height),
            layout_dims: None,
            folder_provenance: None,
            selection_proof: None,
        }),
        Some((None, _, _)) => AudioArtCached::NoArt,
        None => AudioArtCached::Miss,
    })
}

pub fn save_audio_art_pixels(
    cache_dir: &Path,
    scope: &AudioArtCatalogScope,
    key: &str,
    entry: &CacheEntry,
    admission: CatalogAdmission,
    cancel: &AtomicBool,
) -> rusqlite::Result<bool> {
    save_audio_art_pixels_with_cancel_check(cache_dir, scope, key, entry, admission, &|| {
        cancel.load(Ordering::Relaxed)
    })
}

pub fn save_audio_art_pixels_with_cancel_check(
    cache_dir: &Path,
    scope: &AudioArtCatalogScope,
    key: &str,
    entry: &CacheEntry,
    admission: CatalogAdmission,
    should_cancel: &impl Fn() -> bool,
) -> rusqlite::Result<bool> {
    if !key.starts_with(&format!("{}v1:", scope.prefix)) {
        return Err(rusqlite::Error::InvalidQuery);
    }

    if should_cancel() {
        return Ok(false);
    }
    let Some((width, height)) = decode_thumb_dims(&entry.jpeg_data) else {
        return Ok(false);
    };
    let db = CatalogDb::open_admitted(cache_dir, &scope.parent, admission)?;
    let conn = db.phase()?;
    ensure_audio_art_schema(&conn)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM audio_art_absence WHERE filename=?1", [key])?;
    tx.execute("INSERT OR REPLACE INTO thumbnails (filename,mtime,file_size,width,height,thumb_data,source_width,source_height) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![key,entry.mtime,entry.file_size,width,height,entry.jpeg_data,entry.source_dims.map(|d| d.0),entry.source_dims.map(|d| d.1)])?;
    if should_cancel() {
        return Ok(false);
    }
    tx.commit()?;
    Ok(true)
}

pub fn save_audio_art_absence(
    cache_dir: &Path,
    scope: &AudioArtCatalogScope,
    key: &str,
    stamp: AudioArtSourceStamp,
    admission: CatalogAdmission,
    cancel: &AtomicBool,
) -> rusqlite::Result<bool> {
    save_audio_art_absence_with_cancel_check(cache_dir, scope, key, stamp, admission, &|| {
        cancel.load(Ordering::Relaxed)
    })
}

pub fn save_audio_art_absence_with_cancel_check(
    cache_dir: &Path,
    scope: &AudioArtCatalogScope,
    key: &str,
    stamp: AudioArtSourceStamp,
    admission: CatalogAdmission,
    should_cancel: &impl Fn() -> bool,
) -> rusqlite::Result<bool> {
    if !key.starts_with(&format!("{}v1:", scope.prefix)) {
        return Err(rusqlite::Error::InvalidQuery);
    }

    if should_cancel() {
        return Ok(false);
    }
    let db = CatalogDb::open_admitted(cache_dir, &scope.parent, admission)?;
    let conn = db.phase()?;
    ensure_audio_art_schema(&conn)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM thumbnails WHERE filename=?1", [key])?;
    tx.execute(
        "INSERT OR REPLACE INTO audio_art_absence (filename,mtime,file_size) VALUES (?1,?2,?3)",
        params![key, stamp.mtime_secs, stamp.file_size],
    )?;
    if should_cancel() {
        return Ok(false);
    }
    tx.commit()?;
    Ok(true)
}

/// Caller must supply a completed, unfaceted physical inventory for this parent.
pub fn prune_audio_art_scope(
    cache_dir: &Path,
    scope: &AudioArtCatalogScope,
    complete_inventory: &HashSet<String>,
    admission: CatalogAdmission,
    cancel: &AtomicBool,
) -> rusqlite::Result<usize> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(0);
    }
    if !catalog_file_exists(&db_path_for(cache_dir, &scope.parent))? {
        return Ok(0);
    }
    let db = CatalogDb::open_admitted(cache_dir, &scope.parent, admission)?;
    let conn = db.phase()?;
    ensure_audio_art_schema(&conn)?;
    let tx = conn.unchecked_transaction()?;
    let names = {
        let mut stmt = tx.prepare("SELECT filename FROM thumbnails WHERE substr(filename,1,?1)=?2 UNION SELECT filename FROM audio_art_absence WHERE substr(filename,1,?1)=?2")?;
        stmt.query_map(params![scope.prefix.len(), scope.prefix], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut deleted = 0;
    for key in names {
        if cancel.load(Ordering::Relaxed) {
            return Ok(0);
        }
        let basename = key
            .strip_prefix(&scope.prefix)
            .and_then(|key| key.split_once(':'))
            .map(|(_, basename)| basename);
        if basename.is_some_and(|basename| complete_inventory.contains(basename)) {
            continue;
        }
        deleted += tx.execute("DELETE FROM thumbnails WHERE filename=?1", [&key])?;
        deleted += tx.execute("DELETE FROM audio_art_absence WHERE filename=?1", [&key])?;
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(0);
    }
    tx.commit()?;
    Ok(deleted)
}

pub struct CatalogDb {
    conn: std::sync::Arc<CatalogConnection>,
    access: std::sync::Arc<CatalogAccess>,
    admission: CatalogAdmission,
    /// Immutable identity for worker-only CacheOnly continuation, not connection state.
    database_path: Option<PathBuf>,
    has_layout_dims_columns: bool,
    has_folder_proof_columns: bool,
}

/// journal mode の変更は排他ロックを要求するため、**`busy_timeout` が効かない**。SQLite は
/// デッドロックを避けるためこの競合を待たず、即 `SQLITE_BUSY` (`database is locked`) を返す。
///
/// mode はファイルに永続するので、**必要なのは作成時の 1 回だけ**。それなのに従来は開くたびに
/// 実行していて、一覧を開いた瞬間にサムネイル worker 12 本が同じ新規カタログへ殺到すると、
/// 数本が即失敗していた (2026-08-13 の実害: 一覧の 2 枚が記号のまま残った。再要求はされない)。
///
/// 既に WAL ならまず何もしない。これで 2 回目以降の open はロックを取りに行かない。変換が要る
/// ときだけ直列化し、他所で先に変換されていればそれで目的は果たされている。
fn ensure_wal_journal(conn: &Connection) -> rusqlite::Result<()> {
    if journal_mode_is_wal(conn)? {
        return Ok(());
    }
    // 同一プロセス内の競合はここで消える。別プロセスとの競合は下の再確認で拾う。
    static CONVERT: Mutex<()> = Mutex::new(());
    let _serialized = CONVERT.lock().unwrap_or_else(|error| error.into_inner());
    if journal_mode_is_wal(conn)? {
        return Ok(());
    }
    match conn.execute_batch("PRAGMA journal_mode=WAL;") {
        Ok(()) => Ok(()),
        Err(error) if journal_mode_is_wal(conn).unwrap_or(false) => {
            let _ = error;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn journal_mode_is_wal(conn: &Connection) -> rusqlite::Result<bool> {
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    Ok(mode.eq_ignore_ascii_case("wal"))
}

impl Drop for CatalogDb {
    fn drop(&mut self) {
        let _close = self.access.close_lease();
        // Keep the registered Arc alive until physical close finishes. A maintenance
        // worker can therefore upgrade it and serialize close through this mutex.
        let old = std::mem::replace(
            &mut *self.conn.state.lock().unwrap(),
            CatalogConnectionState::Retired,
        );
        drop(old);
    }
}

impl CatalogDb {
    fn phase(&self) -> rusqlite::Result<CatalogPhase<'_>> {
        let lease = self.access.lease(self.admission)?;
        let connection = self.conn.lock()?;
        if !matches!((&*connection.0, self.admission), (CatalogConnectionState::Open { epoch: current, .. }, CatalogAdmission::Admitted(epoch)) if *current == epoch)
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(CatalogPhase {
            connection,
            _lease: lease,
        })
    }

    pub fn is_retired(&self) -> bool {
        // Warm UI lookup touches only memory. Never acquire the SQLite mutex here.
        !matches!((self.access.admit(), self.admission), (CatalogAdmission::Admitted(current), CatalogAdmission::Admitted(epoch)) if current == epoch)
    }

    /// Continue one existing CacheOnly lookup after maintenance. The old handle
    /// remains permanently Retired; this temporary handle never creates/migrates
    /// a catalog and cannot promote the request to source decoding or new writes.
    pub fn resume_cache_only_after_maintenance(
        &self,
        key: &str,
        cancel: &impl Fn() -> bool,
    ) -> rusqlite::Result<Option<CacheEntry>> {
        let Some(db) = self.resume_cache_only_catalog_after_maintenance(cancel)? else {
            return Ok(None);
        };
        let entry = db.load_one(key)?;
        if cancel() {
            return Ok(None);
        }
        Ok(entry)
    }

    /// A short read-only successor for one cache-only lookup using its original
    /// database identity. It must remain on the worker and is never cached.
    pub fn resume_cache_only_catalog_after_maintenance(
        &self,
        cancel: &impl Fn() -> bool,
    ) -> rusqlite::Result<Option<Self>> {
        let Some(path) = self.database_path.as_deref() else {
            return Ok(None);
        };
        let Some(admission) = self.access.wait_read_admission_with(cancel) else {
            return Ok(None);
        };
        let _lease = self.access.lease(admission)?;
        if cancel() || !catalog_file_exists(path)? {
            return Ok(None);
        }
        let db = Self::open_read_only_at_path(path, self.access.clone(), admission)?;
        if cancel() {
            return Ok(None);
        }
        Ok(Some(db))
    }

    fn from_connection(
        connection: Connection,
        access: std::sync::Arc<CatalogAccess>,
        admission: CatalogAdmission,
        has_layout_dims_columns: bool,
        has_folder_proof_columns: bool,
    ) -> Self {
        let database_path = connection
            .path()
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        let conn = CatalogConnection::new(connection, admission);
        access.register(&conn);
        Self {
            conn,
            access,
            admission,
            database_path,
            has_layout_dims_columns,
            has_folder_proof_columns,
        }
    }

    /// Open one existing catalog for a folder-representative selection. Legacy
    /// catalogs receive only the additive revision schema, without migrations or
    /// thumbnail-row changes. A failed initialization remains readable but its
    /// revision state is Unverifiable.
    pub fn open_for_folder_selection(
        cache_dir: &Path,
        folder_path: &Path,
    ) -> Result<Option<Self>, String> {
        let access = CatalogAccess::for_cache_dir(cache_dir);
        let admission = access.admit();
        let _lease = access.lease(admission).map_err(|error| error.to_string())?;
        let db_path = db_path_for(cache_dir, folder_path);
        let metadata = match std::fs::metadata(&db_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let db = Self::open_read_only_at_path(&db_path, access.clone(), admission)
            .map_err(|error| error.to_string())?;
        if matches!(db.revision_state(), CatalogProofState::Present(_)) {
            failed_legacy_initializations()
                .lock()
                .unwrap()
                .remove(&db_path);
            return Ok(Some(db));
        }
        let stamp = CatalogFileStamp::from_path(&db_path, &metadata);
        if failed_legacy_initializations()
            .lock()
            .unwrap()
            .get(&db_path)
            == Some(&stamp)
        {
            return Ok(Some(db));
        }
        #[cfg(test)]
        {
            *legacy_init_attempts()
                .lock()
                .unwrap()
                .entry(db_path.clone())
                .or_default() += 1;
        }
        let initialization = (|| -> rusqlite::Result<()> {
            let conn = Connection::open(&db_path)?;
            conn.busy_timeout(std::time::Duration::from_secs(2))?;
            init_folder_selection_revision_schema(&conn)
        })();
        if initialization.is_ok() {
            failed_legacy_initializations()
                .lock()
                .unwrap()
                .remove(&db_path);
            return Self::open_read_only_at_path(&db_path, access.clone(), admission)
                .map(Some)
                .map_err(|error| error.to_string());
        }
        crate::logger::log(format!(
            "folder selection legacy catalog initialization failed: {}: {}",
            db_path.display(),
            initialization.unwrap_err()
        ));
        // Initialization can partially alter the schema before failing. Store
        // the post-attempt stamp so that this very attempt cannot trigger an
        // immediate retry on the next failed proof validation.
        let after = std::fs::metadata(&db_path)
            .map(|metadata| CatalogFileStamp::from_path(&db_path, &metadata))
            .unwrap_or(stamp);
        failed_legacy_initializations()
            .lock()
            .unwrap()
            .insert(db_path, after);
        Ok(Some(db))
    }

    pub fn revision_state(&self) -> CatalogProofState {
        let Ok(conn) = self.phase() else {
            return CatalogProofState::Unverifiable;
        };
        conn.query_row(
            "SELECT instance_id, revision, ready FROM folder_selection_revision WHERE singleton = 1",
            [],
            |row| {
                let instance_id: String = row.get(0)?;
                let revision: i64 = row.get(1)?;
                let ready: i64 = row.get(2)?;
                Ok((ready == 1).then(|| CatalogRevision {
                    instance_id,
                    revision,
                }))
            },
        )
        .ok()
        .flatten()
        .map(CatalogProofState::Present)
        .unwrap_or(CatalogProofState::Unverifiable)
    }

    /// One candidate stamp and the catalog revision from the same short SQLite
    /// snapshot. The resolver calls this only as it reaches a candidate, which
    /// lets it check cancellation and stop before later rows are queried.
    pub fn load_stamp_snapshot(
        &self,
        key: &str,
    ) -> rusqlite::Result<(CatalogProofState, Option<(i64, i64)>)> {
        let conn = self.phase()?;
        let tx = conn.unchecked_transaction()?;
        let stamp = tx
            .query_row(
                "SELECT mtime, file_size FROM thumbnails WHERE filename = ?1",
                [key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let revision = tx
            .query_row(
                "SELECT instance_id, revision, ready FROM folder_selection_revision WHERE singleton = 1",
                [],
                |row| {
                    let instance_id: String = row.get(0)?;
                    let revision: i64 = row.get(1)?;
                    let ready: i64 = row.get(2)?;
                    Ok((ready == 1).then(|| CatalogRevision {
                        instance_id,
                        revision,
                    }))
                },
            )
            .ok()
            .flatten()
            .map(CatalogProofState::Present)
            .unwrap_or(CatalogProofState::Unverifiable);
        tx.commit()?;
        Ok((revision, stamp))
    }
    /// cache_dir 配下の適切な場所に DB を開く（なければ作成）。
    /// サブディレクトリも自動作成する。
    pub fn open(cache_dir: &Path, folder_path: &Path) -> rusqlite::Result<Self> {
        let access = CatalogAccess::for_cache_dir(cache_dir);
        Self::open_admitted(cache_dir, folder_path, access.admit())
    }

    pub fn open_admitted(
        cache_dir: &Path,
        folder_path: &Path,
        admission: CatalogAdmission,
    ) -> rusqlite::Result<Self> {
        let access = CatalogAccess::for_cache_dir(cache_dir);
        let _lease = access.lease(admission)?;
        let db_path = db_path_for(cache_dir, folder_path);
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mut conn = Connection::open(&db_path)?;
        ensure_wal_journal(&conn)?;
        conn.execute_batch("PRAGMA synchronous=NORMAL;")?;
        init_schema(&conn)?;
        migrate_pdf_layout_dims(&mut conn, folder_path)?;
        Ok(Self::from_connection(
            conn,
            access.clone(),
            admission,
            true,
            true,
        ))
    }

    /// 既存 catalog だけを読み取り専用で開く。ファイル不在では空 DB を作らず Ok(None)。
    /// 呼び出し元はサムネイル重 I/O worker に限定し、UI スレッドから cold open しないこと。
    pub fn open_existing_read_only(
        cache_dir: &Path,
        folder_path: &Path,
    ) -> rusqlite::Result<Option<Self>> {
        let access = CatalogAccess::for_cache_dir(cache_dir);
        Self::open_existing_read_only_admitted(cache_dir, folder_path, access.admit())
    }

    pub fn open_existing_read_only_admitted(
        cache_dir: &Path,
        folder_path: &Path,
        admission: CatalogAdmission,
    ) -> rusqlite::Result<Option<Self>> {
        let access = CatalogAccess::for_cache_dir(cache_dir);
        let _lease = access.lease(admission)?;
        let db_path = db_path_for(cache_dir, folder_path);
        if !catalog_file_exists(&db_path)? {
            return Ok(None);
        }
        Self::open_read_only_at_path(&db_path, access.clone(), admission).map(Some)
    }

    fn open_read_only_at_path(
        db_path: &Path,
        access: std::sync::Arc<CatalogAccess>,
        admission: CatalogAdmission,
    ) -> rusqlite::Result<Self> {
        let _lease = access.lease(admission)?;
        let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let has_layout_dims_columns = thumbnail_column_exists(&conn, "layout_width")?
            && thumbnail_column_exists(&conn, "layout_height")?;
        let has_folder_proof_columns = thumbnail_column_exists(&conn, "folder_provenance")?
            && thumbnail_column_exists(&conn, "selection_proof")?;
        Ok(Self::from_connection(
            conn,
            access.clone(),
            admission,
            has_layout_dims_columns,
            has_folder_proof_columns,
        ))
    }

    /// `load_all` は `thumb_data` も SELECT する。実測で 1 行あたり平均 35 KiB あり
    /// (4,628 枚のカタログで 157 MiB)、5 万枚のフォルダでは寸法を知るためだけに
    /// 1.7 GB を運ぶことになる。整数列だけで答えられる問いにはこちらを使う。
    ///
    /// 値の `None` は「行はあるが寸法列が NULL」(寸法列より前に保存された古いエントリ) を、
    /// key の不在は「行が無い」を表す。blob からの復元が要る呼び出し側は、前者のときだけ
    /// `load_one` で個別に取り直す。
    pub fn load_source_dims(&self) -> rusqlite::Result<HashMap<String, Option<(u32, u32)>>> {
        let conn = self.phase()?;
        let mut stmt =
            conn.prepare("SELECT filename, source_width, source_height FROM thumbnails WHERE substr(filename,1,9) <> 'audioart:'")?;
        let mut map = HashMap::new();
        let iter = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<u32>>(1)?,
                row.get::<_, Option<u32>>(2)?,
            ))
        })?;
        for (filename, width, height) in iter.flatten() {
            map.insert(filename, valid_dims(width, height));
        }
        Ok(map)
    }

    /// EPUB page dimensions belonging to one immutable converted generation.
    /// The same integer columns hold PDF file attributes in ordinary catalogs.
    pub fn load_source_dims_matching(
        &self,
        mtime: i64,
        file_size: i64,
    ) -> rusqlite::Result<HashMap<String, Option<(u32, u32)>>> {
        let conn = self.phase()?;
        let mut stmt = conn.prepare(
            "SELECT filename, source_width, source_height FROM thumbnails \
             WHERE mtime = ?1 AND file_size = ?2 AND substr(filename,1,9) <> 'audioart:'",
        )?;
        let mut map = HashMap::new();
        let iter = stmt.query_map(rusqlite::params![mtime, file_size], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<u32>>(1)?,
                row.get::<_, Option<u32>>(2)?,
            ))
        })?;
        for (filename, width, height) in iter.flatten() {
            map.insert(filename, valid_dims(width, height));
        }
        Ok(map)
    }

    /// DB 内の全エントリを HashMap<filename, CacheEntry> として返す（一括 SELECT）。
    pub fn load_all(&self) -> rusqlite::Result<HashMap<String, CacheEntry>> {
        let conn = self.phase()?;
        let layout_columns = if self.has_layout_dims_columns {
            "layout_width, layout_height"
        } else {
            "NULL, NULL"
        };
        let proof_columns = if self.has_folder_proof_columns {
            "folder_provenance, selection_proof"
        } else {
            "NULL, NULL"
        };
        let sql = format!(
            "SELECT filename, mtime, file_size, thumb_data, source_width, source_height, \
                    {layout_columns}, {proof_columns} FROM thumbnails WHERE substr(filename,1,9) <> 'audioart:'"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut map = HashMap::new();
        let iter = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Option<u32>>(4)?,
                row.get::<_, Option<u32>>(5)?,
                row.get::<_, Option<u32>>(6)?,
                row.get::<_, Option<u32>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })?;
        for item in iter.flatten() {
            let (
                filename,
                mtime,
                file_size,
                jpeg_data,
                src_w,
                src_h,
                layout_w,
                layout_h,
                provenance,
                proof,
            ) = item;
            let source_dims = match (src_w, src_h) {
                (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
                _ => None,
            };
            map.insert(
                filename,
                CacheEntry {
                    mtime,
                    file_size,
                    jpeg_data,
                    source_dims,
                    layout_dims: valid_dims(layout_w, layout_h),
                    folder_provenance: provenance
                        .and_then(|value| serde_json::from_str(&value).ok()),
                    selection_proof: proof.and_then(|value| serde_json::from_str(&value).ok()),
                },
            );
        }
        Ok(map)
    }

    /// 単一エントリのみ取り出す。`load_all` を呼ぶほどではないが特定 key だけ確認したい
    /// 場合用 (例: 仮想フォルダ進入時の親 catalog からの seed lookup)。
    pub fn load_one(&self, filename: &str) -> rusqlite::Result<Option<CacheEntry>> {
        let conn = self.phase()?;
        let layout_columns = if self.has_layout_dims_columns {
            "layout_width, layout_height"
        } else {
            "NULL, NULL"
        };
        let proof_columns = if self.has_folder_proof_columns {
            "folder_provenance, selection_proof"
        } else {
            "NULL, NULL"
        };
        let sql = format!(
            "SELECT mtime, file_size, thumb_data, source_width, source_height, \
                    {layout_columns}, {proof_columns} FROM thumbnails WHERE filename = ?1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut iter = stmt.query_map(params![filename], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Option<u32>>(3)?,
                row.get::<_, Option<u32>>(4)?,
                row.get::<_, Option<u32>>(5)?,
                row.get::<_, Option<u32>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })?;
        if let Some(item) = iter.next() {
            let (mtime, file_size, jpeg_data, src_w, src_h, layout_w, layout_h, provenance, proof) =
                item?;
            let source_dims = match (src_w, src_h) {
                (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
                _ => None,
            };
            return Ok(Some(CacheEntry {
                mtime,
                file_size,
                jpeg_data,
                source_dims,
                layout_dims: valid_dims(layout_w, layout_h),
                folder_provenance: provenance.and_then(|value| serde_json::from_str(&value).ok()),
                selection_proof: proof.and_then(|value| serde_json::from_str(&value).ok()),
            }));
        }
        Ok(None)
    }

    /// `filename` が `prefix` で始まるエントリのうち、mtime / size が最も新しいものを
    /// 1 件だけ返す。フォルダ代表サムネのように base key と `#pin:` 派生 key の
    /// どちらにも既存サムネが残り得る場合の cache-only 参照に使う。
    pub fn load_latest_with_prefix(
        &self,
        prefix: &str,
    ) -> rusqlite::Result<Option<(String, CacheEntry)>> {
        self.load_latest_with_prefix_matching(prefix, None)
    }

    /// As above, but generation validation happens before selecting a winner.
    pub fn load_latest_with_prefix_matching(
        &self,
        prefix: &str,
        stamp: Option<(i64, i64)>,
    ) -> rusqlite::Result<Option<(String, CacheEntry)>> {
        let conn = self.phase()?;
        let layout_columns = if self.has_layout_dims_columns {
            "layout_width, layout_height"
        } else {
            "NULL, NULL"
        };
        let proof_columns = if self.has_folder_proof_columns {
            "folder_provenance, selection_proof"
        } else {
            "NULL, NULL"
        };
        let sql = format!(
            "SELECT filename, mtime, file_size, thumb_data, source_width, source_height, \
                    {layout_columns}, {proof_columns} FROM thumbnails \
             WHERE substr(filename, 1, ?1) = ?2 AND (?3 IS NULL OR (mtime = ?3 AND file_size = ?4)) \
             ORDER BY mtime DESC, file_size DESC, filename DESC \
             LIMIT 1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut iter = stmt.query_map(
            params![
                prefix.chars().count() as i64,
                prefix,
                stamp.map(|stamp| stamp.0),
                stamp.map(|stamp| stamp.1)
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Option<u32>>(4)?,
                    row.get::<_, Option<u32>>(5)?,
                    row.get::<_, Option<u32>>(6)?,
                    row.get::<_, Option<u32>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )?;
        if let Some(item) = iter.next() {
            let (
                filename,
                mtime,
                file_size,
                jpeg_data,
                src_w,
                src_h,
                layout_w,
                layout_h,
                provenance,
                proof,
            ) = item?;
            let source_dims = match (src_w, src_h) {
                (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
                _ => None,
            };
            return Ok(Some((
                filename,
                CacheEntry {
                    mtime,
                    file_size,
                    jpeg_data,
                    source_dims,
                    layout_dims: valid_dims(layout_w, layout_h),
                    folder_provenance: provenance
                        .and_then(|value| serde_json::from_str(&value).ok()),
                    selection_proof: proof.and_then(|value| serde_json::from_str(&value).ok()),
                },
            )));
        }
        Ok(None)
    }

    /// サムネイルを INSERT OR REPLACE で保存する。
    ///
    /// `width` / `height` はキャッシュされる WebP サムネイルの寸法、
    /// `source_dims` は元画像 / PDF raster のピクセル寸法 (未取得なら None)。
    #[allow(clippy::too_many_arguments)]
    pub fn save(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        width: u32,
        height: u32,
        source_dims: Option<(u32, u32)>,
        jpeg_data: &[u8],
    ) -> rusqlite::Result<()> {
        self.save_with_layout_dims(
            filename,
            mtime,
            file_size,
            width,
            height,
            source_dims,
            None,
            jpeg_data,
        )
    }

    /// `save` に raster と独立したレイアウト寸法を付加する PDF 用保存経路。
    #[allow(clippy::too_many_arguments)]
    pub fn save_with_layout_dims(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        width: u32,
        height: u32,
        source_dims: Option<(u32, u32)>,
        layout_dims: Option<(u32, u32)>,
        jpeg_data: &[u8],
    ) -> rusqlite::Result<()> {
        self.save_with_folder_proof(
            filename,
            mtime,
            file_size,
            width,
            height,
            source_dims,
            layout_dims,
            jpeg_data,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save_with_folder_proof(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        width: u32,
        height: u32,
        source_dims: Option<(u32, u32)>,
        layout_dims: Option<(u32, u32)>,
        jpeg_data: &[u8],
        provenance: Option<FolderThumbProvenance>,
        proof: Option<&FolderSelectionProof>,
    ) -> rusqlite::Result<()> {
        let conn = self.phase()?;
        let src_w: Option<u32> = source_dims.map(|(w, _)| w);
        let src_h: Option<u32> = source_dims.map(|(_, h)| h);
        let layout_w: Option<u32> = layout_dims.map(|(w, _)| w);
        let layout_h: Option<u32> = layout_dims.map(|(_, h)| h);
        let provenance_json = provenance
            .map(|value| serde_json::to_string(&value))
            .transpose()
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let proof_json = proof
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        conn.execute(
            "INSERT OR REPLACE INTO thumbnails \
              (filename, mtime, file_size, width, height, thumb_data, source_width, source_height, \
               layout_width, layout_height, folder_provenance, selection_proof) \
              VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                filename,
                mtime,
                file_size,
                width,
                height,
                jpeg_data,
                src_w,
                src_h,
                layout_w,
                layout_h,
                provenance_json,
                proof_json
            ],
        )?;
        Ok(())
    }

    /// サムネバイト列のヘッダから `(w, h)` だけを取り出して `save` する薄いラッパ。
    /// `CacheEntry` には寸法フィールドが無いため、save 経由では `(w, h)` を呼び出し側で
    /// 用意する必要がある。仮想フォルダの seed / write-back のように「親 catalog から
    /// バイトをそのままミラーする」用途で繰り返し書きがちなので集約した。
    /// ヘッダのみ解析なのでフルデコードは走らない。
    ///
    /// 戻り値の `bool` は「実際に保存できたか」。`false` は「寸法を取り出せず保存を断念
    /// した」を意味する (= 壊れたバイト列)。呼び出し側はこれをもとに「cache_map にも
    /// 入れない」ことで、サムネ表示時に `Failed` 状態に陥るのを防げる。
    pub fn save_thumb_bytes(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        source_dims: Option<(u32, u32)>,
        jpeg_data: &[u8],
    ) -> rusqlite::Result<bool> {
        self.save_thumb_bytes_with_layout_dims(
            filename,
            mtime,
            file_size,
            source_dims,
            None,
            jpeg_data,
        )
    }

    /// `save_thumb_bytes` の PDF レイアウト寸法付き版。
    pub fn save_thumb_bytes_with_layout_dims(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        source_dims: Option<(u32, u32)>,
        layout_dims: Option<(u32, u32)>,
        jpeg_data: &[u8],
    ) -> rusqlite::Result<bool> {
        let Some((w, h)) = decode_thumb_dims(jpeg_data) else {
            // 寸法が取れない (= 壊れたバイト列) なら保存を断念。SQLite スキーマ上
            // width/height は NOT NULL なので 0 を入れると整合性が壊れる。
            return Ok(false);
        };
        self.save_with_layout_dims(
            filename,
            mtime,
            file_size,
            w,
            h,
            source_dims,
            layout_dims,
            jpeg_data,
        )?;
        Ok(true)
    }

    pub fn save_auto_folder_bytes(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        source_dims: Option<(u32, u32)>,
        layout_dims: Option<(u32, u32)>,
        data: &[u8],
        proof: Option<&FolderSelectionProof>,
    ) -> rusqlite::Result<bool> {
        let Some((width, height)) = decode_thumb_dims(data) else {
            return Ok(false);
        };
        self.save_with_folder_proof(
            filename,
            mtime,
            file_size,
            width,
            height,
            source_dims,
            layout_dims,
            data,
            Some(FolderThumbProvenance::AutoSelected),
            proof,
        )?;
        Ok(true)
    }

    pub fn save_seeded_folder_bytes(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        source_dims: Option<(u32, u32)>,
        data: &[u8],
    ) -> rusqlite::Result<bool> {
        let Some((width, height)) = decode_thumb_dims(data) else {
            return Ok(false);
        };
        self.save_with_folder_proof(
            filename,
            mtime,
            file_size,
            width,
            height,
            source_dims,
            None,
            data,
            Some(FolderThumbProvenance::Seeded),
            None,
        )?;
        Ok(true)
    }

    /// 単一エントリを `filename` キーで削除する。該当行が無くてもエラーにしない。
    ///
    /// 用途: フォルダ代表ピンが Video を指していたが対応する `video_pins` の WebP が
    /// 消えた / 空になった場合、`folderthumb:{dir}#pin:...` のキャッシュ行を明示的に
    /// 削除して worker を auto-pick fallback に落とすため (Codex Phase C P2 指摘)。
    pub fn delete_one(&self, filename: &str) -> rusqlite::Result<()> {
        let conn = self.phase()?;
        conn.execute(
            "DELETE FROM thumbnails WHERE filename = ?1",
            params![filename],
        )?;
        Ok(())
    }

    /// Worker-owned pin refresh publication. Check cancellation inside the catalog write
    /// boundary so a superseded waiter cannot overwrite a newer refresh's video seed.
    pub(crate) fn commit_pin_materializations(
        &self,
        deletes: &[String],
        seeds: &[(String, CacheEntry)],
        cancel: &std::sync::atomic::AtomicBool,
    ) -> rusqlite::Result<bool> {
        use std::sync::atomic::Ordering;
        let conn = self.phase()?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let tx = conn.unchecked_transaction()?;
        for key in deletes {
            if cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            tx.execute("DELETE FROM thumbnails WHERE filename = ?1", params![key])?;
        }
        for (key, entry) in seeds {
            if cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            let Some((width, height)) = decode_thumb_dims(&entry.jpeg_data) else {
                continue;
            };
            tx.execute(
                "INSERT OR REPLACE INTO thumbnails \
                 (filename, mtime, file_size, width, height, thumb_data, source_width, source_height, \
                  layout_width, layout_height, folder_provenance, selection_proof) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, NULL, NULL, ?7, NULL)",
                params![key, entry.mtime, entry.file_size, width, height, entry.jpeg_data,
                    serde_json::to_string(&FolderThumbProvenance::Seeded).unwrap()],
            )?;
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        tx.commit()?;
        Ok(true)
    }

    /// `existing` に含まれないファイル名の行を削除する（削除済みファイルの掃除）。
    pub fn delete_missing(&self, existing: &HashSet<String>) -> rusqlite::Result<()> {
        let conn = self.phase()?;
        let db_names: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT filename FROM thumbnails WHERE substr(filename,1,9) <> 'audioart:'",
            )?;
            stmt.query_map([], |r| r.get(0))?.flatten().collect()
        };
        for name in db_names {
            if !existing.contains(&name) {
                conn.execute("DELETE FROM thumbnails WHERE filename = ?1", params![name])?;
            }
        }
        Ok(())
    }

    // -------------------------------------------------------------------
    // PDF ページ数メタキャッシュ (v1.0.0)
    //
    // load_pdf_as_folder で Enter→ページ一覧の体感を瞬時にするため、PDFium による
    // PDF open + 構造解析 (warm 5-30ms / cold 100-1300ms) の結果をフォルダごとの
    // catalog DB に永続化する。lookup 時に mtime/file_size が一致すれば cache hit
    // とみなし、即座に N セルの placeholder grid を立てる (= 824ms 待ちを回避)。
    //
    // `password_required` は「最後に成功した enumerate がパスワード保護下だったか」
    // を記録する。パスワード保存なしで cache hit のグリッドを見せると、後で保存
    // パスワードが削除された場合に保護を bypass してしまうため、cache 利用前に
    // `password_required==1 && pdf_passwords にエントリ無し` の組み合わせを
    // 明示的に弾く (Codex P1 対応)。
    // -------------------------------------------------------------------

    /// PDF メタキャッシュをルックアップする。
    ///
    /// `(filename, mtime, file_size)` が完全に一致した場合のみ `Some((page_count,
    /// password_required))` を返す。mtime/file_size 不一致は cache miss (None)。
    /// `password_required == true` の場合、呼び出し側は更に「保存パスワードがある」
    /// ことを確認してから cache を利用すること。
    pub fn get_pdf_meta(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
    ) -> rusqlite::Result<Option<(u32, bool)>> {
        let conn = self.phase()?;
        let mut stmt = conn.prepare(
            "SELECT page_count, password_required FROM pdf_meta \
             WHERE filename = ?1 AND mtime = ?2 AND file_size = ?3",
        )?;
        let result = stmt
            .query_row(params![filename, mtime, file_size], |r| {
                let page_count: i64 = r.get(0)?;
                let pw_req: i64 = r.get(1)?;
                Ok((page_count.max(0) as u32, pw_req != 0))
            })
            .ok();
        Ok(result)
    }

    /// PDF メタキャッシュを INSERT OR REPLACE する。
    /// `page_count == 0` のような無効値もそのまま記録 (= 後で stale 検出に使える)。
    /// `password_required` は呼び出し側が「この PDF 固有の保存パスワードが必要」と
    /// 確信している場合だけ true を渡すこと。session 経由の暫定パスワードでは
    /// `set_pdf_meta_thumb` 側を使う (既存値を保持する)。
    pub fn set_pdf_meta(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        page_count: u32,
        password_required: bool,
    ) -> rusqlite::Result<()> {
        let conn = self.phase()?;
        conn.execute(
            "INSERT OR REPLACE INTO pdf_meta \
             (filename, mtime, file_size, page_count, password_required) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                filename,
                mtime,
                file_size,
                page_count as i64,
                if password_required { 1i64 } else { 0i64 },
            ],
        )?;
        Ok(())
    }

    /// 既存 `pdf_meta` 行が **同じ mtime/file_size のとき** のみ `page_count` を更新する。
    /// それ以外 (新規行 / mtime or file_size 変化) は no-op。
    ///
    /// 用途: password=Some だが「この PDF の保存パスワードがある」とは確信できない
    /// 経路 (= session-level の `pdf_current_password` が居座っているだけかもしれない、
    /// or ユーザーがダイアログで入力したが「保存しない」を選んだ)。
    ///
    /// **mtime/file_size 一致条件の理由 (Codex P1 round 3 対応)**:
    /// 単純な `UPDATE WHERE filename=?` だと、stale な「非暗号化版」の行が、暗号化版に
    /// ファイル置換された後の UPDATE で新 mtime/size を被って lookup hit するようになる
    /// → password_required=0 が保持されたまま placeholder で bypass される。
    /// mtime/file_size が既存行と一致するときだけ更新することで、
    ///   - ファイル不変 (= mtime/size 同じ) → 既存 password_required の確信を保ったまま
    ///     page_count を verify update (実質 no-op になることが多い)
    ///   - ファイル変化 (= mtime/size 違う) → no-op、stale 行はそのまま放置。次回 lookup
    ///     で mtime mismatch して miss するので、確信あり経路で改めて書き直される
    /// が成立する。
    pub fn set_pdf_meta_thumb(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        page_count: u32,
    ) -> rusqlite::Result<()> {
        let conn = self.phase()?;
        conn.execute(
            "UPDATE pdf_meta \
             SET page_count = ?4 \
             WHERE filename = ?1 AND mtime = ?2 AND file_size = ?3",
            params![filename, mtime, file_size, page_count as i64],
        )?;
        Ok(())
    }

    /// `password 不要が確信できる場合**用の UPSERT。
    /// 新規行・既存行とも `password_required=0` で書き込み、
    /// `page_count`/`mtime`/`file_size` を更新する。
    ///
    /// 用途: サムネワーカーが `pdf_password=None` で render に成功した場合 (=
    /// PDFium 側で「パスワード不要」と判明した = 確信あり)。
    ///
    /// **既存行の `password_required` も上書きする理由 (review #1 対応)**:
    /// 呼び出し側の不変条件「password=None で render 成功」が成立しているので、
    /// (filename, mtime, file_size) の組合せで指している今のファイルは確実に
    /// 非保護。既存行 `password_required=1` を保持してしまうと、保護版を
    /// 非保護版に差し替えた場合に永続的に「保護扱い」が残り、placeholder grid が
    /// 表示できず無意味なパスワード入力ダイアログを毎回開く羽目になる。
    /// 「render が None で通った時点で password_required は 0 と判明した」事実を
    /// そのまま反映する。
    pub fn set_pdf_meta_safe(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        page_count: u32,
    ) -> rusqlite::Result<()> {
        let conn = self.phase()?;
        conn.execute(
            "INSERT INTO pdf_meta \
             (filename, mtime, file_size, page_count, password_required) \
             VALUES (?1, ?2, ?3, ?4, 0) \
             ON CONFLICT(filename) DO UPDATE SET \
               mtime = excluded.mtime, \
               file_size = excluded.file_size, \
               page_count = excluded.page_count, \
               password_required = 0",
            params![filename, mtime, file_size, page_count as i64],
        )?;
        Ok(())
    }

    /// ZIP / 画像のみフォルダ / 変換対象アーカイブのページ数を、内容 identity と判定設定
    /// fingerprint が完全一致するときだけ返す。失敗結果は保存せず、`page_count=NULL` はフォルダを
    /// 正常に走査した結果「本として扱う対象外」だったことを表す。
    pub fn get_container_page_meta(
        &self,
        filename: &str,
        kind: ContainerPageKind,
        mtime: i64,
        file_size: i64,
        fingerprint: i64,
    ) -> rusqlite::Result<Option<ContainerPageMeta>> {
        let conn = self.phase()?;
        let mut stmt = conn.prepare(
            "SELECT page_count FROM container_page_meta \
             WHERE filename = ?1 AND kind = ?2 AND mtime = ?3 \
               AND file_size = ?4 AND fingerprint = ?5",
        )?;
        stmt.query_row(
            params![filename, kind as i64, mtime, file_size, fingerprint],
            |row| {
                let count: Option<i64> = row.get(0)?;
                Ok(ContainerPageMeta {
                    page_count: count.map(|value| value.max(0) as u32),
                })
            },
        )
        .optional()
    }

    pub fn get_video_meta(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
    ) -> rusqlite::Result<Option<VideoMeta>> {
        let conn = self.phase()?;
        conn.query_row(
            "SELECT readable, duration_secs, width, height, codec FROM video_meta \
             WHERE filename = ?1 AND mtime = ?2 AND file_size = ?3",
            params![filename, mtime, file_size],
            |row| {
                let readable: bool = row.get(0)?;
                if !readable {
                    return Ok(VideoMeta::Unreadable);
                }
                Ok(VideoMeta::Read {
                    duration_secs: row.get(1)?,
                    dims: valid_dims(row.get(2)?, row.get(3)?),
                    codec: row.get(4)?,
                })
            },
        )
        .optional()
    }

    /// Recheck the source without holding a catalog mutex or SQLite write lock.
    /// Cancellation is checked around the OS stat, which cannot be interrupted.
    pub fn set_video_meta(
        &self,
        source_path: &Path,
        filename: &str,
        mtime: i64,
        file_size: i64,
        meta: &VideoMeta,
        cancel: &AtomicBool,
    ) -> rusqlite::Result<bool> {
        self.set_video_meta_with_source_check(filename, mtime, file_size, meta, cancel, || {
            media_source_identity(source_path)
        })
    }

    fn set_video_meta_with_source_check(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        meta: &VideoMeta,
        cancel: &AtomicBool,
        source_check: impl FnOnce() -> Option<(i64, i64)>,
    ) -> rusqlite::Result<bool> {
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let identity = source_check();
        if cancel.load(Ordering::Relaxed) || identity != Some((mtime, file_size)) {
            return Ok(false);
        }
        let (readable, duration_secs, dims, codec) = match meta {
            VideoMeta::Read {
                duration_secs,
                dims,
                codec,
            } => (true, *duration_secs, *dims, codec.as_deref()),
            VideoMeta::Unreadable => (false, None, None, None),
        };
        let mut conn = self.phase()?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        // The IMMEDIATE transaction contains only SQL writes. A source change
        // after the stat may let this result replace a newer worker's row, but
        // lookups require exact mtime/size equality: that stale row is a miss,
        // costing one later probe rather than publishing incorrect metadata.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO video_meta \
             (filename, mtime, file_size, readable, duration_secs, width, height, codec) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(filename) DO UPDATE SET \
               mtime = excluded.mtime, file_size = excluded.file_size, \
               readable = excluded.readable, duration_secs = excluded.duration_secs, \
               width = excluded.width, height = excluded.height, codec = excluded.codec",
            params![
                filename,
                mtime,
                file_size,
                readable,
                duration_secs,
                dims.map(|(w, _)| w),
                dims.map(|(_, h)| h),
                codec
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn set_container_page_meta(
        &self,
        filename: &str,
        kind: ContainerPageKind,
        mtime: i64,
        file_size: i64,
        fingerprint: i64,
        page_count: Option<u32>,
    ) -> rusqlite::Result<()> {
        let conn = self.phase()?;
        conn.execute(
            "INSERT INTO container_page_meta \
             (filename, kind, mtime, file_size, fingerprint, page_count) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(filename, kind) DO UPDATE SET \
               mtime = excluded.mtime, file_size = excluded.file_size, \
               fingerprint = excluded.fingerprint, page_count = excluded.page_count",
            params![
                filename,
                kind as i64,
                mtime,
                file_size,
                fingerprint,
                page_count.map(i64::from),
            ],
        )?;
        Ok(())
    }
}

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS thumbnails (
             filename       TEXT    NOT NULL PRIMARY KEY,
             mtime          INTEGER NOT NULL,
             file_size      INTEGER NOT NULL,
             width          INTEGER NOT NULL,
             height         INTEGER NOT NULL,
             thumb_data     BLOB    NOT NULL,
             source_width   INTEGER,
             source_height  INTEGER,
             layout_width   INTEGER,
              layout_height  INTEGER,
              folder_provenance TEXT,
              selection_proof TEXT
         );
         CREATE TABLE IF NOT EXISTS pdf_meta (
             filename          TEXT    NOT NULL PRIMARY KEY,
             mtime             INTEGER NOT NULL,
             file_size         INTEGER NOT NULL,
             page_count        INTEGER NOT NULL,
             password_required INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE IF NOT EXISTS container_page_meta (
             filename       TEXT    NOT NULL,
             kind           INTEGER NOT NULL,
             mtime          INTEGER NOT NULL,
             file_size      INTEGER NOT NULL,
             fingerprint    INTEGER NOT NULL,
             page_count     INTEGER,
             PRIMARY KEY(filename, kind)
         );
         CREATE TABLE IF NOT EXISTS video_meta (
             filename       TEXT    NOT NULL PRIMARY KEY,
             mtime          INTEGER NOT NULL,
             file_size      INTEGER NOT NULL,
             readable       INTEGER NOT NULL,
             duration_secs  REAL,
             width          INTEGER,
             height         INTEGER,
             codec          TEXT
         );",
    )?;
    // Trigger ownership is at the catalog layer. Install before any migration or
    // delete_missing can mutate thumbnail rows.
    init_folder_selection_revision_schema(conn)?;
    // 非破壊マイグレーション。open ごとの ALTER 失敗ログを避け、並行 open が同時に
    // missing を観測した場合だけ duplicate column を idempotent success として扱う。
    add_thumbnail_column_if_missing(
        conn,
        "source_width",
        "ALTER TABLE thumbnails ADD COLUMN source_width INTEGER",
    )?;
    add_thumbnail_column_if_missing(
        conn,
        "source_height",
        "ALTER TABLE thumbnails ADD COLUMN source_height INTEGER",
    )?;
    add_thumbnail_column_if_missing(
        conn,
        "layout_width",
        "ALTER TABLE thumbnails ADD COLUMN layout_width INTEGER",
    )?;
    add_thumbnail_column_if_missing(
        conn,
        "layout_height",
        "ALTER TABLE thumbnails ADD COLUMN layout_height INTEGER",
    )?;
    add_thumbnail_column_if_missing(
        conn,
        "folder_provenance",
        "ALTER TABLE thumbnails ADD COLUMN folder_provenance TEXT",
    )?;
    add_thumbnail_column_if_missing(
        conn,
        "selection_proof",
        "ALTER TABLE thumbnails ADD COLUMN selection_proof TEXT",
    )?;

    // バージョン不一致（スキーマ変更）の場合は全削除して再生成
    let version: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| {
            r.get(0)
        })
        .ok();
    if version.as_deref() != Some(CATALOG_VERSION) {
        conn.execute_batch("DELETE FROM thumbnails;")?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('version', ?1)",
            params![CATALOG_VERSION],
        )?;
    }
    Ok(())
}

fn init_folder_selection_revision_schema(conn: &Connection) -> rusqlite::Result<()> {
    if folder_selection_revision_ready(conn)? {
        return Ok(());
    }
    // Serialize first-time setup in this process. The ready marker is set only
    // after all triggers exist; interrupted setup is retried on the next open.
    static INIT: Mutex<()> = Mutex::new(());
    let _guard = INIT.lock().unwrap_or_else(|error| error.into_inner());
    if folder_selection_revision_ready(conn)? {
        return Ok(());
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS folder_selection_revision (
             singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
             instance_id TEXT NOT NULL,
             revision INTEGER NOT NULL,
             ready INTEGER NOT NULL DEFAULT 0
         );",
    )?;
    let has_ready = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('folder_selection_revision') WHERE name = 'ready' LIMIT 1",
            [], |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !has_ready {
        match conn.execute(
            "ALTER TABLE folder_selection_revision ADD COLUMN ready INTEGER NOT NULL DEFAULT 0",
            [],
        ) {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(_, Some(message)))
                if message.contains("duplicate column name") => {}
            Err(error) => return Err(error),
        }
    }
    conn.execute(
        "INSERT OR IGNORE INTO folder_selection_revision (singleton, instance_id, revision, ready)
         VALUES (1, ?1, 0, 0)",
        [uuid::Uuid::new_v4().to_string()],
    )?;
    conn.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS folder_selection_revision_insert
         AFTER INSERT ON thumbnails
         WHEN NEW.filename LIKE 'zipthumb:%' OR NEW.filename LIKE 'pdfthumb:%'
              OR (NEW.filename LIKE 'folderthumb:%' AND instr(NEW.filename, '#pin:') > 0)
         BEGIN
             UPDATE folder_selection_revision SET revision = revision + 1 WHERE singleton = 1;
         END;
         CREATE TRIGGER IF NOT EXISTS folder_selection_revision_update
         AFTER UPDATE ON thumbnails
         WHEN NEW.filename LIKE 'zipthumb:%' OR NEW.filename LIKE 'pdfthumb:%'
              OR OLD.filename LIKE 'zipthumb:%' OR OLD.filename LIKE 'pdfthumb:%'
              OR (NEW.filename LIKE 'folderthumb:%' AND instr(NEW.filename, '#pin:') > 0)
              OR (OLD.filename LIKE 'folderthumb:%' AND instr(OLD.filename, '#pin:') > 0)
         BEGIN
             UPDATE folder_selection_revision SET revision = revision + 1 WHERE singleton = 1;
         END;
         CREATE TRIGGER IF NOT EXISTS folder_selection_revision_delete
         AFTER DELETE ON thumbnails
         WHEN OLD.filename LIKE 'zipthumb:%' OR OLD.filename LIKE 'pdfthumb:%'
              OR (OLD.filename LIKE 'folderthumb:%' AND instr(OLD.filename, '#pin:') > 0)
         BEGIN
             UPDATE folder_selection_revision SET revision = revision + 1 WHERE singleton = 1;
         END;",
    )?;
    conn.execute(
        "UPDATE folder_selection_revision SET ready = 1 WHERE singleton = 1 AND ready = 0",
        [],
    )?;
    Ok(())
}

fn folder_selection_revision_ready(conn: &Connection) -> rusqlite::Result<bool> {
    match conn.query_row(
        "SELECT ready FROM folder_selection_revision WHERE singleton = 1",
        [],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(ready) => Ok(ready == 1),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(error)
            if error.to_string().contains("no such table")
                || error.to_string().contains("no such column") =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

/// Released catalogs store PDF thumbnail raster pixels in `source_*`, but do not
/// contain the page box needed for exact layout. The short-lived development
/// schema version 1 instead wrote page-box units into `source_*`. Neither row can
/// be upgraded from the cached WebP alone, so invalidate only PDF-derived rows
/// once and regenerate both independent dimension pairs.
///
/// A catalog whose owner is a PDF contains its virtual `page_NNNN` rows only;
/// ordinary folder catalogs may contain `pdfthumb:` representative rows beside
/// unrelated image/ZIP entries, which must remain intact.
fn migrate_pdf_layout_dims(conn: &mut Connection, folder_path: &Path) -> rusqlite::Result<()> {
    let current: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            [PDF_LAYOUT_DIMS_META_KEY],
            |row| row.get(0),
        )
        .optional()?;
    if current.as_deref() == Some(PDF_LAYOUT_DIMS_VERSION) {
        return Ok(());
    }

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Another opener may have completed the migration while this connection
    // waited for the write lock. Recheck under the transaction before deleting.
    let version: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            [PDF_LAYOUT_DIMS_META_KEY],
            |row| row.get(0),
        )
        .optional()?;
    if version.as_deref() == Some(PDF_LAYOUT_DIMS_VERSION) {
        return tx.commit();
    }
    let is_pdf_catalog = folder_path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
    if is_pdf_catalog {
        tx.execute("DELETE FROM thumbnails", [])?;
    } else {
        tx.execute(
            "DELETE FROM thumbnails WHERE filename LIKE 'pdfthumb:%'",
            [],
        )?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params![PDF_LAYOUT_DIMS_META_KEY, PDF_LAYOUT_DIMS_VERSION],
    )?;
    tx.commit()
}

fn add_thumbnail_column_if_missing(
    conn: &Connection,
    column: &str,
    alter_sql: &str,
) -> rusqlite::Result<()> {
    if thumbnail_column_exists(conn, column)? {
        return Ok(());
    }
    match conn.execute(alter_sql, []) {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(_, Some(message)))
            if message.contains("duplicate column name") =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn thumbnail_column_exists(conn: &Connection, column: &str) -> rusqlite::Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('thumbnails') WHERE name = ?1 LIMIT 1",
            [column],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

// -----------------------------------------------------------------------
// WebP エンコード・デコードヘルパー
// -----------------------------------------------------------------------

/// 画像を `long_side` px にリサイズし、ロッシー WebP でエンコードする。
/// `quality` は 0.0–100.0 (JPEG の quality と同等の意味)。
/// 戻り値: (webp_bytes, width, height)
///
/// リサイズは SIMD 実装の `fast_image_resize` を Lanczos3 で使用する
/// (image crate のスカラー Lanczos3 より 3-5 倍速い)。
pub fn encode_thumb_webp(
    img: &image::DynamicImage,
    long_side: u32,
    quality: f32,
) -> Option<(Vec<u8>, u32, u32)> {
    encode_thumb_webp_with_source_dims(img, long_side, quality, (img.width(), img.height()))
}

/// `encode_thumb_webp` variant that uses canonical source dimensions for aspect.
///
/// PDF page boxes and DCT-scaled JPEG buffers can differ slightly from the decoded
/// raster's already-rounded aspect. The output still never exceeds `long_side` or
/// upscales the supplied raster.
pub fn encode_thumb_webp_with_source_dims(
    img: &image::DynamicImage,
    long_side: u32,
    quality: f32,
    source_dims: (u32, u32),
) -> Option<(Vec<u8>, u32, u32)> {
    encode_thumb_webp_with_aspect_dims(img, long_side, quality, source_dims)
}

/// `encode_thumb_webp` variant that uses dimensions supplied only for aspect.
/// The values may be pixel source dimensions or PDF page-layout dimensions.
pub fn encode_thumb_webp_with_aspect_dims(
    img: &image::DynamicImage,
    long_side: u32,
    quality: f32,
    aspect_dims: (u32, u32),
) -> Option<(Vec<u8>, u32, u32)> {
    let thumb = crate::fast_resize::resize_dynamic_fit_with_source_aspect(
        img,
        long_side,
        long_side,
        aspect_dims,
        crate::fast_resize::Quality::Lanczos3,
    );
    let rgb = thumb.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    let encoder = webp::Encoder::from_rgb(rgb.as_raw(), w, h);
    let webp_data = encoder.encode(quality.clamp(1.0, 100.0));
    Some((webp_data.to_vec(), w, h))
}

/// キャッシュされたサムネイル (WebP あるいは旧 JPEG) を egui::ColorImage にデコードする。
/// `image::load_from_memory` が自動でフォーマット判定するため両対応。
pub fn decode_thumb_to_color_image(data: &[u8]) -> Option<egui::ColorImage> {
    let (w, h, rgba) = decode_thumb_to_rgba(data)?;
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        &rgba,
    ))
}

/// `image::load_from_memory` でデコードして RGBA8 + (w, h) を返す。
/// `decode_thumb_to_color_image` と動画タイル サムネ cache の WebP 復元で共用。
pub fn decode_thumb_to_rgba(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(data).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    Some((w, h, rgba.into_raw()))
}

/// キャッシュディレクトリのデフォルト位置（DATA_DIR\cache）
pub fn default_cache_dir() -> PathBuf {
    crate::data_dir::get().join("cache")
}

// -----------------------------------------------------------------------
// キャッシュ管理ユーティリティ
// -----------------------------------------------------------------------

/// cache_dir 配下の .db ファイル数と合計バイト数を返す。
pub fn cache_stats(cache_dir: &Path) -> (usize, u64) {
    let mut count = 0usize;
    let mut total_bytes = 0u64;
    collect_db_files(cache_dir, &mut |meta| {
        count += 1;
        total_bytes += meta.len();
    });
    (count, total_bytes)
}

/// cache_dir 配下で最終更新時刻が `days` 日以上前の .db ファイルを削除する。
/// Actual outcomes, including files retained by external locks or I/O errors.
pub(crate) struct CatalogDeleteReport {
    pub deleted: usize,
    pub failures: Vec<(PathBuf, String)>,
}
impl CatalogDeleteReport {
    pub(crate) fn error_message(&self) -> Option<String> {
        (!self.failures.is_empty()).then(|| {
            format!(
                "{} 件を削除しましたが、{} 件を削除できませんでした: {}",
                self.deleted,
                self.failures.len(),
                self.failures[0].1
            )
        })
    }
}

/// Existing public helpers also cross the same boundary (worker callers only).
pub fn delete_old_cache(cache_dir: &Path, days: u64) -> usize {
    let access = CatalogAccess::for_cache_dir(cache_dir);
    let Ok(deletion) = access.begin_delete(CatalogDeleteOperation::OlderThan(days)) else {
        return 0;
    };
    deletion.retire_connections();
    delete_old_cache_under_delete(cache_dir, days).deleted
}

pub fn delete_all_cache(cache_dir: &Path) -> usize {
    let access = CatalogAccess::for_cache_dir(cache_dir);
    let Ok(deletion) = access.begin_delete(CatalogDeleteOperation::All) else {
        return 0;
    };
    deletion.retire_connections();
    delete_all_cache_under_delete(cache_dir).deleted
}

pub(crate) fn delete_old_cache_under_delete(cache_dir: &Path, days: u64) -> CatalogDeleteReport {
    let now = std::time::SystemTime::now();
    let threshold = std::time::Duration::from_secs(days.saturating_mul(24 * 3600));
    delete_catalog_files(cache_dir, |meta| {
        let age = meta
            .modified()
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .unwrap_or(std::time::Duration::ZERO);
        age >= threshold
    })
}

pub(crate) fn delete_all_cache_under_delete(cache_dir: &Path) -> CatalogDeleteReport {
    delete_catalog_files(cache_dir, |_| true)
}

fn delete_catalog_files(
    cache_dir: &Path,
    select: impl Fn(&std::fs::Metadata) -> bool,
) -> CatalogDeleteReport {
    let mut report = CatalogDeleteReport {
        deleted: 0,
        failures: Vec::new(),
    };
    let top = match std::fs::read_dir(cache_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return report,
        Err(error) => {
            report
                .failures
                .push((cache_dir.to_owned(), error.to_string()));
            return report;
        }
    };
    for entry in top {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                report
                    .failures
                    .push((cache_dir.to_owned(), error.to_string()));
                continue;
            }
        };
        let sub = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {}
            Ok(_) => continue,
            Err(error) => {
                report.failures.push((sub, error.to_string()));
                continue;
            }
        }
        let entries = match std::fs::read_dir(&sub) {
            Ok(entries) => entries,
            Err(error) => {
                report.failures.push((sub, error.to_string()));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    report.failures.push((sub.clone(), error.to_string()));
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("db") {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    report.failures.push((path, error.to_string()));
                    continue;
                }
            };
            if !select(&metadata) {
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => report.deleted += 1,
                Err(error) => {
                    report.failures.push((path, error.to_string()));
                }
            }
        }
    }
    for (path, error) in &report.failures {
        crate::logger::log(format!(
            "catalog delete failed: {}: {error}",
            path.display()
        ));
    }
    report
}

/// Delete general rows and only this parent's audio scope in a drive-shared DB.
/// This exclusive maintenance connection is closed before the deletion token drops.
pub(crate) fn delete_folder_cache_under_delete(
    cache_dir: &Path,
    parent: &Path,
) -> rusqlite::Result<bool> {
    let path = db_path_for(cache_dir, parent);
    if !catalog_file_exists(&path)? {
        return Ok(false);
    }
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let scope = AudioArtCatalogScope::new(parent);
    let tx = connection.unchecked_transaction()?;
    tx.execute("DELETE FROM thumbnails WHERE substr(filename,1,9) <> 'audioart:' OR substr(filename,1,?1)=?2", params![scope.prefix.len(),scope.prefix])?;
    let has_absence: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='audio_art_absence')", [], |row| row.get(0))?;
    if has_absence {
        tx.execute(
            "DELETE FROM audio_art_absence WHERE substr(filename,1,?1)=?2",
            params![scope.prefix.len(), scope.prefix],
        )?;
    }
    for table in ["pdf_meta", "container_page_meta", "video_meta"] {
        let present: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |row| row.get(0),
        )?;
        if present {
            tx.execute(&format!("DELETE FROM {table}"), [])?;
        }
    }
    tx.commit()?;
    let remaining: i64 =
        connection.query_row("SELECT count(*) FROM thumbnails", [], |row| row.get(0))?;
    let remaining_absence: i64 = if has_absence {
        connection.query_row("SELECT count(*) FROM audio_art_absence", [], |row| {
            row.get(0)
        })?
    } else {
        0
    };
    drop(connection);
    if remaining == 0 && remaining_absence == 0 {
        std::fs::remove_file(path)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    }
    Ok(true)
}

/// cache_dir 配下の .db ファイルのパスとメタデータを列挙してコールバックを呼ぶ。
fn collect_db_paths(cache_dir: &Path, cb: &mut impl FnMut(&Path, std::fs::Metadata)) {
    let Ok(top) = std::fs::read_dir(cache_dir) else {
        return;
    };
    for entry in top.flatten() {
        // per-entry GetFileAttributes syscall を避けるため file_type を 1 回取る
        // (docs/ui-responsiveness.md §4)。キャッシュ全走査は数千フォルダ規模になるので効く。
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_dir() {
            continue;
        }
        let sub = entry.path();
        let Ok(sub_entries) = std::fs::read_dir(&sub) else {
            continue;
        };
        for file in sub_entries.flatten() {
            let p = file.path();
            if p.extension().and_then(|e| e.to_str()) == Some("db") {
                if let Ok(meta) = file.metadata() {
                    cb(&p, meta);
                }
            }
        }
    }
}

/// collect_db_paths の統計専用バリアント（パス不要）。
fn collect_db_files(cache_dir: &Path, cb: &mut impl FnMut(std::fs::Metadata)) {
    collect_db_paths(cache_dir, &mut |_, meta| cb(meta));
}

// -----------------------------------------------------------------------
// テスト
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::Mutex;

    /// テスト用: in-memory SQLite で CatalogDb を作成する。
    fn open_in_memory() -> CatalogDb {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .unwrap();
        init_schema(&conn).unwrap();
        CatalogDb::from_connection(
            conn,
            std::sync::Arc::new(CatalogAccess::new()),
            CatalogAdmission::Admitted(0),
            true,
            true,
        )
    }

    #[test]
    fn pin_materialization_batch_rolls_back_delete_when_seed_fails() {
        let db = open_in_memory();
        db.save("old", 1, 1, 1, 1, None, b"old").unwrap();
        db.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_seed BEFORE INSERT ON thumbnails WHEN NEW.filename = 'seed' BEGIN SELECT RAISE(ABORT, 'rejected seed'); END;").unwrap();
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([0, 0, 0, 255]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::WebP,
        )
        .unwrap();
        let seed = CacheEntry {
            mtime: 2,
            file_size: 2,
            jpeg_data: bytes,
            source_dims: None,
            layout_dims: None,
            folder_provenance: Some(FolderThumbProvenance::Seeded),
            selection_proof: None,
        };
        assert!(
            db.commit_pin_materializations(
                &["old".into()],
                &[("seed".into(), seed)],
                &std::sync::atomic::AtomicBool::new(false)
            )
            .is_err()
        );
        assert_eq!(db.load_one("old").unwrap().unwrap().jpeg_data, b"old");
        assert!(db.load_one("seed").unwrap().is_none());
    }

    #[test]
    fn pin_materialization_cancelled_owner_cannot_delete_newer_seed() {
        let db = std::sync::Arc::new(open_in_memory());
        db.save("newer", 2, 2, 1, 1, None, b"newer").unwrap();
        let guard = db.conn.lock().unwrap();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_db = db.clone();
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || {
            worker_db
                .commit_pin_materializations(&["newer".into()], &[], &worker_cancel)
                .unwrap()
        });
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        drop(guard);
        assert!(!worker.join().unwrap());
        assert_eq!(db.load_one("newer").unwrap().unwrap().jpeg_data, b"newer");
    }

    fn revision(db: &CatalogDb) -> CatalogRevision {
        match db.revision_state() {
            CatalogProofState::Present(value) => value,
            state => panic!("expected catalog revision, got {state:?}"),
        }
    }

    #[test]
    fn folder_selection_revision_triggers_cover_archive_and_child_pin_rows() {
        let db = open_in_memory();
        let initial = revision(&db);
        let mut expected = initial.revision;
        for key in [
            "zipthumb:book.zip",
            "zipthumb:C:\\books\\book.cbz#pin:page",
            "pdfthumb:book.pdf",
            "pdfthumb:C:\\books\\book.pdf#pin:page",
            "folderthumb:auto-v2:numeric:d3:child#pin:video",
            "folderthumb:auto-v3:numeric:d3:C:\\child#pin:folder",
        ] {
            db.save(key, 1, 2, 1, 1, None, b"bytes").unwrap();
            expected += 1;
            assert_eq!(revision(&db).revision, expected, "insert {key}");
            db.save(key, 2, 3, 1, 1, None, b"bytes2").unwrap();
            expected += 1;
            assert_eq!(revision(&db).revision, expected, "replace {key}");
            db.delete_one(key).unwrap();
            expected += 1;
            assert_eq!(revision(&db).revision, expected, "delete {key}");
        }
        db.save(
            "folderthumb:auto-v3:numeric:d3:parent",
            1,
            2,
            1,
            1,
            None,
            b"bytes",
        )
        .unwrap();
        db.save("image.jpg", 1, 2, 1, 1, None, b"bytes").unwrap();
        assert_eq!(
            revision(&db).revision,
            expected,
            "automatic rows must not churn revision"
        );
    }

    #[test]
    fn folder_proof_without_pin_provenance_cannot_deserialize_as_no_dependency() {
        let proof = FolderSelectionProof {
            directories: Vec::new(),
            pin_store: PinStoreProof::NotConsulted,
            winner: FolderSelectionWinner {
                path: PathBuf::from("cover.jpg"),
                mtime: 1,
                file_size: 2,
                archive_row_key: None,
            },
        };
        let mut value = serde_json::to_value(&proof).unwrap();
        value.as_object_mut().unwrap().remove("pin_store");
        assert!(serde_json::from_value::<FolderSelectionProof>(value).is_err());
        assert_eq!(
            serde_json::from_str::<FolderSelectionProof>(&serde_json::to_string(&proof).unwrap())
                .unwrap(),
            proof
        );
    }

    #[test]
    fn folder_selection_revision_covers_delete_missing_and_recreated_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let folder = temp.path().join("folder");
        let db = CatalogDb::open(&cache, &folder).unwrap();
        let first_instance = revision(&db).instance_id;
        db.save("zipthumb:book.zip", 1, 2, 1, 1, None, b"bytes")
            .unwrap();
        let before = revision(&db).revision;
        db.delete_missing(&HashSet::new()).unwrap();
        assert_eq!(revision(&db).revision, before + 1);
        drop(db);
        assert_eq!(delete_all_cache(&cache), 1);
        let reopened = CatalogDb::open(&cache, &folder).unwrap();
        assert_ne!(revision(&reopened).instance_id, first_instance);
    }

    #[test]
    fn folder_selection_identity_detects_age_and_current_folder_cache_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let folder = temp.path().join("folder");
        let db = CatalogDb::open(&cache, &folder).unwrap();
        let first = revision(&db);
        drop(db);
        assert_eq!(delete_old_cache(&cache, 0), 1);
        assert!(
            CatalogDb::open_for_folder_selection(&cache, &folder)
                .unwrap()
                .is_none()
        );
        let recreated = CatalogDb::open(&cache, &folder).unwrap();
        assert_ne!(revision(&recreated).instance_id, first.instance_id);
        drop(recreated);
        // Cache manager's current-folder mode removes this exact hashed file.
        std::fs::remove_file(db_path_for(&cache, &folder)).unwrap();
        assert!(
            CatalogDb::open_for_folder_selection(&cache, &folder)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn folder_selection_non_not_found_open_failure_is_not_absent() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let folder = temp.path().join("folder");
        let path = db_path_for(&cache, &folder);
        std::fs::create_dir_all(&path).unwrap();
        assert!(CatalogDb::open_for_folder_selection(&cache, &folder).is_err());
    }

    #[test]
    fn folder_selection_initializes_released_catalog_without_rewriting_rows() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let folder = temp.path().join("folder");
        let path = db_path_for(&cache, &folder);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE thumbnails (
            filename TEXT NOT NULL PRIMARY KEY, mtime INTEGER NOT NULL,
            file_size INTEGER NOT NULL, width INTEGER NOT NULL, height INTEGER NOT NULL,
            thumb_data BLOB NOT NULL, source_width INTEGER, source_height INTEGER
        );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO thumbnails VALUES ('zipthumb:book.zip', 1, 2, 1, 1, X'01', NULL, NULL)",
            [],
        )
        .unwrap();
        drop(conn);

        let db = CatalogDb::open_for_folder_selection(&cache, &folder)
            .unwrap()
            .unwrap();
        assert!(matches!(db.revision_state(), CatalogProofState::Present(_)));
        assert_eq!(
            db.load_stamp_snapshot("zipthumb:book.zip").unwrap().1,
            Some((1, 2))
        );
        assert_eq!(
            db.load_one("zipthumb:book.zip").unwrap().unwrap().jpeg_data,
            vec![1]
        );
        drop(db);
        let writable = CatalogDb::open(&cache, &folder).unwrap();
        // Opening an old schema may prune released rows; triggers were installed
        // before that migration. Exercise a fresh row through the public writer.
        writable
            .save("zipthumb:book.zip", 1, 2, 1, 1, None, b"bytes")
            .unwrap();
        let before = revision(&writable).revision;
        writable.delete_one("zipthumb:book.zip").unwrap();
        assert_eq!(revision(&writable).revision, before + 1);
    }

    #[test]
    fn incomplete_revision_setup_is_repaired_before_selection() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let folder = temp.path().join("folder");
        let path = db_path_for(&cache, &folder);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE thumbnails (
            filename TEXT NOT NULL PRIMARY KEY, mtime INTEGER NOT NULL,
            file_size INTEGER NOT NULL, width INTEGER NOT NULL, height INTEGER NOT NULL,
            thumb_data BLOB NOT NULL, source_width INTEGER, source_height INTEGER
        );
        CREATE TABLE folder_selection_revision (
            singleton INTEGER PRIMARY KEY, instance_id TEXT NOT NULL, revision INTEGER NOT NULL
        );
        INSERT INTO folder_selection_revision VALUES (1, 'old-instance', 12);
        INSERT INTO thumbnails VALUES ('zipthumb:book.zip', 1, 2, 1, 1, X'01', NULL, NULL);",
        )
        .unwrap();
        drop(conn);
        let db = CatalogDb::open_for_folder_selection(&cache, &folder)
            .unwrap()
            .unwrap();
        assert_eq!(
            revision(&db),
            CatalogRevision {
                instance_id: "old-instance".to_owned(),
                revision: 12,
            }
        );
        assert!(db.load_one("zipthumb:book.zip").unwrap().is_some());
        drop(db);
        let writer = Connection::open(&path).unwrap();
        writer
            .execute(
                "DELETE FROM thumbnails WHERE filename = 'zipthumb:book.zip'",
                [],
            )
            .unwrap();
        drop(writer);
        let db = CatalogDb::open_for_folder_selection(&cache, &folder)
            .unwrap()
            .unwrap();
        assert_eq!(revision(&db).revision, 13);
    }

    #[test]
    fn failed_legacy_initialization_is_memoized_until_catalog_file_changes() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let folder = temp.path().join("folder");
        let path = db_path_for(&cache, &folder);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE thumbnails (
                filename TEXT PRIMARY KEY, mtime INTEGER, file_size INTEGER,
                thumb_data BLOB
            );
            CREATE TABLE folder_selection_revision (wrong INTEGER);",
        )
        .unwrap();
        drop(conn);
        for _ in 0..3 {
            let db = CatalogDb::open_for_folder_selection(&cache, &folder)
                .unwrap()
                .unwrap();
            assert_eq!(db.revision_state(), CatalogProofState::Unverifiable);
        }
        assert_eq!(legacy_init_attempts().lock().unwrap().get(&path), Some(&1));

        // Replace the failed legacy file. Its different identity and size must
        // permit exactly one new initialization attempt.
        std::fs::remove_file(&path).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE thumbnails (
                filename TEXT PRIMARY KEY, mtime INTEGER, file_size INTEGER,
                thumb_data BLOB
            );
            INSERT INTO thumbnails VALUES ('zipthumb:book.zip', 1, 2, zeroblob(20000));",
        )
        .unwrap();
        drop(conn);
        let db = CatalogDb::open_for_folder_selection(&cache, &folder)
            .unwrap()
            .unwrap();
        assert!(matches!(db.revision_state(), CatalogProofState::Present(_)));
        assert_eq!(legacy_init_attempts().lock().unwrap().get(&path), Some(&2));
    }

    #[test]
    fn failed_legacy_initialization_retries_after_wal_only_change() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let folder = temp.path().join("folder");
        let path = db_path_for(&cache, &folder);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
             CREATE TABLE thumbnails (
                 filename TEXT PRIMARY KEY, mtime INTEGER, file_size INTEGER,
                 thumb_data BLOB
             );
             CREATE TABLE folder_selection_revision (wrong INTEGER);",
            )
            .unwrap();
        for _ in 0..2 {
            let db = CatalogDb::open_for_folder_selection(&cache, &folder)
                .unwrap()
                .unwrap();
            assert_eq!(db.revision_state(), CatalogProofState::Unverifiable);
        }
        assert_eq!(legacy_init_attempts().lock().unwrap().get(&path), Some(&1));
        let before = CatalogFileStamp::from_path(&path, &std::fs::metadata(&path).unwrap());
        writer
            .execute(
                "INSERT INTO thumbnails VALUES ('zipthumb:book.zip', 1, 2, zeroblob(10000))",
                [],
            )
            .unwrap();
        let after = CatalogFileStamp::from_path(&path, &std::fs::metadata(&path).unwrap());
        assert_eq!(
            before.db, after.db,
            "the main db file should not have changed"
        );
        assert_ne!(
            before.wal, after.wal,
            "the WAL must identify the changed catalog"
        );
        let db = CatalogDb::open_for_folder_selection(&cache, &folder)
            .unwrap()
            .unwrap();
        assert_eq!(db.revision_state(), CatalogProofState::Unverifiable);
        assert_eq!(legacy_init_attempts().lock().unwrap().get(&path), Some(&2));
    }

    #[test]
    fn malformed_legacy_revision_is_unverifiable_but_rows_remain_readable() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let folder = temp.path().join("folder");
        let path = db_path_for(&cache, &folder);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE thumbnails (
            filename TEXT NOT NULL PRIMARY KEY, mtime INTEGER NOT NULL,
            file_size INTEGER NOT NULL, width INTEGER NOT NULL, height INTEGER NOT NULL,
            thumb_data BLOB NOT NULL, source_width INTEGER, source_height INTEGER
        );
        CREATE TABLE folder_selection_revision (wrong INTEGER);
        INSERT INTO thumbnails VALUES ('pdfthumb:book.pdf', 1, 2, 1, 1, X'01', NULL, NULL);",
        )
        .unwrap();
        drop(conn);
        let db = CatalogDb::open_for_folder_selection(&cache, &folder)
            .unwrap()
            .unwrap();
        let (state, stamps) = db.load_stamp_snapshot("pdfthumb:book.pdf").unwrap();
        assert_eq!(state, CatalogProofState::Unverifiable);
        assert_eq!(stamps, Some((1, 2)));
        assert!(db.load_one("pdfthumb:book.pdf").unwrap().is_some());
    }

    // -- db_path_for --

    #[test]
    fn db_path_for_deterministic() {
        let cache = Path::new(r"C:\cache");
        let folder = Path::new(r"D:\photos\2024");
        let a = db_path_for(cache, folder);
        let b = db_path_for(cache, folder);
        assert_eq!(a, b);
    }

    #[test]
    fn db_path_for_different_paths() {
        let cache = Path::new(r"C:\cache");
        let a = db_path_for(cache, Path::new(r"D:\photos\2024"));
        let b = db_path_for(cache, Path::new(r"D:\photos\2025"));
        assert_ne!(a, b);
    }

    #[test]
    fn db_path_for_case_insensitive() {
        let cache = Path::new(r"C:\cache");
        let a = db_path_for(cache, Path::new(r"C:\Photos\Vacation"));
        let b = db_path_for(cache, Path::new(r"D:\photos\vacation"));
        // ドライブ文字は除去され、小文字化されるので同じパスになるはず
        assert_eq!(a, b);
    }

    #[test]
    fn db_path_for_drive_roots_keeps_drive_letter() {
        let cache = Path::new(r"C:\cache");
        let c = db_path_for(cache, Path::new(r"C:\"));
        let d = db_path_for(cache, Path::new(r"D:\"));
        assert_ne!(c, d, "ドライブルート catalog は直下同名項目の衝突を避ける");
    }

    #[test]
    fn db_path_for_non_root_still_ignores_drive_letter() {
        let cache = Path::new(r"C:\cache");
        let c = db_path_for(cache, Path::new(r"C:\Photos"));
        let d = db_path_for(cache, Path::new(r"D:\photos"));
        assert_eq!(c, d, "非 root は従来どおりドライブレター変更に追従する");
    }

    #[test]
    fn db_path_for_structure() {
        let cache = Path::new(r"C:\cache");
        let result = db_path_for(cache, Path::new(r"D:\test"));
        let result_str = result.to_string_lossy();
        // {cache_dir}/{xx}/{hash}.db の形式
        assert!(result_str.starts_with(r"C:\cache\"));
        assert!(result_str.ends_with(".db"));
        // xx サブディレクトリが2文字の hex
        let relative = result.strip_prefix(cache).unwrap();
        let components: Vec<_> = relative.components().collect();
        assert_eq!(components.len(), 2); // xx/ と hash.db
    }

    // -- CatalogDb schema --

    #[test]
    fn catalog_delete_all_closes_live_handles() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("music");
        let db = std::sync::Arc::new(CatalogDb::open(temp.path(), &folder).unwrap());
        db.save("photo.jpg", 1, 2, 1, 1, None, b"pixels").unwrap();
        assert_eq!(delete_all_cache(temp.path()), 1);
        assert!(!db_path_for(temp.path(), &folder).exists());
        assert!(db.load_one("photo.jpg").is_err());
    }

    #[test]
    fn audio_art_scope_keeps_drive_rows_and_negative_results_through_general_prune() {
        let temp = tempfile::tempdir().unwrap();
        let c = AudioArtCatalogScope::new(Path::new(r"C:\Music"));
        let d = AudioArtCatalogScope::new(Path::new(r"D:\Music"));
        assert_eq!(
            db_path_for(temp.path(), &c.parent),
            db_path_for(temp.path(), &d.parent)
        );
        let admission = CatalogAccess::for_cache_dir(temp.path()).admit();
        let cancel = AtomicBool::new(false);
        let stamp = AudioArtSourceStamp {
            mtime_secs: 7,
            file_size: 13,
        };
        let ck = c.key_for(&c.parent.join("song.mp3"));
        let dk = d.key_for(&d.parent.join("song.mp3"));
        assert_ne!(ck, dk);
        let image = image::DynamicImage::new_rgb8(2, 3);
        let entry = CacheEntry {
            mtime: 7,
            file_size: 13,
            jpeg_data: encode_thumb_webp(&image, 2, 80.0).unwrap().0,
            source_dims: Some((2, 3)),
            layout_dims: None,
            folder_provenance: None,
            selection_proof: None,
        };
        save_audio_art_pixels(temp.path(), &c, &ck, &entry, admission, &cancel).unwrap();
        save_audio_art_pixels(temp.path(), &d, &dk, &entry, admission, &cancel).unwrap();
        let cn = c.key_for(&c.parent.join("empty.mp3"));
        let dn = d.key_for(&d.parent.join("empty.mp3"));
        save_audio_art_absence(temp.path(), &c, &cn, stamp, admission, &cancel).unwrap();
        save_audio_art_absence(temp.path(), &d, &dn, stamp, admission, &cancel).unwrap();
        let db = CatalogDb::open(temp.path(), &c.parent).unwrap();
        db.save("photo.jpg", 1, 1, 1, 1, None, b"photo").unwrap();
        assert_eq!(db.load_all().unwrap().len(), 1);
        assert_eq!(db.load_source_dims().unwrap().len(), 1);
        assert!(db.load_source_dims_matching(7, 13).unwrap().is_empty());
        db.delete_missing(&HashSet::new()).unwrap();
        assert!(matches!(
            lookup_audio_art(temp.path(), &c, &ck, stamp, admission).unwrap(),
            AudioArtCached::Pixels(_)
        ));
        assert!(matches!(
            lookup_audio_art(temp.path(), &d, &dk, stamp, admission).unwrap(),
            AudioArtCached::Pixels(_)
        ));
        assert!(matches!(
            lookup_audio_art(temp.path(), &c, &cn, stamp, admission).unwrap(),
            AudioArtCached::NoArt
        ));
        assert!(matches!(
            lookup_audio_art(temp.path(), &d, &dn, stamp, admission).unwrap(),
            AudioArtCached::NoArt
        ));
        let complete = HashSet::from(["empty.mp3".to_owned()]);
        assert_eq!(
            prune_audio_art_scope(temp.path(), &c, &complete, admission, &cancel).unwrap(),
            1
        );
        assert!(matches!(
            lookup_audio_art(temp.path(), &c, &ck, stamp, admission).unwrap(),
            AudioArtCached::Miss
        ));
        assert!(matches!(
            lookup_audio_art(temp.path(), &c, &cn, stamp, admission).unwrap(),
            AudioArtCached::NoArt
        ));
        assert!(matches!(
            lookup_audio_art(temp.path(), &d, &dk, stamp, admission).unwrap(),
            AudioArtCached::Pixels(_)
        ));
        assert!(matches!(
            lookup_audio_art(temp.path(), &d, &dn, stamp, admission).unwrap(),
            AudioArtCached::NoArt
        ));
    }

    #[test]
    fn audio_art_positive_and_absence_are_atomic_exclusive_and_stamp_exact() {
        let temp = tempfile::tempdir().unwrap();
        let scope = AudioArtCatalogScope::new(&temp.path().join("music"));
        let key = scope.key_for(&scope.parent.join("song.mp3"));
        let admission = CatalogAccess::for_cache_dir(temp.path()).admit();
        let cancel = AtomicBool::new(false);
        let stamp = AudioArtSourceStamp {
            mtime_secs: 0,
            file_size: 0,
        };
        assert!(
            save_audio_art_absence(temp.path(), &scope, &key, stamp, admission, &cancel).unwrap()
        );
        assert!(matches!(
            lookup_audio_art(temp.path(), &scope, &key, stamp, admission).unwrap(),
            AudioArtCached::NoArt
        ));
        assert!(matches!(
            lookup_audio_art(
                temp.path(),
                &scope,
                &key,
                AudioArtSourceStamp {
                    mtime_secs: 1,
                    ..stamp
                },
                admission
            )
            .unwrap(),
            AudioArtCached::Miss
        ));
        assert!(matches!(
            lookup_audio_art(
                temp.path(),
                &scope,
                &key,
                AudioArtSourceStamp {
                    file_size: 1,
                    ..stamp
                },
                admission
            )
            .unwrap(),
            AudioArtCached::Miss
        ));
        let entry = CacheEntry {
            mtime: 0,
            file_size: 0,
            jpeg_data: encode_thumb_webp(&image::DynamicImage::new_rgb8(2, 2), 2, 80.0)
                .unwrap()
                .0,
            source_dims: Some((2, 2)),
            layout_dims: None,
            folder_provenance: None,
            selection_proof: None,
        };
        save_audio_art_pixels(temp.path(), &scope, &key, &entry, admission, &cancel).unwrap();
        assert!(matches!(
            lookup_audio_art(temp.path(), &scope, &key, stamp, admission).unwrap(),
            AudioArtCached::Pixels(_)
        ));
        let db = CatalogDb::open(temp.path(), &scope.parent).unwrap();
        assert_eq!(
            db.phase()
                .unwrap()
                .query_row("SELECT count(*) FROM audio_art_absence", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        save_audio_art_absence(temp.path(), &scope, &key, stamp, admission, &cancel).unwrap();
        assert!(db.load_one(&key).unwrap().is_none());
        cancel.store(true, Ordering::Relaxed);
        assert!(
            !save_audio_art_pixels(temp.path(), &scope, &key, &entry, admission, &cancel).unwrap()
        );
        assert!(matches!(
            lookup_audio_art(temp.path(), &scope, &key, stamp, admission).unwrap(),
            AudioArtCached::NoArt
        ));
    }

    #[test]
    fn audio_art_lookup_old_read_only_schema_is_miss_without_schema_or_file_creation() {
        let temp = tempfile::tempdir().unwrap();
        let scope = AudioArtCatalogScope::new(&temp.path().join("music"));
        let key = scope.key_for(&scope.parent.join("song.mp3"));
        let admission = CatalogAccess::for_cache_dir(temp.path()).admit();
        let stamp = AudioArtSourceStamp {
            mtime_secs: 1,
            file_size: 2,
        };
        assert!(matches!(
            lookup_audio_art(temp.path(), &scope, &key, stamp, admission).unwrap(),
            AudioArtCached::Miss
        ));
        assert!(!db_path_for(temp.path(), &scope.parent).exists());
        let db = CatalogDb::open(temp.path(), &scope.parent).unwrap();
        db.save("original.jpg", 1, 2, 1, 1, None, b"released")
            .unwrap();
        assert!(matches!(
            lookup_audio_art(temp.path(), &scope, &key, stamp, admission).unwrap(),
            AudioArtCached::Miss
        ));
        assert_eq!(
            db.phase()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name='audio_art_absence'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            db.load_one("original.jpg").unwrap().unwrap().jpeg_data,
            b"released"
        );
    }

    #[test]
    fn catalog_maintenance_drains_existing_phase_and_rejects_stale_and_display_only_writes() {
        let temp = tempfile::tempdir().unwrap();
        let scope = AudioArtCatalogScope::new(&temp.path().join("music"));
        let key = scope.key_for(&scope.parent.join("song.mp3"));
        let access = CatalogAccess::for_cache_dir(temp.path());
        let admitted = access.admit();
        let db = std::sync::Arc::new(CatalogDb::open(temp.path(), &scope.parent).unwrap());
        let phase = db.phase().unwrap();
        let deletion = access.begin_delete(CatalogDeleteOperation::All).unwrap();
        let display_only = access.admit();
        assert!(matches!(display_only, CatalogAdmission::DisplayOnly(_)));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            deletion.retire_connections();
            done_tx.send(deletion).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_millis(20))
                .is_err()
        );
        drop(phase);
        let deletion = done_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert!(db.is_retired());
        assert!(db.load_all().is_err());
        assert_eq!(delete_all_cache_under_delete(temp.path()).deleted, 1);
        drop(deletion);
        handle.join().unwrap();
        let stamp = AudioArtSourceStamp {
            mtime_secs: 1,
            file_size: 2,
        };
        let cancel = AtomicBool::new(false);
        assert!(
            save_audio_art_absence(temp.path(), &scope, &key, stamp, admitted, &cancel).is_err()
        );
        assert!(
            save_audio_art_absence(temp.path(), &scope, &key, stamp, display_only, &cancel)
                .is_err()
        );
        assert!(!db_path_for(temp.path(), &scope.parent).exists());
        let new = access.admit();
        assert_ne!(new, admitted);
        save_audio_art_absence(temp.path(), &scope, &key, stamp, new, &cancel).unwrap();
        assert!(matches!(
            lookup_audio_art(temp.path(), &scope, &key, stamp, new).unwrap(),
            AudioArtCached::NoArt
        ));
    }

    #[test]
    fn catalog_expiry_retires_handles_but_preserves_unselected_cache_and_read_continuation() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("music");
        let db = CatalogDb::open(temp.path(), &parent).unwrap();
        db.save("photo.jpg", 1, 2, 1, 1, None, b"released").unwrap();
        assert_eq!(delete_old_cache(temp.path(), 365), 0);
        assert!(db.is_retired());
        let entry = db
            .resume_cache_only_after_maintenance("photo.jpg", &|| false)
            .unwrap()
            .unwrap();
        assert_eq!(entry.jpeg_data, b"released");
        assert!(db.is_retired());
        assert!(db.load_one("photo.jpg").is_err());
        assert_eq!(delete_old_cache(temp.path(), 0), 1);
        assert!(
            CatalogDb::open_existing_read_only(temp.path(), &parent)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn catalog_folder_delete_preserves_other_drive_audio_scope() {
        let temp = tempfile::tempdir().unwrap();
        let c = AudioArtCatalogScope::new(Path::new(r"C:\Music"));
        let d = AudioArtCatalogScope::new(Path::new(r"D:\Music"));
        let access = CatalogAccess::for_cache_dir(temp.path());
        let admitted = access.admit();
        let cancel = AtomicBool::new(false);
        let stamp = AudioArtSourceStamp {
            mtime_secs: 1,
            file_size: 2,
        };
        let ck = c.key_for(&c.parent.join("song.mp3"));
        let dk = d.key_for(&d.parent.join("song.mp3"));
        save_audio_art_absence(temp.path(), &c, &ck, stamp, admitted, &cancel).unwrap();
        save_audio_art_absence(temp.path(), &d, &dk, stamp, admitted, &cancel).unwrap();
        let deletion = access.begin_delete(CatalogDeleteOperation::All).unwrap();
        deletion.retire_connections();
        assert!(delete_folder_cache_under_delete(temp.path(), &c.parent).unwrap());
        drop(deletion);
        let new = access.admit();
        assert!(matches!(
            lookup_audio_art(temp.path(), &c, &ck, stamp, new).unwrap(),
            AudioArtCached::Miss
        ));
        assert!(matches!(
            lookup_audio_art(temp.path(), &d, &dk, stamp, new).unwrap(),
            AudioArtCached::NoArt
        ));
    }

    #[test]
    fn catalog_maintenance_drop_reopens_admission_and_cancel_exits_read_wait() {
        let temp = tempfile::tempdir().unwrap();
        let access = CatalogAccess::for_cache_dir(temp.path());
        let deletion = access.begin_delete(CatalogDeleteOperation::All).unwrap();
        assert!(access.wait_read_admission(&AtomicBool::new(true)).is_none());
        drop(deletion);
        assert!(matches!(access.admit(), CatalogAdmission::Admitted(_)));
        assert!(
            access
                .wait_read_admission(&AtomicBool::new(false))
                .is_some()
        );
    }

    #[test]
    fn catalog_maintenance_read_wait_cancels_and_unwind_restores_admission() {
        let temp = tempfile::tempdir().unwrap();
        let access = CatalogAccess::for_cache_dir(temp.path());
        let deletion = access.begin_delete(CatalogDeleteOperation::All).unwrap();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker_access = access.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            worker_access.wait_read_admission(&worker_cancel)
        });
        started_rx.recv().unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert!(worker.join().unwrap().is_none());
        assert!(
            std::panic::catch_unwind(move || {
                let _deletion = deletion;
                panic!("synthetic worker unwind");
            })
            .is_err()
        );
        assert!(matches!(access.admit(), CatalogAdmission::Admitted(_)));
    }

    #[cfg(windows)]
    #[test]
    fn catalog_delete_reports_external_lock_and_keeps_actual_remaining_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("music");
        drop(CatalogDb::open(temp.path(), &parent).unwrap());
        let db_path = db_path_for(temp.path(), &parent);
        let external = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&db_path)
            .unwrap();
        let access = CatalogAccess::for_cache_dir(temp.path());
        let deletion = access.begin_delete(CatalogDeleteOperation::All).unwrap();
        deletion.retire_connections();
        let report = delete_all_cache_under_delete(temp.path());
        assert_eq!(report.deleted, 0);
        assert_eq!(report.failures.len(), 1);
        assert!(report.error_message().is_some());
        assert!(db_path.exists());
        drop(deletion);
        drop(external);
        assert_eq!(delete_all_cache(temp.path()), 1);
    }

    #[test]
    fn catalog_retirement_waits_for_last_handle_close_started_after_initial_drain() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("music");
        let db = CatalogDb::open(temp.path(), &parent).unwrap();
        let access = CatalogAccess::for_cache_dir(temp.path());
        let deletion = access.begin_delete(CatalogDeleteOperation::All).unwrap();
        let (drained_tx, drained_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            deletion.retire_connections_after_drain(|| {
                drained_tx.send(()).unwrap();
                go_rx.recv().unwrap();
            });
            done_tx.send(deletion).unwrap();
        });
        drained_rx.recv().unwrap();
        // Precisely reproduce the normal final-Arc close boundary, before the
        // worker sees the already-Retired connection in its weak registry.
        let close = access.close_lease();
        let connection = std::mem::replace(
            &mut *db.conn.state.lock().unwrap(),
            CatalogConnectionState::Retired,
        );
        go_tx.send(()).unwrap();
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_millis(20))
                .is_err()
        );
        drop(connection);
        drop(close);
        let deletion = done_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(delete_all_cache_under_delete(temp.path()).deleted, 1);
        drop(deletion);
        worker.join().unwrap();
    }

    #[test]
    fn audio_art_cancel_callback_rolls_back_negative_and_positive_replacement_at_commit() {
        let temp = tempfile::tempdir().unwrap();
        let scope = AudioArtCatalogScope::new(&temp.path().join("music"));
        let key = scope.key_for(&scope.parent.join("song.mp3"));
        let admission = CatalogAccess::for_cache_dir(temp.path()).admit();
        let stamp = AudioArtSourceStamp {
            mtime_secs: 1,
            file_size: 2,
        };
        let cancel = AtomicBool::new(false);
        let entry = CacheEntry {
            mtime: 1,
            file_size: 2,
            jpeg_data: encode_thumb_webp(&image::DynamicImage::new_rgb8(2, 2), 2, 80.0)
                .unwrap()
                .0,
            source_dims: Some((2, 2)),
            layout_dims: None,
            folder_provenance: None,
            selection_proof: None,
        };
        save_audio_art_pixels(temp.path(), &scope, &key, &entry, admission, &cancel).unwrap();
        let calls = std::cell::Cell::new(0);
        let check = || {
            let count = calls.get() + 1;
            calls.set(count);
            count > 1
        };
        assert!(
            !save_audio_art_absence_with_cancel_check(
                temp.path(),
                &scope,
                &key,
                stamp,
                admission,
                &check
            )
            .unwrap()
        );
        assert_eq!(calls.get(), 2);
        assert!(matches!(
            lookup_audio_art(temp.path(), &scope, &key, stamp, admission).unwrap(),
            AudioArtCached::Pixels(_)
        ));
        save_audio_art_absence(temp.path(), &scope, &key, stamp, admission, &cancel).unwrap();
        calls.set(0);
        assert!(
            !save_audio_art_pixels_with_cancel_check(
                temp.path(),
                &scope,
                &key,
                &entry,
                admission,
                &check
            )
            .unwrap()
        );
        assert_eq!(calls.get(), 2);
        assert!(matches!(
            lookup_audio_art(temp.path(), &scope, &key, stamp, admission).unwrap(),
            AudioArtCached::NoArt
        ));
    }

    #[test]
    fn audio_art_cache_notice_has_one_shared_owner_and_does_not_reset_on_delete() {
        let temp = tempfile::tempdir().unwrap();
        let local = CatalogAccess::for_cache_dir(temp.path());
        let remote = CatalogAccess::for_cache_dir(temp.path());
        let old = local.admit();
        assert!(std::sync::Arc::ptr_eq(&local, &remote));
        assert!(local.take_audio_art_notice().is_none());
        local.record_audio_art_cache_error("local save failed");
        remote.record_audio_art_cache_error("remote lookup failed");
        assert_eq!(
            remote.take_audio_art_notice().as_deref(),
            Some("local save failed")
        );
        assert!(local.take_audio_art_notice().is_none());
        let deletion = local.begin_delete(CatalogDeleteOperation::All).unwrap();
        assert!(!local.is_admitted(old));
        deletion.retire_connections();
        drop(deletion);
        local.record_audio_art_cache_error("later failure");
        assert!(local.take_audio_art_notice().is_none());
        let other = tempfile::tempdir().unwrap();
        let separate = CatalogAccess::for_cache_dir(other.path());
        separate.record_audio_art_cache_error("different cache");
        assert_eq!(
            separate.take_audio_art_notice().as_deref(),
            Some("different cache")
        );
    }

    #[test]
    fn catalog_cache_only_continuation_cancels_during_delete_and_whole_clear_is_miss() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("music");
        let db = std::sync::Arc::new(CatalogDb::open(temp.path(), &parent).unwrap());
        db.save("photo.jpg", 1, 2, 1, 1, None, b"released").unwrap();
        let access = CatalogAccess::for_cache_dir(temp.path());
        let deletion = access.begin_delete(CatalogDeleteOperation::All).unwrap();
        deletion.retire_connections();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let worker_db = db.clone();
        let worker_cancel = cancel.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            worker_db.resume_cache_only_after_maintenance("photo.jpg", &|| {
                worker_cancel.load(Ordering::Relaxed)
            })
        });
        started_rx.recv().unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert!(worker.join().unwrap().unwrap().is_none());
        assert_eq!(delete_all_cache_under_delete(temp.path()).deleted, 1);
        drop(deletion);
        assert!(
            db.resume_cache_only_after_maintenance("photo.jpg", &|| false)
                .unwrap()
                .is_none()
        );
        assert!(!db_path_for(temp.path(), &parent).exists());
        assert!(db.is_retired());
        assert!(db.load_one("photo.jpg").is_err());
        let in_memory = open_in_memory();
        in_memory
            .save("photo.jpg", 1, 2, 1, 1, None, b"pixels")
            .unwrap();
        assert!(
            in_memory
                .resume_cache_only_after_maintenance("photo.jpg", &|| false)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn catalog_open_and_schema() {
        let db = open_in_memory();
        let conn = db.conn.lock().unwrap();
        // meta テーブルにバージョンが記録されているか
        let version: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(version, CATALOG_VERSION);
        let columns = conn
            .prepare("SELECT name FROM pragma_table_info('thumbnails')")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .flatten()
            .collect::<HashSet<_>>();
        assert!(columns.contains("source_width"));
        assert!(columns.contains("source_height"));
        assert!(columns.contains("layout_width"));
        assert!(columns.contains("layout_height"));
    }

    #[test]
    fn pdf_layout_migration_rebuilds_development_v1_page_rows_once() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache");
        let pdf_path = tmp.path().join("book.pdf");
        let db = CatalogDb::open(&cache_dir, &pdf_path).unwrap();
        db.save(
            "page_0000",
            1,
            10,
            327,
            473,
            Some((595_276, 841_890)),
            b"development-v1",
        )
        .unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, '1')",
                [PDF_LAYOUT_DIMS_META_KEY],
            )
            .unwrap();
        drop(db);

        let migrated = CatalogDb::open(&cache_dir, &pdf_path).unwrap();
        assert!(migrated.load_all().unwrap().is_empty());
        migrated
            .save_with_layout_dims(
                "page_0000",
                1,
                10,
                327,
                473,
                Some((327, 473)),
                Some((595_276, 841_890)),
                b"fixed",
            )
            .unwrap();
        drop(migrated);

        let reopened = CatalogDb::open(&cache_dir, &pdf_path).unwrap();
        assert_eq!(
            reopened.load_all().unwrap()["page_0000"].source_dims,
            Some((327, 473)),
            "the migration marker must preserve regenerated rows on later opens"
        );
        assert_eq!(
            reopened.load_all().unwrap()["page_0000"].layout_dims,
            Some((595_276, 841_890))
        );
    }

    #[test]
    fn pdf_layout_migration_keeps_non_pdf_rows_in_folder_catalogs() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache");
        let folder = tmp.path().join("photos");
        let db = CatalogDb::open(&cache_dir, &folder).unwrap();
        db.save("image.jpg", 1, 10, 8, 8, Some((4000, 3000)), b"image")
            .unwrap();
        db.save(
            "pdfthumb:book.pdf",
            1,
            10,
            8,
            8,
            Some((327, 473)),
            b"legacy-pdf",
        )
        .unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM meta WHERE key = ?1",
                [PDF_LAYOUT_DIMS_META_KEY],
            )
            .unwrap();
        drop(db);

        let migrated = CatalogDb::open(&cache_dir, &folder).unwrap();
        let rows = migrated.load_all().unwrap();
        assert!(rows.contains_key("image.jpg"));
        assert!(!rows.contains_key("pdfthumb:book.pdf"));
    }

    // -- CatalogDb CRUD --

    #[test]
    fn catalog_save_and_load_all() {
        let db = open_in_memory();
        db.save(
            "test.jpg",
            1000,
            2048,
            256,
            192,
            Some((4000, 3000)),
            b"fake_webp",
        )
        .unwrap();

        let map = db.load_all().unwrap();
        assert_eq!(map.len(), 1);
        let entry = &map["test.jpg"];
        assert_eq!(entry.mtime, 1000);
        assert_eq!(entry.file_size, 2048);
        assert_eq!(entry.jpeg_data, b"fake_webp");
        assert_eq!(entry.source_dims, Some((4000, 3000)));
        assert_eq!(entry.layout_dims, None);
    }

    #[test]
    fn catalog_keeps_pdf_raster_pixels_separate_from_page_layout() {
        let db = open_in_memory();
        db.save_with_layout_dims(
            "page_0000",
            1000,
            2048,
            273,
            416,
            Some((273, 416)),
            Some((468_600, 714_360)),
            b"fake_webp",
        )
        .unwrap();

        let entry = db.load_one("page_0000").unwrap().unwrap();
        assert_eq!(entry.source_dims, Some((273, 416)));
        assert_eq!(entry.layout_dims, Some((468_600, 714_360)));
    }

    /// 一覧を開いた瞬間、リモートはサムネイル worker 12 本から同じカタログを同時に開く。
    /// 2026-08-13 の実害はここで、2 要求が `database is locked` で 1.1ms 失敗し、再要求もされず
    /// 一覧のその 2 枚だけが記号のまま残った。`busy_timeout` は既定 5 秒入っているのに効かない。
    #[test]
    fn opening_one_catalog_from_many_threads_at_once_never_reports_it_locked() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().to_path_buf();
        let folder = tmp.path().join("folder");
        std::fs::create_dir_all(&folder).unwrap();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(12));
        let mut handles = Vec::new();
        for _ in 0..12 {
            let barrier = std::sync::Arc::clone(&barrier);
            let cache_dir = cache_dir.clone();
            let folder = folder.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                CatalogDb::open(&cache_dir, &folder)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }));
        }
        let failures: Vec<String> = handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap().err())
            .collect();
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn read_only_legacy_catalog_treats_missing_layout_columns_as_none() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache");
        let folder = tmp.path().join("photos");
        let db_path = db_path_for(&cache_dir, &folder);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE thumbnails (
                 filename TEXT NOT NULL PRIMARY KEY,
                 mtime INTEGER NOT NULL,
                 file_size INTEGER NOT NULL,
                 width INTEGER NOT NULL,
                 height INTEGER NOT NULL,
                 thumb_data BLOB NOT NULL,
                 source_width INTEGER,
                 source_height INTEGER
             );
             INSERT INTO thumbnails VALUES
                 ('image.jpg', 1, 10, 8, 8, X'0102', 4000, 3000);",
        )
        .unwrap();
        drop(conn);

        let db = CatalogDb::open_existing_read_only(&cache_dir, &folder)
            .unwrap()
            .unwrap();
        let all = db.load_all().unwrap();
        assert_eq!(all["image.jpg"].source_dims, Some((4000, 3000)));
        assert_eq!(all["image.jpg"].layout_dims, None);
        assert_eq!(db.load_one("image.jpg").unwrap().unwrap().layout_dims, None);
        assert_eq!(
            db.load_latest_with_prefix("image")
                .unwrap()
                .unwrap()
                .1
                .layout_dims,
            None
        );
    }

    #[test]
    fn catalog_save_overwrites() {
        let db = open_in_memory();
        db.save("img.jpg", 100, 500, 128, 96, None, b"data1")
            .unwrap();
        db.save("img.jpg", 200, 600, 128, 96, None, b"data2")
            .unwrap();

        let map = db.load_all().unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map["img.jpg"].mtime, 200);
        assert_eq!(map["img.jpg"].jpeg_data, b"data2");
    }

    #[test]
    fn catalog_source_dims_none() {
        let db = open_in_memory();
        db.save("no_dims.jpg", 100, 500, 128, 96, None, b"data")
            .unwrap();

        let map = db.load_all().unwrap();
        assert_eq!(map["no_dims.jpg"].source_dims, None);
    }

    #[test]
    fn source_dims_query_separates_a_missing_row_from_a_row_without_dimensions() {
        // Callers use the difference to decide whether reading a thumbnail is worth it:
        // a missing row has nothing to recover, an empty one predates the columns.
        let db = open_in_memory();
        db.save("wide.jpg", 1, 10, 128, 96, Some((4000, 3000)), b"thumb")
            .unwrap();
        db.save("legacy.jpg", 2, 20, 128, 96, None, b"thumb")
            .unwrap();

        let dims = db.load_source_dims().unwrap();
        assert_eq!(dims.get("wide.jpg"), Some(&Some((4000, 3000))));
        assert_eq!(dims.get("legacy.jpg"), Some(&None));
        assert_eq!(dims.get("absent.jpg"), None);
        assert_eq!(dims.len(), 2);
    }

    #[test]
    fn source_dims_query_agrees_with_the_full_load_it_replaces() {
        let db = open_in_memory();
        db.save("a.jpg", 1, 10, 128, 96, Some((1200, 1800)), b"a")
            .unwrap();
        db.save("b.jpg", 2, 20, 128, 96, Some((1800, 1200)), b"b")
            .unwrap();
        db.save("c.jpg", 3, 30, 128, 96, None, b"c").unwrap();

        let full = db.load_all().unwrap();
        let dims = db.load_source_dims().unwrap();
        assert_eq!(full.len(), dims.len());
        for (filename, entry) in &full {
            assert_eq!(dims.get(filename), Some(&entry.source_dims), "{filename}");
        }
    }

    #[test]
    fn catalog_delete_missing() {
        let db = open_in_memory();
        db.save("keep.jpg", 100, 500, 128, 96, None, b"a").unwrap();
        db.save("remove.jpg", 200, 600, 128, 96, None, b"b")
            .unwrap();
        db.save("also_remove.jpg", 300, 700, 128, 96, None, b"c")
            .unwrap();

        let existing: HashSet<String> = ["keep.jpg".to_string()].into_iter().collect();
        db.delete_missing(&existing).unwrap();

        let map = db.load_all().unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("keep.jpg"));
    }

    #[test]
    fn catalog_delete_one_removes_only_target() {
        let db = open_in_memory();
        db.save("a.jpg", 1, 10, 8, 8, None, b"a").unwrap();
        db.save("b.jpg", 1, 10, 8, 8, None, b"b").unwrap();
        db.delete_one("a.jpg").unwrap();
        let map = db.load_all().unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("b.jpg"));
        assert!(!map.contains_key("a.jpg"));
        // 二度目の delete (存在しないキー) もエラーにしない
        db.delete_one("a.jpg").unwrap();
        db.delete_one("never_existed.jpg").unwrap();
    }

    #[test]
    fn catalog_load_latest_with_prefix_picks_newest_matching_pin_entry() {
        let db = open_in_memory();
        db.save("folderthumb:child", 10, 1, 8, 8, None, b"base")
            .unwrap();
        db.save(
            "folderthumb:child#pin:image|cover|-|-|20|2",
            20,
            2,
            8,
            8,
            None,
            b"pin",
        )
        .unwrap();
        db.save("folderthumb:child-other", 99, 9, 8, 8, None, b"other")
            .unwrap();

        let (filename, entry) = db
            .load_latest_with_prefix("folderthumb:child#pin:")
            .unwrap()
            .expect("narrow prefix hit");
        assert_eq!(filename, "folderthumb:child#pin:image|cover|-|-|20|2");
        assert_eq!(entry.jpeg_data, b"pin");
        assert!(db.load_latest_with_prefix("missing:").unwrap().is_none());
    }

    #[test]
    fn catalog_column_migration_runs_once_and_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE thumbnails (
                filename TEXT NOT NULL PRIMARY KEY,
                mtime INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                width INTEGER NOT NULL,
                height INTEGER NOT NULL,
                thumb_data BLOB NOT NULL
            );",
        )
        .unwrap();

        init_schema(&conn).unwrap();
        init_schema(&conn).unwrap();
        let migrated: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('thumbnails')
                 WHERE name IN ('source_width', 'source_height')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(migrated, 2);
    }

    #[test]
    fn catalog_version_mismatch_clears() {
        // 1) DB を作成してデータを保存
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO thumbnails (filename, mtime, file_size, width, height, thumb_data) \
             VALUES ('old.jpg', 1, 1, 1, 1, X'00')",
            [],
        )
        .unwrap();
        // データが存在することを確認
        let count: i64 = conn
            .query_row("SELECT count(*) FROM thumbnails", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // 2) バージョンを不正な値に書き換え
        conn.execute(
            "UPDATE meta SET value = 'old_version' WHERE key = 'version'",
            [],
        )
        .unwrap();

        // 3) init_schema を再度呼ぶとバージョン不一致で全削除されるはず
        init_schema(&conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM thumbnails", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    fn write_video_meta_source(path: &Path, mtime: i64, size: i64) {
        std::fs::write(path, vec![0; size as usize]).unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new().set_modified(
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime as u64),
                ),
            )
            .unwrap();
        assert_eq!(media_source_identity(path), Some((mtime, size)));
    }

    #[test]
    fn video_meta_stalled_source_stat_does_not_block_catalog_or_ui_pin_writes() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("movie.mp4");
        let cache = temp.path().join("cache");
        let writer = CatalogDb::open(&cache, temp.path()).unwrap();
        let ui = CatalogDb::open(&cache, temp.path()).unwrap();
        let pin_key = "folderthumb:auto-v3:numeric:d3:child#pin:video";
        ui.save(pin_key, 1, 2, 1, 1, None, b"pin").unwrap();
        ui.conn
            .lock()
            .unwrap()
            .busy_timeout(std::time::Duration::ZERO)
            .unwrap();
        write_video_meta_source(&source, 100, 2048);
        let cancel = AtomicBool::new(false);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let (mutex_available, pin_write, saved) = std::thread::scope(|scope| {
            let writer_ref = &writer;
            let source_ref = &source;
            let cancel_ref = &cancel;
            let worker = scope.spawn(move || {
                writer_ref.set_video_meta_with_source_check(
                    "movie.mp4",
                    100,
                    2048,
                    &VideoMeta::Unreadable,
                    cancel_ref,
                    || {
                        entered_tx.send(()).unwrap();
                        resume_rx
                            .recv_timeout(std::time::Duration::from_secs(10))
                            .unwrap();
                        media_source_identity(source_ref)
                    },
                )
            });
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            let mutex_available = writer.conn.try_lock().is_ok();
            // This is the same write used by UI metadata pin refresh. A zero
            // busy timeout makes holding an IMMEDIATE lock fail deterministically.
            let pin_write = ui.delete_one(pin_key);
            resume_tx.send(()).unwrap();
            (mutex_available, pin_write, worker.join().unwrap())
        });
        assert!(mutex_available, "source stat held the catalog mutex");
        pin_write.expect("source stat held a SQLite write lock against the UI");
        assert!(saved.unwrap());
        assert_eq!(
            ui.get_video_meta("movie.mp4", 100, 2048).unwrap(),
            Some(VideoMeta::Unreadable)
        );
    }

    #[test]
    fn video_meta_cancel_before_or_during_source_stat_prevents_writes() {
        let db = open_in_memory();
        let cancel = AtomicBool::new(true);
        let mut stat_called = false;
        assert!(
            !db.set_video_meta_with_source_check(
                "movie.mp4",
                100,
                2048,
                &VideoMeta::Unreadable,
                &cancel,
                || {
                    stat_called = true;
                    Some((100, 2048))
                },
            )
            .unwrap()
        );
        assert!(!stat_called, "already canceled work must not start a stat");

        cancel.store(false, Ordering::Relaxed);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let saved = std::thread::scope(|scope| {
            let db_ref = &db;
            let cancel_ref = &cancel;
            let worker = scope.spawn(move || {
                db_ref.set_video_meta_with_source_check(
                    "movie.mp4",
                    100,
                    2048,
                    &VideoMeta::Unreadable,
                    cancel_ref,
                    || {
                        entered_tx.send(()).unwrap();
                        resume_rx
                            .recv_timeout(std::time::Duration::from_secs(10))
                            .unwrap();
                        Some((100, 2048))
                    },
                )
            });
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            cancel.store(true, Ordering::Relaxed);
            resume_tx.send(()).unwrap();
            worker.join().unwrap()
        });
        assert!(!saved.unwrap());
        assert_eq!(db.get_video_meta("movie.mp4", 100, 2048).unwrap(), None);
    }

    #[test]
    fn video_meta_source_change_after_stat_makes_delayed_row_a_current_identity_miss() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("movie.mp4");
        let cache = temp.path().join("cache");
        let old_worker = CatalogDb::open(&cache, temp.path()).unwrap();
        let new_worker = CatalogDb::open(&cache, temp.path()).unwrap();
        write_video_meta_source(&source, 100, 2048);
        let cancel = AtomicBool::new(false);
        let (checked_tx, checked_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let current = VideoMeta::Read {
            duration_secs: Some(42.0),
            dims: Some((1280, 720)),
            codec: Some("h264".into()),
        };
        let (new_saved, old_saved) = std::thread::scope(|scope| {
            let old_ref = &old_worker;
            let source_ref = &source;
            let cancel_ref = &cancel;
            let worker = scope.spawn(move || {
                old_ref.set_video_meta_with_source_check(
                    "movie.mp4",
                    100,
                    2048,
                    &VideoMeta::Unreadable,
                    cancel_ref,
                    || {
                        let identity = media_source_identity(source_ref);
                        checked_tx.send(()).unwrap();
                        resume_rx
                            .recv_timeout(std::time::Duration::from_secs(10))
                            .unwrap();
                        identity
                    },
                )
            });
            checked_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            write_video_meta_source(&source, 50, 4096);
            let new_saved =
                new_worker.set_video_meta(&source, "movie.mp4", 50, 4096, &current, &cancel);
            resume_tx.send(()).unwrap();
            (new_saved, worker.join().unwrap())
        });
        assert!(new_saved.unwrap());
        assert!(old_saved.unwrap());
        assert_eq!(media_source_identity(&source), Some((50, 4096)));
        assert_eq!(
            new_worker.get_video_meta("movie.mp4", 50, 4096).unwrap(),
            None
        );
        assert_eq!(
            old_worker.get_video_meta("movie.mp4", 100, 2048).unwrap(),
            Some(VideoMeta::Unreadable)
        );
    }

    #[test]
    fn video_meta_changed_source_is_rejected_in_both_completion_orders() {
        // A file identity is not ordered by mtime: replacements can be backdated
        // or keep the same timestamp while changing size.
        for new_identity in [(200, 4096), (50, 4096), (100, 4096)] {
            for old_finishes_first in [false, true] {
                let temp = tempfile::TempDir::new().unwrap();
                let source = temp.path().join("movie.mp4");
                let cache = temp.path().join("cache");
                let old_worker = CatalogDb::open(&cache, temp.path()).unwrap();
                let new_worker = CatalogDb::open(&cache, temp.path()).unwrap();
                write_video_meta_source(&source, 100, 2048);
                // Both workers captured their identities before completing;
                // either completion order must reject the obsolete result.
                write_video_meta_source(&source, new_identity.0, new_identity.1);
                if old_finishes_first {
                    assert!(
                        !old_worker
                            .set_video_meta(
                                &source,
                                "movie.mp4",
                                100,
                                2048,
                                &VideoMeta::Unreadable,
                                &AtomicBool::new(false)
                            )
                            .unwrap()
                    );
                }
                let current = VideoMeta::Read {
                    duration_secs: Some(42.0),
                    dims: Some((1280, 720)),
                    codec: Some("h264".into()),
                };
                assert!(
                    new_worker
                        .set_video_meta(
                            &source,
                            "movie.mp4",
                            new_identity.0,
                            new_identity.1,
                            &current,
                            &AtomicBool::new(false),
                        )
                        .unwrap()
                );
                if !old_finishes_first {
                    assert!(
                        !old_worker
                            .set_video_meta(
                                &source,
                                "movie.mp4",
                                100,
                                2048,
                                &VideoMeta::Unreadable,
                                &AtomicBool::new(false)
                            )
                            .unwrap()
                    );
                }
                assert_eq!(
                    old_worker
                        .get_video_meta("movie.mp4", new_identity.0, new_identity.1)
                        .unwrap(),
                    Some(current)
                );
                assert_eq!(
                    new_worker.get_video_meta("movie.mp4", 100, 2048).unwrap(),
                    None
                );
            }
        }
    }

    #[test]
    fn video_meta_publication_rejects_missing_source_and_accepts_known_zero_size() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("empty.mp4");
        let db = open_in_memory();
        assert!(
            !db.set_video_meta(
                &source,
                "empty.mp4",
                0,
                0,
                &VideoMeta::Unreadable,
                &AtomicBool::new(false)
            )
            .unwrap()
        );
        write_video_meta_source(&source, 0, 0);
        assert!(
            db.set_video_meta(
                &source,
                "empty.mp4",
                0,
                0,
                &VideoMeta::Unreadable,
                &AtomicBool::new(false)
            )
            .unwrap()
        );
        assert_eq!(
            db.get_video_meta("empty.mp4", 0, 0).unwrap(),
            Some(VideoMeta::Unreadable)
        );
    }

    #[test]
    fn video_meta_roundtrip_identity_and_negative_cache() {
        let db = open_in_memory();
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("movie.mp4");
        write_video_meta_source(&source, 100, 2048);
        let value = VideoMeta::Read {
            duration_secs: Some(123.456789),
            dims: Some((1920, 1080)),
            codec: Some("h264".into()),
        };
        assert!(
            db.set_video_meta(
                &source,
                "movie.mp4",
                100,
                2048,
                &value,
                &AtomicBool::new(false)
            )
            .unwrap()
        );
        assert_eq!(
            db.get_video_meta("movie.mp4", 100, 2048).unwrap(),
            Some(value)
        );
        assert_eq!(db.get_video_meta("movie.mp4", 101, 2048).unwrap(), None);
        assert_eq!(db.get_video_meta("movie.mp4", 100, 4096).unwrap(), None);
        write_video_meta_source(&source, 101, 4096);
        assert!(
            db.set_video_meta(
                &source,
                "movie.mp4",
                101,
                4096,
                &VideoMeta::Unreadable,
                &AtomicBool::new(false)
            )
            .unwrap()
        );
        assert_eq!(
            db.get_video_meta("movie.mp4", 101, 4096).unwrap(),
            Some(VideoMeta::Unreadable)
        );
        assert_eq!(db.get_video_meta("movie.mp4", 102, 4096).unwrap(), None);
        assert_eq!(db.get_video_meta("movie.mp4", 101, 4097).unwrap(), None);
        let conn = db.conn.lock().unwrap();
        let values: (Option<f64>, Option<u32>, Option<u32>, Option<String>) = conn.query_row(
            "SELECT duration_secs, width, height, codec FROM video_meta WHERE filename='movie.mp4'",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ).unwrap();
        assert_eq!(values, (None, None, None, None));
    }

    #[test]
    fn video_meta_audio_preserves_null_dimensions_and_unknown_duration() {
        let db = open_in_memory();
        let temp = tempfile::TempDir::new().unwrap();
        for (name, duration_secs) in [("music.flac", Some(65.25)), ("stream.mp3", None)] {
            let value = VideoMeta::Read {
                duration_secs,
                dims: None,
                codec: Some("flac".into()),
            };
            let source = temp.path().join(name);
            write_video_meta_source(&source, 100, 1024);
            assert!(
                db.set_video_meta(&source, name, 100, 1024, &value, &AtomicBool::new(false))
                    .unwrap()
            );
            assert_eq!(db.get_video_meta(name, 100, 1024).unwrap(), Some(value));
            let dims: (Option<u32>, Option<u32>) = db
                .conn
                .lock()
                .unwrap()
                .query_row(
                    "SELECT width, height FROM video_meta WHERE filename=?1",
                    [name],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(dims, (None, None));
        }
    }

    #[test]
    fn video_meta_schema_addition_preserves_existing_metadata() {
        let db = open_in_memory();
        db.set_pdf_meta("book.pdf", 100, 1024, 42, true).unwrap();
        db.set_container_page_meta("book.zip", ContainerPageKind::Zip, 100, 2048, 0, Some(30))
            .unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute("DROP TABLE video_meta", []).unwrap();
            init_schema(&conn).unwrap();
        }
        assert_eq!(
            db.get_pdf_meta("book.pdf", 100, 1024).unwrap(),
            Some((42, true))
        );
        assert_eq!(
            db.get_container_page_meta("book.zip", ContainerPageKind::Zip, 100, 2048, 0)
                .unwrap(),
            Some(ContainerPageMeta {
                page_count: Some(30)
            })
        );
        assert_eq!(db.get_video_meta("new.mp4", 100, 1024).unwrap(), None);
    }

    #[test]
    fn container_page_meta_roundtrip_and_identity_invalidation() {
        let db = open_in_memory();
        db.set_container_page_meta("book.zip", ContainerPageKind::Zip, 100, 2_048, 0, Some(123))
            .unwrap();
        db.set_container_page_meta(
            "book.rar",
            ContainerPageKind::Archive,
            300,
            4_096,
            77,
            Some(45),
        )
        .unwrap();

        assert_eq!(
            db.get_container_page_meta("book.zip", ContainerPageKind::Zip, 100, 2_048, 0)
                .unwrap(),
            Some(ContainerPageMeta {
                page_count: Some(123)
            })
        );
        assert_eq!(
            db.get_container_page_meta("book.zip", ContainerPageKind::Zip, 101, 2_048, 0)
                .unwrap(),
            None,
            "mtime が変われば再走査する"
        );
        assert_eq!(
            db.get_container_page_meta("book.zip", ContainerPageKind::Zip, 100, 4_096, 0)
                .unwrap(),
            None,
            "サイズが変われば再走査する"
        );
        assert_eq!(
            db.get_container_page_meta("book.zip", ContainerPageKind::Folder, 100, 2_048, 0)
                .unwrap(),
            None,
            "同名でもコンテナ種別を混同しない"
        );
        assert_eq!(
            db.get_container_page_meta("book.rar", ContainerPageKind::Archive, 300, 4_096, 77,)
                .unwrap(),
            Some(ContainerPageMeta {
                page_count: Some(45)
            }),
            "変換対象アーカイブのページ数を独立して保存する"
        );
    }

    #[test]
    fn container_page_meta_preserves_non_book_and_fingerprint() {
        let db = open_in_memory();
        db.set_container_page_meta("pictures", ContainerPageKind::Folder, 200, 0, 77, None)
            .unwrap();

        assert_eq!(
            db.get_container_page_meta("pictures", ContainerPageKind::Folder, 200, 0, 77)
                .unwrap(),
            Some(ContainerPageMeta { page_count: None }),
            "走査済みの対象外フォルダは NULL として区別する"
        );
        assert_eq!(
            db.get_container_page_meta("pictures", ContainerPageKind::Folder, 200, 0, 78)
                .unwrap(),
            None,
            "判定設定が変われば再走査する"
        );
    }

    // -- WebP encode/decode --

    #[test]
    fn encode_thumb_webp_basic() {
        // 小さな 4x4 テスト画像を生成
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(4, 4, |x, y| {
            image::Rgb([(x * 60) as u8, (y * 60) as u8, 128])
        }));
        let result = encode_thumb_webp(&img, 4, 75.0);
        assert!(result.is_some());
        let (data, w, h) = result.unwrap();
        assert!(!data.is_empty());
        assert!(w <= 4 && h <= 4);
    }

    /// `collect_db_paths` が `cache_dir/<sub>/*.db` を網羅すること。
    /// `cache_dir/file.db` (top-level) は subdir でないので **無視**、
    /// 非 .db ファイル / 余計なフォルダの中の非 .db も無視。
    /// docs/ui-responsiveness.md §4 (file_type 経由) との整合を機能面から保証する。
    #[test]
    fn collect_db_paths_enumerates_only_subdir_db_files() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache_dir = temp.path().join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        // sub1: foo.db + readme.txt
        let sub1 = cache_dir.join("sub1");
        std::fs::create_dir_all(&sub1).unwrap();
        std::fs::write(sub1.join("foo.db"), b"x").unwrap();
        std::fs::write(sub1.join("readme.txt"), b"x").unwrap();
        // sub2: bar.db
        let sub2 = cache_dir.join("sub2");
        std::fs::create_dir_all(&sub2).unwrap();
        std::fs::write(sub2.join("bar.db"), b"x").unwrap();
        // top-level の loose db (subdir に居ない) は拾わない
        std::fs::write(cache_dir.join("loose.db"), b"x").unwrap();
        // 空サブフォルダは無害
        std::fs::create_dir_all(cache_dir.join("empty_sub")).unwrap();

        let mut found: Vec<String> = Vec::new();
        super::collect_db_paths(&cache_dir, &mut |p, _meta| {
            found.push(
                p.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string(),
            );
        });
        found.sort();
        assert_eq!(
            found,
            vec!["bar.db".to_string(), "foo.db".to_string()],
            "subdir 配下の .db のみ列挙、top-level loose.db は無視"
        );
    }

    /// `collect_db_paths` は cache_dir 自体が存在しない場合に panic せず、
    /// 単にコールバックを呼ばずに return する (`std::fs::read_dir` Err 時の規約)。
    #[test]
    fn collect_db_paths_handles_missing_cache_dir() {
        let temp = tempfile::TempDir::new().unwrap();
        let nonexistent = temp.path().join("does_not_exist");
        let mut count = 0usize;
        super::collect_db_paths(&nonexistent, &mut |_, _| count += 1);
        assert_eq!(count, 0, "missing cache_dir なら空列挙");
    }

    /// 大量サブフォルダ (200 件) でも全 .db ファイルを取りこぼさず列挙する。
    /// 実時間 assert は flaky になるので、件数だけ厳密に確認 (file_type 経路で
    /// per-entry syscall が発生していないことの間接担保)。
    #[test]
    fn collect_db_paths_handles_many_subfolders() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache_dir = temp.path().join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        for i in 0..200 {
            let sub = cache_dir.join(format!("s{i:03}"));
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join("a.db"), b"x").unwrap();
        }
        let mut count = 0usize;
        super::collect_db_paths(&cache_dir, &mut |_, _| count += 1);
        assert_eq!(count, 200, "200 件全部列挙");
    }

    /// 0.8.2 で `decode_thumb_dims` を WebP 固定から `with_guessed_format()` auto-detect
    /// に変更した回帰ガード。ここで JPEG が読めなくなると、旧バージョンが JPEG で書いた
    /// 親 catalog エントリから seed/writeback できなくなる (= 仮想フォルダの初回 thumb
    /// が永続的に失われる)。
    #[test]
    fn decode_thumb_dims_reads_webp_jpeg_and_rejects_garbage() {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(8, 6, |x, y| {
            image::Rgb([(x * 30) as u8, (y * 40) as u8, 200])
        }));

        // WebP (現行フォーマット): 寸法を返す
        let (webp_bytes, _, _) = encode_thumb_webp(&img, 8, 75.0).expect("webp encode ok");
        assert_eq!(decode_thumb_dims(&webp_bytes), Some((8, 6)));

        // JPEG (旧バージョンが書いていた形式): 寸法を返す
        let mut jpeg_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut jpeg_bytes),
            image::ImageFormat::Jpeg,
        )
        .expect("jpeg encode");
        assert_eq!(decode_thumb_dims(&jpeg_bytes), Some((8, 6)));

        // 破損データ: None。空バイト列・テキスト・WebP magic だけ・短縮 JPEG いずれも reject。
        assert_eq!(decode_thumb_dims(&[]), None);
        assert_eq!(decode_thumb_dims(b"NOT-AN-IMAGE-AT-ALL"), None);
        // RIFF/WEBP magic の手前 12 バイトだけ (本体なし)
        assert_eq!(
            decode_thumb_dims(b"RIFF\x00\x00\x00\x00WEBP"),
            None,
            "header だけで本体なし → None"
        );
        // JPEG SOI のみ (SOF0 まで届かない)
        assert_eq!(decode_thumb_dims(b"\xFF\xD8\xFF\xE0"), None);
    }

    // -- pdf_meta --

    #[test]
    fn pdf_meta_set_and_get_roundtrip() {
        let db = open_in_memory();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, false).unwrap();

        let result = db.get_pdf_meta("foo.pdf", 1000, 2048).unwrap();
        assert_eq!(result, Some((32, false)));
    }

    #[test]
    fn pdf_meta_mtime_mismatch_returns_none() {
        let db = open_in_memory();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, false).unwrap();

        // mtime が変わったら cache miss
        let result = db.get_pdf_meta("foo.pdf", 1001, 2048).unwrap();
        assert_eq!(result, None, "mtime 変化で cache miss");
    }

    #[test]
    fn pdf_meta_file_size_mismatch_returns_none() {
        let db = open_in_memory();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, false).unwrap();

        // file_size が変わったら cache miss
        let result = db.get_pdf_meta("foo.pdf", 1000, 4096).unwrap();
        assert_eq!(result, None, "file_size 変化で cache miss");
    }

    #[test]
    fn pdf_meta_password_required_flag_preserved() {
        let db = open_in_memory();
        db.set_pdf_meta("locked.pdf", 100, 500, 8, true).unwrap();

        let result = db.get_pdf_meta("locked.pdf", 100, 500).unwrap();
        assert_eq!(result, Some((8, true)));
    }

    #[test]
    fn pdf_meta_insert_or_replace() {
        let db = open_in_memory();
        // 同じ filename で 2 回 set → 2 回目で上書き
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, false).unwrap();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 100, true).unwrap();

        let result = db.get_pdf_meta("foo.pdf", 1000, 2048).unwrap();
        assert_eq!(result, Some((100, true)), "2 回目の値が残る");
    }

    #[test]
    fn pdf_meta_get_missing_returns_none() {
        let db = open_in_memory();
        let result = db.get_pdf_meta("nonexistent.pdf", 1000, 2048).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn pdf_meta_does_not_affect_thumbnails_table() {
        // pdf_meta テーブルと thumbnails テーブルが独立していることを確認
        let db = open_in_memory();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, false).unwrap();

        let all = db.load_all().unwrap();
        assert!(all.is_empty(), "thumbnails は空のまま");
    }

    #[test]
    fn pdf_meta_thumb_preserves_password_required_flag() {
        // unknown 経路 (session-only pw など) の verify update が password_required を
        // 消さないこと (mtime/size 一致の場合のみ page_count update が走る)
        let db = open_in_memory();
        // 既存: パスワード必須として記録済み
        db.set_pdf_meta("locked.pdf", 1000, 2048, 32, true).unwrap();
        // 同じ mtime/size での unknown 経路 update (page_count もたまたま同じ)
        db.set_pdf_meta_thumb("locked.pdf", 1000, 2048, 32).unwrap();

        let result = db.get_pdf_meta("locked.pdf", 1000, 2048).unwrap();
        assert_eq!(
            result,
            Some((32, true)),
            "password_required=true が保持される"
        );
    }

    #[test]
    fn pdf_meta_thumb_does_not_insert_new_row() {
        // **Codex P1 round 2 対応**: 新規 PDF (= まだ pdf_meta 行が無い) で
        // unknown 経路 (set_pdf_meta_thumb) を呼んでも、false-default 行を作らない。
        // 保護 PDF を「パスワード入力したが保存しない」で開いたケースで、永続的に
        // 「非保護」と記録されてしまう bypass を防ぐ。
        let db = open_in_memory();
        db.set_pdf_meta_thumb("new.pdf", 500, 1024, 16).unwrap();

        let result = db.get_pdf_meta("new.pdf", 500, 1024).unwrap();
        assert_eq!(result, None, "新規行は作られない (UPDATE only)");
    }

    #[test]
    fn pdf_meta_thumb_does_not_promote_stale_row() {
        // **Codex P1 round 3 対応**: 旧 stale 行 (例: 非暗号化として cache 済み) を、
        // ファイル置換後 (暗号化版、新 mtime/size) の unknown 経路 update で
        // 新 mtime/size に上書きすると password_required=0 のまま昇格してしまう。
        // mtime/file_size 一致条件で no-op にする実装で防止する。
        let db = open_in_memory();
        // 旧: 非暗号化として記録 (mtime=1000, size=2000)
        db.set_pdf_meta("foo.pdf", 1000, 2000, 10, false).unwrap();
        // ファイル更新後、ユーザーが新版を session pw で開いて unknown 経路 update が来た
        // (新 mtime=2000, size=3000) — 旧 stale 行を promote しようとする
        db.set_pdf_meta_thumb("foo.pdf", 2000, 3000, 20).unwrap();

        // 新 mtime/size での lookup は cache miss (stale 行は古いまま、新値で promote されない)
        let new_lookup = db.get_pdf_meta("foo.pdf", 2000, 3000).unwrap();
        assert_eq!(
            new_lookup, None,
            "新 mtime/size の cache hit が起こらない (stale 行 promote されず)"
        );
        // 旧 mtime/size の行はそのまま (page_count=10, password_required=false)
        let old_lookup = db.get_pdf_meta("foo.pdf", 1000, 2000).unwrap();
        assert_eq!(
            old_lookup,
            Some((10, false)),
            "旧 mtime/size の行は元のまま変更されない"
        );
    }

    #[test]
    fn pdf_meta_thumb_updates_page_count_when_mtime_size_match() {
        // mtime/file_size が既存行と一致するときは page_count を更新する (verify update)。
        // password_required 列は保持。
        let db = open_in_memory();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, true).unwrap();
        // 同じ mtime/size で page_count だけ違う update (例: 別経路でカウントを再計測)
        db.set_pdf_meta_thumb("foo.pdf", 1000, 2048, 35).unwrap();

        let result = db.get_pdf_meta("foo.pdf", 1000, 2048).unwrap();
        assert_eq!(
            result,
            Some((35, true)),
            "page_count 更新、password_required は保持"
        );
    }

    #[test]
    fn pdf_meta_thumb_noop_when_only_mtime_differs() {
        // **Codex P3 round 4 対応**: mtime だけが異なる stale 状況でも promote しない。
        let db = open_in_memory();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, true).unwrap();
        // 新 mtime=2000 (size 同じ) で update が来た → 一致条件外
        db.set_pdf_meta_thumb("foo.pdf", 2000, 2048, 50).unwrap();

        // 新 mtime での lookup は cache miss (stale 行は古いまま)
        let new_lookup = db.get_pdf_meta("foo.pdf", 2000, 2048).unwrap();
        assert_eq!(new_lookup, None);
        // 旧 mtime での行は不変
        let old_lookup = db.get_pdf_meta("foo.pdf", 1000, 2048).unwrap();
        assert_eq!(old_lookup, Some((32, true)));
    }

    #[test]
    fn pdf_meta_thumb_noop_when_only_file_size_differs() {
        // **Codex P3 round 4 対応**: file_size だけが異なる stale 状況でも promote しない。
        let db = open_in_memory();
        db.set_pdf_meta("foo.pdf", 1000, 2048, 32, true).unwrap();
        // 新 size=4096 (mtime 同じ) で update が来た → 一致条件外
        db.set_pdf_meta_thumb("foo.pdf", 1000, 4096, 50).unwrap();

        // 新 size での lookup は cache miss
        let new_lookup = db.get_pdf_meta("foo.pdf", 1000, 4096).unwrap();
        assert_eq!(new_lookup, None);
        // 旧 size での行は不変
        let old_lookup = db.get_pdf_meta("foo.pdf", 1000, 2048).unwrap();
        assert_eq!(old_lookup, Some((32, true)));
    }

    #[test]
    fn pdf_meta_safe_inserts_new_row_with_false() {
        // password=None 確定経路: 新規行を password_required=false で挿入する
        let db = open_in_memory();
        db.set_pdf_meta_safe("new.pdf", 500, 1024, 16).unwrap();

        let result = db.get_pdf_meta("new.pdf", 500, 1024).unwrap();
        assert_eq!(result, Some((16, false)), "新規行は false で挿入");
    }

    #[test]
    fn pdf_meta_safe_overrides_password_required_on_existing_row() {
        // **review #1 対応**: 既存行に password_required=true があっても、safe 経路
        // (= None password で render 成功 = 非保護確信あり) は上書きで 0 にする。
        // 保護版を非保護版に同名差し替えしたとき、stale な「保護」フラグを
        // 永続化しないため。
        let db = open_in_memory();
        db.set_pdf_meta("locked.pdf", 100, 500, 8, true).unwrap();
        db.set_pdf_meta_safe("locked.pdf", 200, 600, 10).unwrap();

        let result = db.get_pdf_meta("locked.pdf", 200, 600).unwrap();
        assert_eq!(
            result,
            Some((10, false)),
            "safe 経路の確信 (password_required=false) で上書きされる"
        );
    }
}
