//! 編集済みコンテンツの物理 identity を記録する append-on-edit ledger。
//!
//! UI 側は物理ファイル 1 件を channel へ渡すだけにし、metadata 取得・
//! ファイル読み出し・SHA-256・SQLite はすべて専用 worker が行う。
//! schema 作成と upgrade は `PRAGMA user_version` を正本に 1 箇所で行い、
//! 過去ビルドが作った台帳も open 時に現在形へ引き上げる。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};

mod restore;

pub(crate) use restore::{
    ContentRestoreReport, DeclinedRestore, InternalByteCopyDeclineRecorder, RestorePresence,
    RestoreSidecarMirror, SelectedRestore, restore_candidates_at,
};

const HEAD_HASH_BYTES: u64 = 64 * 1024;
const HASH_CHUNK_BYTES: usize = 256 * 1024;
const CONTENT_IDENTITY_SCHEMA_VERSION: i64 = 1;

/// 台帳そのもののファイル名。`STORES` にも載っている (復元先へ行を写すため) が、
/// 「復元して何か起きるか」を数えるときは**除外する**。台帳の行はその問いの
/// 対象であって答えではない。
pub(crate) const LEDGER_DB_FILE: &str = "content_identity.db";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ContentIdentityTrigger {
    ViewingState,
    Edit,
}

/// `edit_origin` への観測が復元可能な状態そのものか、検出時の hash cache だけかを表す。
/// `ContentIdentityTrigger` は最終編集時刻を進めるかどうかの軸なので、混同しない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObservationRole {
    RestorableContent,
    DetectionCache,
}

impl ObservationRole {
    fn has_restorable_content(self) -> bool {
        matches!(self, Self::RestorableContent)
    }
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

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "image" => Some(Self::Image),
            "zip" => Some(Self::Zip),
            "pdf" => Some(Self::Pdf),
            "convertible" => Some(Self::Convertible),
            _ => None,
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

/// `edit_origin` のメモリ表現。A2 の段 0 はこの snapshot だけを参照し、
/// ファイルや SQLite には触れない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LedgerEntry {
    pub(crate) file_key: String,
    pub(crate) size: u64,
    pub(crate) head_hash: String,
    pub(crate) full_hash: Option<String>,
    pub(crate) hashed_mtime: i64,
    pub(crate) kind: ContentKind,
    pub(crate) last_edit_at: i64,
    pub(crate) has_restorable_content: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ContentIdentityIndex {
    entries_by_size: BTreeMap<u64, Vec<LedgerEntry>>,
    size_by_file_key: HashMap<String, u64>,
    ledger_file_keys: HashSet<String>,
}

impl ContentIdentityIndex {
    fn from_entries(entries: Vec<LedgerEntry>) -> Self {
        let mut index = Self::default();
        for entry in entries {
            index.upsert(entry);
        }
        index
    }

    pub(crate) fn contains_file_key(&self, file_key: &str) -> bool {
        self.size_by_file_key.contains_key(file_key)
    }

    /// 復元元と detection hash cache の両方を含む、台帳上の全物理キー。
    /// 段 0 は cache 行で抑止してはいけないため `contains_file_key` と分ける。
    pub(crate) fn contains_ledger_file_key(&self, file_key: &str) -> bool {
        self.ledger_file_keys.contains(file_key)
    }

    pub(crate) fn len(&self) -> usize {
        self.size_by_file_key.len()
    }

    pub(crate) fn entries_for_size(&self, size: u64) -> &[LedgerEntry] {
        self.entries_by_size
            .get(&size)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub(crate) fn upsert(&mut self, entry: LedgerEntry) {
        self.ledger_file_keys.insert(entry.file_key.clone());
        // DB の flag は 0 -> 1 の単調遷移。遅れて届いた detection cache (0) が、
        // 先に届いた A1 の復元元更新 (1) をメモリ索引から消してはならない。
        if !entry.has_restorable_content {
            return;
        }
        if let Some(old_size) = self.size_by_file_key.remove(&entry.file_key) {
            let remove_bucket = if let Some(entries) = self.entries_by_size.get_mut(&old_size) {
                entries.retain(|existing| existing.file_key != entry.file_key);
                entries.is_empty()
            } else {
                false
            };
            if remove_bucket {
                self.entries_by_size.remove(&old_size);
            }
        }
        self.size_by_file_key
            .insert(entry.file_key.clone(), entry.size);
        self.entries_by_size
            .entry(entry.size)
            .or_default()
            .push(entry);
    }
}

/// A2/A4 が参照する台帳の利用可否。schema/open 失敗を空索引へ潰さない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ContentIdentityLedgerState {
    Disabled,
    Loading,
    Ready,
    Unusable(String),
}

impl ContentIdentityLedgerState {
    pub(crate) fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    pub(crate) fn unusable_detail(&self) -> Option<&str> {
        match self {
            Self::Unusable(detail) => Some(detail),
            Self::Disabled | Self::Loading | Self::Ready => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DetectionTarget {
    pub(crate) source: ContentIdentitySource,
    pub(crate) file_key: String,
    pub(crate) size: u64,
    origins: Vec<LedgerEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestoreSourceCandidate {
    pub(crate) file_key: String,
    pub(crate) path: PathBuf,
    pub(crate) kind: ContentKind,
    pub(crate) last_edit_at: i64,
    pub(crate) source_exists: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestoreCandidate {
    pub(crate) target_key: String,
    pub(crate) target_path: PathBuf,
    pub(crate) target_kind: ContentKind,
    pub(crate) full_hash: String,
    pub(crate) sources: Vec<RestoreSourceCandidate>,
}

#[derive(Clone, Debug)]
pub(crate) struct DetectionResult {
    pub(crate) items_generation: u64,
    pub(crate) folder_key: String,
    pub(crate) candidates: Vec<RestoreCandidate>,
    pub(crate) ledger_updates: Vec<LedgerEntry>,
}

/// A2 段 0。呼び出し側は物理フォルダ一覧と既存編集なしを確認済みの item だけを渡す。
/// この関数はメモリ上の size index を見るだけで、I/O は一切行わない。
pub(crate) fn stage0_target(
    index: &ContentIdentityIndex,
    source: ContentIdentitySource,
    size: u64,
) -> Option<DetectionTarget> {
    let file_key = crate::path_key::normalize_keep_drive(&source.path);
    if index.contains_file_key(&file_key) {
        return None;
    }
    let origins = index
        .entries_for_size(size)
        .iter()
        .filter(|entry| entry.file_key != file_key)
        .cloned()
        .collect::<Vec<_>>();
    (!origins.is_empty()).then_some(DetectionTarget {
        source,
        file_key,
        size,
        origins,
    })
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
    has_restorable_content: bool,
}

impl ContentIdentityDb {
    fn open() -> Result<Self, String> {
        Self::open_at(&crate::data_dir::get().join("content_identity.db"))
    }

    fn open_at(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut conn = rusqlite::Connection::open(path).map_err(|error| error.to_string())?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|error| error.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             ",
        )
        .map_err(|error| error.to_string())?;
        ensure_content_identity_schema(&mut conn)?;
        Ok(Self { conn })
    }

    fn recorded_state(&self, file_key: &str) -> Result<Option<StoredRecord>, String> {
        self.conn
            .query_row(
                "SELECT file_key, size, hashed_mtime, last_edit_at, has_restorable_content
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
                        has_restorable_content: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    fn ledger_entry(&self, file_key: &str) -> Result<Option<LedgerEntry>, String> {
        self.conn
            .query_row(
                "SELECT file_key, size, head_hash, full_hash, hashed_mtime, kind, last_edit_at,
                        has_restorable_content
                   FROM edit_origin WHERE file_key = ?1",
                [file_key],
                ledger_entry_from_row,
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    fn load_index(&self, cancel: &AtomicBool) -> Result<Option<ContentIdentityIndex>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT file_key, size, head_hash, full_hash, hashed_mtime, kind, last_edit_at,
                        has_restorable_content
                   FROM edit_origin",
            )
            .map_err(|error| error.to_string())?;
        let mut rows = stmt.query([]).map_err(|error| error.to_string())?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            if cancel.load(Ordering::Acquire) {
                return Ok(None);
            }
            entries.push(ledger_entry_from_row(row).map_err(|error| error.to_string())?);
        }
        if cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        Ok(Some(ContentIdentityIndex::from_entries(entries)))
    }

    fn restore_was_declined(&self, full_hash: &str, target_key: &str) -> Result<bool, String> {
        self.conn
            .query_row(
                "SELECT 1 FROM restore_declined WHERE full_hash = ?1 AND target_key = ?2",
                rusqlite::params![full_hash, target_key],
                |_| Ok(()),
            )
            .optional()
            .map(|value| value.is_some())
            .map_err(|error| error.to_string())
    }

    fn mark_restorable(
        &self,
        file_key: &str,
        kind: ContentKind,
        last_edit_at: Option<i64>,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE edit_origin
                    SET kind = ?2,
                        last_edit_at = COALESCE(?3, last_edit_at),
                        has_restorable_content = 1
                  WHERE file_key = ?1",
                rusqlite::params![file_key, kind.as_str(), last_edit_at],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// 自分が下ろした flag を戻す。`clear_restorable_if_unchanged` の直後に store を
    /// 読み直して行が見つかった (= probe と clear の間に編集が入った) ときだけ呼ぶ。
    ///
    /// `kind` と `last_edit_at` は触らない。取り消すのは自分の clear だけで、その編集の
    /// 記録は編集側の経路が持っている。
    fn restore_restorable_flag(&self, file_key: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE edit_origin
                    SET has_restorable_content = 1
                  WHERE file_key = ?1",
                rusqlite::params![file_key],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// 復元元 flag を下ろす。**store を全部読んで 1 行も無いと確認できたときだけ**呼ぶ。
    ///
    /// 通常の書き込みで flag は 0 -> 1 の単調遷移にしてある (遅れて届いた
    /// detection cache の 0 が、先に届いた復元元更新の 1 を消さないため)。ここはその
    /// 例外で、「実データがもう無い」という別の根拠に基づく明示的な取り消し。
    ///
    /// `last_edit_at` の一致を条件にして compare-and-swap にする。probe 中に UI
    /// スレッドが新しい編集を記録していたら 0 行更新で終わり、その編集は残る。
    ///
    /// **期待値は probe より前に読んだものを渡すこと。** probe の後に読むと、その値が
    /// 既に並行編集の timestamp になっていて CAS が必ず成立し、守るはずの編集を消す
    /// (2026-08-29 レビュー R-05)。同じミリ秒に 2 つの編集が入って timestamp で
    /// 見分けられない場合は、呼び出し側の clear 後の再 probe が行の有無で拾う。
    fn clear_restorable_if_unchanged(
        &self,
        file_key: &str,
        last_edit_at: i64,
    ) -> Result<bool, String> {
        self.conn
            .execute(
                "UPDATE edit_origin
                    SET has_restorable_content = 0
                  WHERE file_key = ?1
                    AND last_edit_at = ?2
                    AND has_restorable_content = 1",
                rusqlite::params![file_key, last_edit_at],
            )
            .map(|rows| rows > 0)
            .map_err(|error| error.to_string())
    }

    fn upsert(
        &self,
        source: &ContentIdentitySource,
        state: &RecordedFileState,
        head_hash: &str,
        full_hash: &str,
        last_edit_at: i64,
        role: ObservationRole,
    ) -> Result<(), String> {
        let size = i64::try_from(state.size)
            .map_err(|_| "file size exceeds SQLite INTEGER".to_string())?;
        self.conn
            .execute(
                "INSERT INTO edit_origin
                     (file_key, size, head_hash, full_hash, hashed_mtime, kind, last_edit_at,
                      has_restorable_content)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(file_key) DO UPDATE SET
                     size = excluded.size,
                     head_hash = excluded.head_hash,
                     full_hash = excluded.full_hash,
                     hashed_mtime = excluded.hashed_mtime,
                     kind = excluded.kind,
                     last_edit_at = excluded.last_edit_at,
                     has_restorable_content = MAX(
                         edit_origin.has_restorable_content,
                         excluded.has_restorable_content
                     )",
                rusqlite::params![
                    state.file_key,
                    size,
                    head_hash,
                    full_hash,
                    state.hashed_mtime,
                    source.kind.as_str(),
                    last_edit_at,
                    role.has_restorable_content(),
                ],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// 新規作成と過去 schema からの upgrade を担う唯一の入口。
/// version 0 は A1 の unversioned schema、version 1 は現在 schema を表す。
fn ensure_content_identity_schema(conn: &mut rusqlite::Connection) -> Result<(), String> {
    let version = conn
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|error| format!("read PRAGMA user_version: {error}"))?;
    if !(0..=CONTENT_IDENTITY_SCHEMA_VERSION).contains(&version) {
        return Err(format!(
            "unsupported content identity schema version {version} (current {})",
            CONTENT_IDENTITY_SCHEMA_VERSION
        ));
    }

    let transaction = conn.transaction().map_err(|error| error.to_string())?;
    if version == 0 {
        if !schema_object_exists(&transaction, "table", "edit_origin")? {
            transaction
                .execute_batch(
                    "CREATE TABLE edit_origin (
                         file_key TEXT PRIMARY KEY,
                         size INTEGER NOT NULL,
                         head_hash TEXT NOT NULL,
                         full_hash TEXT,
                         hashed_mtime INTEGER NOT NULL,
                         kind TEXT NOT NULL,
                         last_edit_at INTEGER NOT NULL,
                         has_restorable_content INTEGER NOT NULL DEFAULT 1
                     );",
                )
                .map_err(|error| format!("create edit_origin: {error}"))?;
        } else if !table_columns(&transaction, "edit_origin")?.contains("has_restorable_content") {
            transaction
                .execute_batch(
                    "ALTER TABLE edit_origin
                         ADD COLUMN has_restorable_content INTEGER NOT NULL DEFAULT 1;",
                )
                .map_err(|error| format!("upgrade edit_origin to schema v1: {error}"))?;
        }
        transaction
            .execute_batch(
                "CREATE INDEX IF NOT EXISTS edit_origin_full ON edit_origin(full_hash);
                 CREATE TABLE IF NOT EXISTS restore_declined (
                     full_hash TEXT NOT NULL,
                     target_key TEXT NOT NULL,
                     PRIMARY KEY(full_hash, target_key)
                 );",
            )
            .map_err(|error| format!("complete content identity schema v1: {error}"))?;
        transaction
            .pragma_update(None, "user_version", CONTENT_IDENTITY_SCHEMA_VERSION)
            .map_err(|error| format!("write PRAGMA user_version: {error}"))?;
    }

    validate_content_identity_schema(&transaction)?;
    transaction.commit().map_err(|error| error.to_string())
}

fn schema_object_exists(
    conn: &rusqlite::Connection,
    object_type: &str,
    name: &str,
) -> Result<bool, String> {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = ?1 AND name = ?2",
        rusqlite::params![object_type, name],
        |_| Ok(()),
    )
    .optional()
    .map(|value| value.is_some())
    .map_err(|error| error.to_string())
}

fn table_columns(conn: &rusqlite::Connection, table: &str) -> Result<HashSet<String>, String> {
    let sql = format!("PRAGMA table_info({table})");
    let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
    statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| error.to_string())?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|error| error.to_string())
}

fn validate_content_identity_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    for (table, expected) in [
        (
            "edit_origin",
            &[
                "file_key",
                "size",
                "head_hash",
                "full_hash",
                "hashed_mtime",
                "kind",
                "last_edit_at",
                "has_restorable_content",
            ][..],
        ),
        ("restore_declined", &["full_hash", "target_key"][..]),
    ] {
        let columns = table_columns(conn, table)?;
        for column in expected {
            if !columns.contains(*column) {
                return Err(format!(
                    "content identity schema v{} is missing {table}.{column}",
                    CONTENT_IDENTITY_SCHEMA_VERSION
                ));
            }
        }
    }
    if !schema_object_exists(conn, "index", "edit_origin_full")? {
        return Err(format!(
            "content identity schema v{} is missing edit_origin_full",
            CONTENT_IDENTITY_SCHEMA_VERSION
        ));
    }
    Ok(())
}

fn ledger_entry_from_row(row: &rusqlite::Row<'_>) -> Result<LedgerEntry, rusqlite::Error> {
    let size: i64 = row.get(1)?;
    let size = u64::try_from(size).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })?;
    let kind_value: String = row.get(5)?;
    let kind = ContentKind::from_str(&kind_value).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Text,
            Box::new(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown content identity kind: {kind_value}"),
            )),
        )
    })?;
    Ok(LedgerEntry {
        file_key: row.get(0)?,
        size,
        head_hash: row.get(2)?,
        full_hash: row.get(3)?,
        hashed_mtime: row.get(4)?,
        kind,
        last_edit_at: row.get(6)?,
        has_restorable_content: row.get(7)?,
    })
}

pub(crate) struct ContentIdentityIndexLoadPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<ContentIdentityIndex, String>>,
}

impl ContentIdentityIndexLoadPending {
    /// A2 の索引ロード。設定 OFF では呼び出し側がこの worker 自体を作らない。
    pub(crate) fn spawn(
        io_sem: Arc<crate::io_semaphore::GlobalIoSemaphore>,
    ) -> Result<Self, String> {
        Self::spawn_at(crate::data_dir::get().join("content_identity.db"), io_sem)
    }

    fn spawn_at(
        db_path: PathBuf,
        io_sem: Arc<crate::io_semaphore::GlobalIoSemaphore>,
    ) -> Result<Self, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("content-identity-index-load".into())
            .spawn(move || {
                let result = io_sem
                    .acquire_cancellable(crate::io_semaphore::IoPriority::Low, &worker_cancel)
                    .ok_or_else(|| "cancelled".to_string())
                    .and_then(|_permit| ContentIdentityDb::open_at(&db_path))
                    .and_then(|db| db.load_index(&worker_cancel))
                    .and_then(|index| index.ok_or_else(|| "cancelled".to_string()));
                if !worker_cancel.load(Ordering::Acquire) {
                    let _ = tx.send(result);
                }
            })
            .map(|_| Self { cancel, rx })
            .map_err(|error| error.to_string())
    }

    pub(crate) fn try_recv(
        &self,
    ) -> Result<Result<ContentIdentityIndex, String>, mpsc::TryRecvError> {
        self.rx.try_recv()
    }

    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn for_test(result: Result<ContentIdentityIndex, String>) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        tx.send(result).unwrap();
        Self { cancel, rx }
    }
}

impl Drop for ContentIdentityIndexLoadPending {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub(crate) struct ContentIdentityRecorder {
    tx: Option<mpsc::Sender<RecordRequest>>,
    update_tx: mpsc::Sender<LedgerEntry>,
    update_rx: mpsc::Receiver<LedgerEntry>,
    handle: Option<std::thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl ContentIdentityRecorder {
    pub(crate) fn spawn() -> Option<Self> {
        let (tx, rx) = mpsc::channel::<RecordRequest>();
        let (update_tx, update_rx) = mpsc::channel::<LedgerEntry>();
        let worker_update_tx = update_tx.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        match std::thread::Builder::new()
            .name("content-identity-recorder".into())
            .spawn(move || run_worker(rx, worker_update_tx, worker_shutdown))
        {
            Ok(handle) => Some(Self {
                tx: Some(tx),
                update_tx,
                update_rx,
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

    pub(crate) fn drain_updates(&self) -> Vec<LedgerEntry> {
        self.update_rx.try_iter().collect()
    }

    pub(crate) fn update_sender(&self) -> mpsc::Sender<LedgerEntry> {
        self.update_tx.clone()
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

pub(crate) struct ContentIdentityDetectionPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<DetectionResult>,
}

impl ContentIdentityDetectionPending {
    pub(crate) fn spawn(
        targets: Vec<DetectionTarget>,
        items_generation: u64,
        folder_key: String,
        input_seq: u64,
        io_sem: Arc<crate::io_semaphore::GlobalIoSemaphore>,
        update_tx: Option<mpsc::Sender<LedgerEntry>>,
    ) -> Option<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let worker_folder_key = folder_key.clone();
        let db_path = crate::data_dir::get().join("content_identity.db");
        match std::thread::Builder::new()
            .name("content-identity-detect".into())
            .spawn(move || {
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "content_identity",
                        "detect_begin",
                        Some(&worker_folder_key),
                        input_seq,
                        &[("targets", serde_json::Value::from(targets.len()))],
                    );
                }
                let started = std::time::Instant::now();
                match run_detection_at_with_updates(
                    &db_path,
                    targets,
                    items_generation,
                    worker_folder_key.clone(),
                    &worker_cancel,
                    &io_sem,
                    update_tx.as_ref(),
                ) {
                    Ok(Some(result)) => {
                        if crate::perf::is_enabled() {
                            crate::perf::event(
                                "content_identity",
                                "detect_end",
                                Some(&worker_folder_key),
                                input_seq,
                                &[
                                    (
                                        "ms",
                                        serde_json::Value::from(
                                            started.elapsed().as_secs_f64() * 1000.0,
                                        ),
                                    ),
                                    (
                                        "candidates",
                                        serde_json::Value::from(result.candidates.len()),
                                    ),
                                ],
                            );
                        }
                        if !worker_cancel.load(Ordering::Acquire) {
                            let _ = tx.send(result);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => crate::logger::log(format!(
                        "content_identity: detection failed for {worker_folder_key}: {error}"
                    )),
                }
            }) {
            Ok(_) => Some(Self { cancel, rx }),
            Err(error) => {
                crate::logger::log(format!(
                    "content_identity: detector thread spawn failed: {error}"
                ));
                None
            }
        }
    }

    pub(crate) fn try_recv(&self) -> Result<DetectionResult, mpsc::TryRecvError> {
        self.rx.try_recv()
    }

    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn for_test(result: Option<DetectionResult>) -> (Self, Arc<AtomicBool>) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        if let Some(result) = result {
            tx.send(result).unwrap();
        }
        (
            Self {
                cancel: Arc::clone(&cancel),
                rx,
            },
            cancel,
        )
    }
}

impl Drop for ContentIdentityDetectionPending {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Clone, Debug)]
pub(crate) struct BackfillResult {
    pub(crate) ledger_updates: Vec<LedgerEntry>,
    pub(crate) errors: usize,
}

/// A4 の folder-open backfill。現在フォルダの候補だけを Low priority で記録し、
/// folder 切替では token を立てて metadata/hash の各境界で止める。
pub(crate) struct ContentIdentityBackfillPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<BackfillResult>,
    #[cfg(test)]
    requested_sources: usize,
}

impl ContentIdentityBackfillPending {
    pub(crate) fn spawn(
        sources: Vec<ContentIdentitySource>,
        input_seq: u64,
        folder_key: String,
        io_sem: Arc<crate::io_semaphore::GlobalIoSemaphore>,
        update_tx: Option<mpsc::Sender<LedgerEntry>>,
    ) -> Option<Self> {
        let requested_sources = sources.len();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let db_path = crate::data_dir::get().join("content_identity.db");
        match std::thread::Builder::new()
            .name("content-identity-backfill".into())
            .spawn(move || {
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "content_identity",
                        "backfill_begin",
                        Some(&folder_key),
                        input_seq,
                        &[("targets", serde_json::Value::from(requested_sources))],
                    );
                }
                let started = std::time::Instant::now();
                match run_backfill_at(
                    &db_path,
                    sources,
                    &worker_cancel,
                    &io_sem,
                    update_tx.as_ref(),
                ) {
                    Ok(Some(result)) => {
                        if crate::perf::is_enabled() {
                            crate::perf::event(
                                "content_identity",
                                "backfill_end",
                                Some(&folder_key),
                                input_seq,
                                &[
                                    (
                                        "ms",
                                        serde_json::Value::from(
                                            started.elapsed().as_secs_f64() * 1000.0,
                                        ),
                                    ),
                                    (
                                        "recorded",
                                        serde_json::Value::from(result.ledger_updates.len()),
                                    ),
                                    ("errors", serde_json::Value::from(result.errors)),
                                ],
                            );
                        }
                        if !worker_cancel.load(Ordering::Acquire) {
                            let _ = tx.send(result);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => crate::logger::log(format!(
                        "content_identity: backfill failed for {folder_key}: {error}"
                    )),
                }
            }) {
            Ok(_) => Some(Self {
                cancel,
                rx,
                #[cfg(test)]
                requested_sources,
            }),
            Err(error) => {
                crate::logger::log(format!(
                    "content_identity: backfill thread spawn failed: {error}"
                ));
                None
            }
        }
    }

    pub(crate) fn try_recv(&self) -> Result<BackfillResult, mpsc::TryRecvError> {
        self.rx.try_recv()
    }

    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn requested_sources(&self) -> usize {
        self.requested_sources
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> (Self, Arc<AtomicBool>) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (_tx, rx) = mpsc::channel();
        (
            Self {
                cancel: Arc::clone(&cancel),
                rx,
                #[cfg(test)]
                requested_sources: 0,
            },
            cancel,
        )
    }
}

impl Drop for ContentIdentityBackfillPending {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn run_backfill_at(
    db_path: &Path,
    sources: Vec<ContentIdentitySource>,
    cancel: &AtomicBool,
    io_sem: &crate::io_semaphore::GlobalIoSemaphore,
    update_tx: Option<&mpsc::Sender<LedgerEntry>>,
) -> Result<Option<BackfillResult>, String> {
    if cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    let db = {
        let Some(_permit) =
            io_sem.acquire_cancellable(crate::io_semaphore::IoPriority::Low, cancel)
        else {
            return Ok(None);
        };
        ContentIdentityDb::open_at(db_path)?
    };
    let mut ledger_updates = Vec::new();
    let mut errors = 0;
    for source in sources {
        if cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        let Some(_permit) =
            io_sem.acquire_cancellable(crate::io_semaphore::IoPriority::Low, cancel)
        else {
            return Ok(None);
        };
        let request = CoalescedRecordRequest {
            file_key: crate::path_key::normalize_keep_drive(&source.path),
            source,
            trigger: ContentIdentityTrigger::ViewingState,
            recorded_at: 0,
        };
        match record_source(&db, &request, cancel) {
            Ok(Some(entry)) => {
                if let Some(update_tx) = update_tx {
                    let _ = update_tx.send(entry.clone());
                }
                ledger_updates.push(entry);
            }
            Ok(None) => return Ok(None),
            Err(error) => {
                errors += 1;
                crate::logger::log(format!(
                    "content_identity: backfill recording failed for {}: {error}",
                    request.source.path.display()
                ));
            }
        }
    }
    Ok(Some(BackfillResult {
        ledger_updates,
        errors,
    }))
}

struct CancellableReader<'a, R> {
    inner: &'a mut R,
    cancel: &'a AtomicBool,
}

fn match_target_content<R: Read + Seek>(
    reader: &mut R,
    size: u64,
    origins: Vec<LedgerEntry>,
    cancel: &AtomicBool,
) -> Result<Option<(String, String, Vec<LedgerEntry>)>, String> {
    let head_hash = {
        let mut reader = CancellableReader {
            inner: reader,
            cancel,
        };
        match stage1_head_hash(&mut reader, size) {
            Ok(hash) => hash,
            Err(error)
                if error.kind() == io::ErrorKind::Interrupted && cancel.load(Ordering::Acquire) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    if cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    let head_matches = origins
        .into_iter()
        .filter(|origin| origin.head_hash == head_hash && origin.full_hash.is_some())
        .collect::<Vec<_>>();
    if head_matches.is_empty() {
        return Ok(None);
    }

    reader.rewind().map_err(|error| error.to_string())?;
    if cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    let full_hash = {
        let mut reader = CancellableReader {
            inner: reader,
            cancel,
        };
        match stage2_full_hash(&mut reader) {
            Ok(hash) => hash,
            Err(error)
                if error.kind() == io::ErrorKind::Interrupted && cancel.load(Ordering::Acquire) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    if cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    let matching = head_matches
        .into_iter()
        .filter(|origin| origin.full_hash.as_deref() == Some(full_hash.as_str()))
        .collect::<Vec<_>>();
    Ok(Some((head_hash, full_hash, matching)))
}

impl<R: Read> Read for CancellableReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "content identity detection cancelled",
            ));
        }
        self.inner.read(buffer)
    }
}

#[cfg(test)]
fn run_detection_at(
    db_path: &Path,
    targets: Vec<DetectionTarget>,
    items_generation: u64,
    folder_key: String,
    cancel: &AtomicBool,
    io_sem: &crate::io_semaphore::GlobalIoSemaphore,
) -> Result<Option<DetectionResult>, String> {
    run_detection_at_with_updates(
        db_path,
        targets,
        items_generation,
        folder_key,
        cancel,
        io_sem,
        None,
    )
}

fn run_detection_at_with_updates(
    db_path: &Path,
    targets: Vec<DetectionTarget>,
    items_generation: u64,
    folder_key: String,
    cancel: &AtomicBool,
    io_sem: &crate::io_semaphore::GlobalIoSemaphore,
    update_tx: Option<&mpsc::Sender<LedgerEntry>>,
) -> Result<Option<DetectionResult>, String> {
    if cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    let db = {
        let Some(_permit) =
            io_sem.acquire_cancellable(crate::io_semaphore::IoPriority::Low, cancel)
        else {
            return Ok(None);
        };
        ContentIdentityDb::open_at(db_path).map_err(|error| error.to_string())?
    };
    let mut candidates = Vec::new();
    let mut ledger_updates = Vec::new();
    for target in targets {
        if cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        let Some(_permit) =
            io_sem.acquire_cancellable(crate::io_semaphore::IoPriority::Low, cancel)
        else {
            return Ok(None);
        };
        match detect_target(&db, target, cancel) {
            Ok(Some((candidate, update))) => {
                if let Some(update_tx) = update_tx {
                    let _ = update_tx.send(update.clone());
                }
                ledger_updates.push(update);
                if let Some(candidate) = candidate {
                    candidates.push(candidate);
                }
            }
            Ok(None) => {}
            Err(_error) if cancel.load(Ordering::Acquire) => return Ok(None),
            Err(error) => {
                crate::logger::log(format!("content_identity: candidate check failed: {error}"))
            }
        }
    }
    if cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    let data_dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    if !candidates.is_empty() {
        // store を読むのでここも背景 I/O の枠に入れる。取れなければ候補はそのまま出す
        // (余計な確認が 1 回出るだけで、復元できるものは失わない)。
        if let Some(_permit) =
            io_sem.acquire_cancellable(crate::io_semaphore::IoPriority::Low, cancel)
        {
            drop_sources_with_nothing_to_restore(&db, data_dir, &mut candidates);
        }
    }
    Ok(Some(DetectionResult {
        items_generation,
        folder_key,
        candidates,
        ledger_updates,
    }))
}

/// 中身がもう無い復元元を候補から外し、台帳の flag も下ろす。
///
/// 台帳の `has_restorable_content` は「編集した」という**操作**で立つ。編集を取り消した
/// 経路 (マスク削除、trim override 削除、標準値と同じになった補正の破棄) も同じ record を
/// 通るため、実データが 1 つも無い file_key が復元元として残り続ける。利用者からは
/// 「何も編集していないファイルで復元ダイアログが出る」に見える (2026-08-25 報告)。
///
/// 復元が運ぶのは `copy_stores_at` が写す `STORES` の行だけなので、そこに 1 行も無い
/// 復元元は選ばれても no-op にしかならない。**候補から外しても失われるものは無い。**
/// ついでに flag を下ろして、次のフォルダ訪問で同じ probe を繰り返さないようにする。
///
/// 「無い」という観測は store を何本も開く間に古くなる。そのため
/// (1) CAS の期待値は probe より前に読み、(2) clear の後にもう一度 probe して、
/// 間に書かれた行があれば flag を戻し候補も残す。候補を残すかどうかも最初の観測では
/// なく 2 回目の観測で決めるので、clear と retain が同じ事実を見る。
fn drop_sources_with_nothing_to_restore(
    db: &ContentIdentityDb,
    data_dir: &Path,
    candidates: &mut Vec<RestoreCandidate>,
) {
    let outcome = drop_sources_with_nothing_to_restore_with_probe(db, candidates, |file_keys| {
        crate::rename_key_migration::ledger_keys_with_restorable_rows_at(data_dir, file_keys)
    });
    for file_key in &outcome.cleared {
        crate::logger::log(format!(
            "content_identity: dropped empty restore origin {file_key}"
        ));
    }
    for file_key in &outcome.restored {
        crate::logger::log(format!(
            "content_identity: restored origin {file_key} (edited while probing)"
        ));
    }
}

/// [`drop_sources_with_nothing_to_restore`] が実際に台帳へ書いたこと。
///
/// `restored` は「自分が下ろした flag を戻した」= probe と clear の間に編集が入り、
/// CAS が **それを止められなかった** ことを意味する。CAS の期待値を probe より前に
/// 読んでいれば `cleared` にも `restored` にも載らない。ここを返り値にしておかないと、
/// 「止めた」のか「消してから戻した」のかを外から区別できない (レビュー R-05 の
/// 修正を、再 probe による事後修復だけで通してしまわないため)。
#[derive(Debug, Default, PartialEq, Eq)]
struct EmptyOriginCleanup {
    cleared: Vec<String>,
    restored: Vec<String>,
}

/// store probe を差し替えられる本体。競合は「probe の途中で行が書かれる」ことなので、
/// probe を閉包にしないと再現できない (`detect_target_with_opener` と同じ形)。
fn drop_sources_with_nothing_to_restore_with_probe(
    db: &ContentIdentityDb,
    candidates: &mut Vec<RestoreCandidate>,
    mut probe: impl FnMut(&[String]) -> std::collections::BTreeSet<String>,
) -> EmptyOriginCleanup {
    let mut outcome = EmptyOriginCleanup::default();
    if candidates.is_empty() {
        return outcome;
    }
    let file_keys = candidates
        .iter()
        .flat_map(|candidate| candidate.sources.iter())
        .map(|source| source.file_key.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    // CAS の期待値は **probe より前** に読む。probe は store を何本も開くので、その間に
    // UI スレッドが編集を記録し得る。probe の後に読むと、期待値が既にその編集の
    // timestamp になっていて CAS が必ず成立し、守るはずだった編集を自分で消す。
    let expected_last_edit_at = file_keys
        .iter()
        .filter_map(|file_key| match db.ledger_entry(file_key) {
            Ok(Some(entry)) => Some((file_key.clone(), entry.last_edit_at)),
            Ok(None) => None,
            Err(error) => {
                crate::logger::log(format!(
                    "content_identity: could not read ledger for {file_key}: {error}"
                ));
                None
            }
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let backed = probe(&file_keys);
    if backed.len() == file_keys.len() {
        return outcome;
    }
    let unbacked = file_keys
        .iter()
        .filter(|key| !backed.contains(*key))
        .cloned()
        .collect::<Vec<_>>();
    for file_key in &unbacked {
        // 台帳を読めなかった key は触らない。期待値が無いまま消すと CAS の意味が無い。
        let Some(&expected) = expected_last_edit_at.get(file_key) else {
            continue;
        };
        match db.clear_restorable_if_unchanged(file_key, expected) {
            Ok(true) => outcome.cleared.push(file_key.clone()),
            Ok(false) => {}
            Err(error) => crate::logger::log(format!(
                "content_identity: could not clear empty origin {file_key}: {error}"
            )),
        }
    }
    // clear の後にもう一度 store を読む。probe と clear の間に書かれた行はここで見つかる。
    // timestamp ではなく **行の有無** を見るので、同じミリ秒に 2 つの編集が入って CAS が
    // 見分けられなかった場合もここで拾える。候補を残すか消すかも、この後の観測で決める。
    let re_backed = probe(&unbacked);
    // 戻すのは **自分が下ろした** key だけ。CAS が拒んだ key は flag が 1 のままなので
    // 触る必要が無い。
    for file_key in outcome
        .cleared
        .iter()
        .filter(|file_key| re_backed.contains(*file_key))
        .cloned()
        .collect::<Vec<_>>()
    {
        match db.restore_restorable_flag(&file_key) {
            Ok(()) => outcome.restored.push(file_key.clone()),
            Err(error) => crate::logger::log(format!(
                "content_identity: could not restore origin {file_key}: {error}"
            )),
        }
    }
    for candidate in candidates.iter_mut() {
        candidate.sources.retain(|source| {
            backed.contains(&source.file_key) || re_backed.contains(&source.file_key)
        });
    }
    candidates.retain(|candidate| !candidate.sources.is_empty());
    outcome
}

fn detect_target(
    db: &ContentIdentityDb,
    target: DetectionTarget,
    cancel: &AtomicBool,
) -> Result<Option<(Option<RestoreCandidate>, LedgerEntry)>, String> {
    detect_target_with_opener(db, target, cancel, |path| {
        File::open(path).map_err(|error| format!("open {}: {error}", path.display()))
    })
}

fn detect_target_with_opener<R: Read + Seek>(
    db: &ContentIdentityDb,
    target: DetectionTarget,
    cancel: &AtomicBool,
    open: impl FnOnce(&Path) -> Result<R, String>,
) -> Result<Option<(Option<RestoreCandidate>, LedgerEntry)>, String> {
    if cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    let before = std::fs::metadata(&target.source.path).map_err(|error| error.to_string())?;
    if !before.is_file() || before.len() != target.size {
        return Ok(None);
    }
    let hashed_mtime = metadata_mtime(&before)?;
    if cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    let state = RecordedFileState {
        file_key: target.file_key.clone(),
        size: target.size,
        hashed_mtime,
    };
    let cached = db.ledger_entry(&target.file_key)?;
    if cached
        .as_ref()
        .is_some_and(|entry| entry.has_restorable_content)
    {
        return Ok(None);
    }

    let (_head_hash, full_hash, matching, update) = if let Some(cached) = cached.filter(|entry| {
        entry.size == state.size
            && entry.hashed_mtime == state.hashed_mtime
            && entry.full_hash.is_some()
    }) {
        let full_hash = cached
            .full_hash
            .clone()
            .expect("cache predicate checked full_hash");
        let matching = matching_origins(&target.origins, &cached.head_hash, &full_hash);
        (cached.head_hash.clone(), full_hash, matching, cached)
    } else {
        if cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        let mut file = open(&target.source.path)?;
        let Some((head_hash, full_hash, matching)) =
            match_target_content(&mut file, target.size, target.origins, cancel)?
        else {
            return Ok(None);
        };
        let after = std::fs::metadata(&target.source.path).map_err(|error| error.to_string())?;
        if after.len() != target.size || metadata_mtime(&after)? != hashed_mtime {
            return Err(format!(
                "{} changed while hashing",
                target.source.path.display()
            ));
        }
        if cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        record_observation_with_hasher(
            db,
            &target.source,
            &state,
            ContentIdentityTrigger::ViewingState,
            ObservationRole::DetectionCache,
            unix_time_millis(),
            || Ok(Some((head_hash.clone(), full_hash.clone()))),
        )?;
        let update = db
            .ledger_entry(&target.file_key)?
            .ok_or_else(|| "detection cache row was not stored".to_string())?;
        (head_hash, full_hash, matching, update)
    };
    if cancel.load(Ordering::Acquire) {
        return Ok(Some((None, update)));
    }
    if matching.is_empty() {
        return Ok(Some((None, update)));
    }
    let declined = db.restore_was_declined(&full_hash, &target.file_key)?;
    let candidate = if declined {
        None
    } else {
        let mut sources = Vec::with_capacity(matching.len());
        for origin in matching {
            if cancel.load(Ordering::Acquire) {
                return Ok(Some((None, update)));
            }
            let path = PathBuf::from(&origin.file_key);
            let source_exists = path.try_exists().unwrap_or(false);
            sources.push(RestoreSourceCandidate {
                file_key: origin.file_key,
                path,
                kind: origin.kind,
                last_edit_at: origin.last_edit_at,
                source_exists,
            });
        }
        sort_restore_sources(&mut sources);
        Some(RestoreCandidate {
            target_key: target.file_key,
            target_path: target.source.path,
            target_kind: target.source.kind,
            full_hash,
            sources,
        })
    };
    Ok(Some((candidate, update)))
}

fn matching_origins(origins: &[LedgerEntry], head_hash: &str, full_hash: &str) -> Vec<LedgerEntry> {
    origins
        .iter()
        .filter(|origin| {
            origin.head_hash == head_hash && origin.full_hash.as_deref() == Some(full_hash)
        })
        .cloned()
        .collect()
}

pub(crate) fn sort_restore_sources(sources: &mut [RestoreSourceCandidate]) {
    sources.sort_by(|left, right| {
        right
            .last_edit_at
            .cmp(&left.last_edit_at)
            .then_with(|| left.file_key.cmp(&right.file_key))
    });
}

fn run_worker(
    rx: mpsc::Receiver<RecordRequest>,
    update_tx: mpsc::Sender<LedgerEntry>,
    shutdown: Arc<AtomicBool>,
) {
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
            match record_source(&db, request, &shutdown) {
                Ok(Some(entry)) => {
                    let _ = update_tx.send(entry);
                }
                Ok(None) => {}
                Err(error) => {
                    crate::logger::log(format!(
                        "content_identity: recording failed for {}: {error}",
                        request.source.path.display()
                    ));
                }
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
) -> Result<Option<LedgerEntry>, String> {
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
        ObservationRole::RestorableContent,
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
    )?;
    if shutdown.load(Ordering::Acquire) {
        return Ok(None);
    }
    db.ledger_entry(&state.file_key)
}

fn record_observation_with_hasher(
    db: &ContentIdentityDb,
    source: &ContentIdentitySource,
    state: &RecordedFileState,
    trigger: ContentIdentityTrigger,
    role: ObservationRole,
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
        return match (role, trigger) {
            (ObservationRole::RestorableContent, ContentIdentityTrigger::Edit) => {
                db.mark_restorable(&state.file_key, source.kind, Some(last_edit_at))
            }
            (ObservationRole::RestorableContent, ContentIdentityTrigger::ViewingState)
                if !recorded
                    .as_ref()
                    .is_some_and(|recorded| recorded.has_restorable_content) =>
            {
                db.mark_restorable(&state.file_key, source.kind, None)
            }
            _ => Ok(()),
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
    db.upsert(
        source,
        state,
        &head_hash,
        &full_hash,
        stored_last_edit_at,
        role,
    )
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

    /// 「行がもう無い」という観測は store を何本も開く間に古くなる。その間に入った編集を
    /// 自分の clear で消してはいけない。
    ///
    /// 欠陥は CAS の期待値を probe の **後** に読んでいたことで、期待値が既に並行編集の
    /// timestamp になっているため CAS が必ず成立していた。守るはずの編集が消え、候補も
    /// stale な観測で落ちていた (2026-08-29 レビュー R-05)。
    #[test]
    fn an_edit_that_lands_while_probing_keeps_its_restore_origin() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("content_identity.db");
        let db = ContentIdentityDb::open_at(&db_path).unwrap();
        let origin_path = dir.path().join("origin.png");
        let file_key = crate::path_key::normalize_keep_drive(&origin_path);
        db.upsert(
            &ContentIdentitySource::new(&origin_path, ContentKind::Image),
            &RecordedFileState {
                file_key: file_key.clone(),
                size: 10,
                hashed_mtime: 1,
            },
            "head",
            "full",
            100,
            ObservationRole::RestorableContent,
        )
        .unwrap();

        let mut candidates = vec![RestoreCandidate {
            target_key: "target".to_string(),
            target_path: dir.path().join("target.png"),
            target_kind: ContentKind::Image,
            full_hash: "full".to_string(),
            sources: vec![RestoreSourceCandidate {
                file_key: file_key.clone(),
                path: origin_path.clone(),
                kind: ContentKind::Image,
                last_edit_at: 100,
                source_exists: true,
            }],
        }];

        // 1 回目の probe の最中に編集が入る。probe 自身は古い観測 (= 行なし) を返す。
        let mut probe_calls = 0;
        let outcome =
            drop_sources_with_nothing_to_restore_with_probe(&db, &mut candidates, |keys| {
                probe_calls += 1;
                if probe_calls == 1 {
                    db.mark_restorable(&file_key, ContentKind::Image, Some(200))
                        .unwrap();
                    std::collections::BTreeSet::new()
                } else {
                    keys.iter().cloned().collect()
                }
            });

        assert_eq!(probe_calls, 2, "clear の後に観測し直していない");
        // 再 probe による事後修復ではなく、CAS が止めたことを確かめる。消してから戻すと
        // その間 flag が 0 で、そこで落ちれば復元の申し出が永久に消える。
        assert_eq!(
            outcome,
            EmptyOriginCleanup::default(),
            "並行編集を CAS で止めず、一度消してから戻している"
        );
        let entry = db.ledger_entry(&file_key).unwrap().unwrap();
        assert!(
            entry.has_restorable_content,
            "probe 中に入った編集の復元元 flag を消した"
        );
        assert_eq!(entry.last_edit_at, 200, "編集側の timestamp を書き換えた");
        assert_eq!(
            candidates.len(),
            1,
            "行が見つかったのに候補から落とした: {candidates:?}"
        );
    }

    /// 本当に何も残っていない復元元は、これまでどおり flag を下ろして候補から外す。
    #[test]
    fn a_restore_origin_with_nothing_behind_it_is_still_dropped() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("content_identity.db");
        let db = ContentIdentityDb::open_at(&db_path).unwrap();
        let origin_path = dir.path().join("origin.png");
        let file_key = crate::path_key::normalize_keep_drive(&origin_path);
        db.upsert(
            &ContentIdentitySource::new(&origin_path, ContentKind::Image),
            &RecordedFileState {
                file_key: file_key.clone(),
                size: 10,
                hashed_mtime: 1,
            },
            "head",
            "full",
            100,
            ObservationRole::RestorableContent,
        )
        .unwrap();

        let mut candidates = vec![RestoreCandidate {
            target_key: "target".to_string(),
            target_path: dir.path().join("target.png"),
            target_kind: ContentKind::Image,
            full_hash: "full".to_string(),
            sources: vec![RestoreSourceCandidate {
                file_key: file_key.clone(),
                path: origin_path.clone(),
                kind: ContentKind::Image,
                last_edit_at: 100,
                source_exists: true,
            }],
        }];

        drop_sources_with_nothing_to_restore_with_probe(&db, &mut candidates, |_| {
            std::collections::BTreeSet::new()
        });

        let entry = db.ledger_entry(&file_key).unwrap().unwrap();
        assert!(
            !entry.has_restorable_content,
            "実データが無い復元元の flag が残っている"
        );
        assert!(
            candidates.is_empty(),
            "実データが無い復元元が候補に残っている: {candidates:?}"
        );
    }
    use super::*;
    use std::io::{Cursor, SeekFrom};

    fn test_ledger_entry(
        file_key: impl Into<String>,
        size: u64,
        head_hash: impl Into<String>,
        full_hash: impl Into<String>,
        last_edit_at: i64,
    ) -> LedgerEntry {
        LedgerEntry {
            file_key: file_key.into(),
            size,
            head_hash: head_hash.into(),
            full_hash: Some(full_hash.into()),
            hashed_mtime: 1,
            kind: ContentKind::Image,
            last_edit_at,
            has_restorable_content: true,
        }
    }

    fn poll_test_index_reload(app: &mut crate::app::AppTestEnvForTest) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !app.content_identity_ledger_state.is_ready() {
            app.poll_content_identity_detection(&egui::Context::default());
            assert!(
                std::time::Instant::now() < deadline,
                "content identity index reload did not complete"
            );
            std::thread::yield_now();
        }
    }

    /// 復元元として成立する最小の実データを置く。
    ///
    /// 台帳の行だけでは復元元にならない。復元が運ぶのは `STORES` の行なので、
    /// 1 つも無い file_key は候補から外れる (2026-08-25 の「取り消した編集が復元元として
    /// 残る」対応)。本番の backfill は編集済みファイルにしか走らないので、fixture 側も
    /// 実データを持たせて production と同じ形にする。
    fn give_test_file_restorable_content(data_dir: &Path, path: &Path) {
        let db = crate::rating_db::RatingDb::open_at(data_dir.join("rating.db")).unwrap();
        db.set(&crate::adjustment_db::normalize_path(path), 3)
            .unwrap();
    }

    fn detect_test_image_copy(
        db_path: &Path,
        index: &ContentIdentityIndex,
        path: &Path,
    ) -> Vec<RestoreCandidate> {
        let size = std::fs::metadata(path).unwrap().len();
        let target = stage0_target(
            index,
            ContentIdentitySource::new(path, ContentKind::Image),
            size,
        )
        .expect("byte-identical test copy must reach content identity detection");
        run_detection_at(
            db_path,
            vec![target],
            1,
            crate::path_key::normalize_keep_drive(path.parent().unwrap()),
            &AtomicBool::new(false),
            &crate::io_semaphore::GlobalIoSemaphore::new(1),
        )
        .unwrap()
        .unwrap()
        .candidates
    }

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
    fn stage0_size_mismatch_performs_zero_io_calls() {
        let index = ContentIdentityIndex::from_entries(vec![test_ledger_entry(
            "c:/origin.png",
            10,
            "head",
            "full",
            1,
        )]);
        let io_calls = std::cell::Cell::new(0);
        let target = stage0_target(
            &index,
            ContentIdentitySource::new("c:/target.png", ContentKind::Image),
            11,
        );
        if target.is_some() {
            io_calls.set(io_calls.get() + 1);
        }

        assert!(target.is_none());
        assert_eq!(io_calls.get(), 0, "段 0 不一致では worker I/O へ渡さない");
    }

    #[test]
    fn stage0_excludes_the_same_file_key() {
        let index = ContentIdentityIndex::from_entries(vec![test_ledger_entry(
            "c:/images/same.png",
            10,
            "head",
            "full",
            1,
        )]);

        assert!(
            stage0_target(
                &index,
                ContentIdentitySource::new("C:/images/same.png", ContentKind::Image),
                10,
            )
            .is_none()
        );
    }

    #[test]
    fn index_reload_is_background_low_priority_and_cancellable() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("content_identity.db");
        drop(ContentIdentityDb::open_at(&db_path).unwrap());
        let io_sem = Arc::new(crate::io_semaphore::GlobalIoSemaphore::new(1));
        let holder = io_sem.acquire(crate::io_semaphore::IoPriority::Normal);

        let pending = ContentIdentityIndexLoadPending::spawn_at(db_path, Arc::clone(&io_sem))
            .expect("spawn only; DB load must not run on the caller thread");
        assert!(matches!(pending.try_recv(), Err(mpsc::TryRecvError::Empty)));

        pending.cancel();
        drop(holder);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match pending.try_recv() {
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::yield_now();
                }
                Ok(result) => panic!("canceled index load published a result: {result:?}"),
            }
        }
        assert_eq!(
            io_sem.stats().0,
            1,
            "canceled Low wait must not leak a permit"
        );
    }

    #[test]
    fn purge_reload_removes_restore_source_exclusion_and_copy_is_candidate_again() {
        let mut app = crate::app::setup_app_for_test();
        if let Some(pending) = app.content_identity_index_load_pending.take() {
            pending.cancel();
        }
        app.settings.edit_restore_prompt_enabled = true;
        let db_path = app.tmp.path().join("content_identity.db");
        let origin_path = app.tmp.path().join("origin.png");
        let copied_path = app.tmp.path().join("copied.png");
        let bytes = vec![23_u8; HEAD_HASH_BYTES as usize + 9];
        std::fs::write(&origin_path, &bytes).unwrap();
        std::fs::write(&copied_path, &bytes).unwrap();
        let size = bytes.len() as u64;
        let head_hash = stage1_head_hash(&mut Cursor::new(&bytes), size).unwrap();
        let full_hash = stage2_full_hash(&mut Cursor::new(&bytes)).unwrap();
        let origin_key = crate::path_key::normalize_keep_drive(&origin_path);
        let copied_key = crate::path_key::normalize_keep_drive(&copied_path);
        let db = ContentIdentityDb::open_at(&db_path).unwrap();
        for (path, key) in [(&origin_path, &origin_key), (&copied_path, &copied_key)] {
            db.upsert(
                &ContentIdentitySource::new(path, ContentKind::Image),
                &RecordedFileState {
                    file_key: key.clone(),
                    size,
                    hashed_mtime: 1,
                },
                &head_hash,
                &full_hash,
                10,
                ObservationRole::RestorableContent,
            )
            .unwrap();
        }
        app.content_identity_index = db.load_index(&AtomicBool::new(false)).unwrap().unwrap();
        app.content_identity_ledger_state = ContentIdentityLedgerState::Ready;
        assert!(app.content_identity_index.contains_file_key(&copied_key));
        drop(db);

        let purge = crate::rename_key_migration::purge_removed_paths_at(
            app.tmp.path(),
            std::slice::from_ref(&copied_path),
            &[],
        );
        assert!(purge.errors.is_empty(), "{:?}", purge.errors);
        assert!(purge.store_mutations.content_identity_index_stale());
        app.apply_content_identity_store_mutations(purge.store_mutations);
        poll_test_index_reload(&mut app);

        assert!(!app.content_identity_index.contains_file_key(&copied_key));
        assert!(
            stage0_target(
                &app.content_identity_index,
                ContentIdentitySource::new(&copied_path, ContentKind::Image),
                size,
            )
            .is_some(),
            "a copy recreated at the purged path must be eligible again"
        );
    }

    #[test]
    fn rename_reload_replaces_old_restore_source_key_with_new_key() {
        let mut app = crate::app::setup_app_for_test();
        if let Some(pending) = app.content_identity_index_load_pending.take() {
            pending.cancel();
        }
        app.settings.edit_restore_prompt_enabled = true;
        let old_path = app.tmp.path().join("old.png");
        let new_path = app.tmp.path().join("new.png");
        let old_key = crate::path_key::normalize_keep_drive(&old_path);
        let new_key = crate::path_key::normalize_keep_drive(&new_path);
        let db = ContentIdentityDb::open_at(&app.tmp.path().join("content_identity.db")).unwrap();
        db.upsert(
            &ContentIdentitySource::new(&old_path, ContentKind::Image),
            &RecordedFileState {
                file_key: old_key.clone(),
                size: 42,
                hashed_mtime: 1,
            },
            "head",
            "full",
            10,
            ObservationRole::RestorableContent,
        )
        .unwrap();
        app.content_identity_index = db.load_index(&AtomicBool::new(false)).unwrap().unwrap();
        app.content_identity_ledger_state = ContentIdentityLedgerState::Ready;
        drop(db);

        let report = crate::rename_key_migration::run_at(app.tmp.path(), &old_path, &new_path);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.store_mutations.content_identity_index_stale());
        app.apply_content_identity_store_mutations(report.store_mutations);
        poll_test_index_reload(&mut app);

        assert!(!app.content_identity_index.contains_file_key(&old_key));
        assert!(app.content_identity_index.contains_file_key(&new_key));
        assert!(
            app.content_identity_index
                .contains_ledger_file_key(&new_key)
        );
        assert!(
            !app.content_identity_index
                .contains_ledger_file_key(&old_key)
        );
    }

    #[test]
    fn stale_reload_cancels_folder_workers_and_drops_pre_mutation_updates() {
        let mut app = crate::app::setup_app_for_test();
        if let Some(pending) = app.content_identity_index_load_pending.take() {
            pending.cancel();
        }
        app.settings.edit_restore_prompt_enabled = true;
        app.content_identity_ledger_state = ContentIdentityLedgerState::Ready;
        let (detection, detection_cancel) = ContentIdentityDetectionPending::for_test(None);
        app.content_identity_detection_pending = Some(detection);
        let (backfill, backfill_cancel) = ContentIdentityBackfillPending::for_test();
        app.content_identity_backfill_pending = Some(backfill);
        app.content_identity_updates_before_load
            .push(test_ledger_entry(
                "c:/deleted-before-reload.png",
                1,
                "head",
                "full",
                1,
            ));
        let descriptor = crate::rename_key_migration::STORES
            .iter()
            .find(|descriptor| descriptor.table == "edit_origin")
            .unwrap();
        let mut effects = crate::rename_key_migration::StoreMutationEffects::default();
        effects.record_completed(descriptor);

        app.apply_content_identity_store_mutations(effects);

        assert!(detection_cancel.load(Ordering::Acquire));
        assert!(backfill_cancel.load(Ordering::Acquire));
        assert!(app.content_identity_detection_pending.is_none());
        assert!(app.content_identity_backfill_pending.is_none());
        assert!(app.content_identity_updates_before_load.is_empty());
        assert!(matches!(
            app.content_identity_ledger_state,
            ContentIdentityLedgerState::Loading
        ));
        assert!(app.content_identity_index_load_pending.is_some());
    }

    #[test]
    fn late_detection_cache_update_does_not_remove_a_restorable_index_entry() {
        let restorable = test_ledger_entry("c:/images/same.png", 10, "head", "full", 1);
        let mut index = ContentIdentityIndex::from_entries(vec![restorable.clone()]);
        let mut stale_cache_update = restorable;
        stale_cache_update.has_restorable_content = false;
        stale_cache_update.size = 20;

        index.upsert(stale_cache_update);

        assert!(index.contains_file_key("c:/images/same.png"));
        assert_eq!(index.entries_for_size(10).len(), 1);
        assert!(index.entries_for_size(20).is_empty());
    }

    #[test]
    fn stage1_mismatch_does_not_seek_or_read_stage2() {
        struct CountingCursor {
            inner: Cursor<Vec<u8>>,
            reads: usize,
            bytes: usize,
            seeks: usize,
        }

        impl Read for CountingCursor {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                self.reads += 1;
                let count = self.inner.read(buffer)?;
                self.bytes += count;
                Ok(count)
            }
        }

        impl Seek for CountingCursor {
            fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
                self.seeks += 1;
                self.inner.seek(position)
            }
        }

        let data = vec![7_u8; HEAD_HASH_BYTES as usize + 123];
        let size = data.len() as u64;
        let mut reader = CountingCursor {
            inner: Cursor::new(data),
            reads: 0,
            bytes: 0,
            seeks: 0,
        };
        let result = match_target_content(
            &mut reader,
            size,
            vec![test_ledger_entry(
                "c:/origin.png",
                size,
                "different-head",
                "unused-full",
                1,
            )],
            &AtomicBool::new(false),
        )
        .unwrap();

        assert!(result.is_none());
        assert_eq!(reader.seeks, 0, "段 2 のための rewind をしていない");
        assert_eq!(reader.bytes, HEAD_HASH_BYTES as usize);
    }

    #[test]
    fn detection_hashing_observes_cancellation_between_reads() {
        struct CancelOnSecondRead<'a> {
            inner: Cursor<Vec<u8>>,
            cancel: &'a AtomicBool,
            reads: usize,
        }

        impl Read for CancelOnSecondRead<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                self.reads += 1;
                let count = self.inner.read(buffer)?;
                if self.reads == 2 {
                    self.cancel.store(true, Ordering::Release);
                }
                Ok(count)
            }
        }

        impl Seek for CancelOnSecondRead<'_> {
            fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
                self.inner.seek(position)
            }
        }

        let data = vec![9_u8; HASH_CHUNK_BYTES + 17];
        let size = data.len() as u64;
        let head_hash = stage1_head_hash(&mut Cursor::new(&data), size).unwrap();
        let full_hash = stage2_full_hash(&mut Cursor::new(&data)).unwrap();
        let cancel = AtomicBool::new(false);
        let mut reader = CancelOnSecondRead {
            inner: Cursor::new(data),
            cancel: &cancel,
            reads: 0,
        };

        let result = match_target_content(
            &mut reader,
            size,
            vec![test_ledger_entry(
                "c:/origin.png",
                size,
                head_hash,
                full_hash,
                1,
            )],
            &cancel,
        )
        .unwrap();

        assert!(result.is_none());
        assert_eq!(reader.reads, 2, "cancel 後は次の read を開始しない");
    }

    #[test]
    fn restore_declined_is_filtered_after_full_hash_and_detection_cache_uses_zero_edit_time() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("content_identity.db");
        let target_path = temp.path().join("target.png");
        let bytes = vec![3_u8; HEAD_HASH_BYTES as usize + 7];
        std::fs::write(&target_path, &bytes).unwrap();
        let size = bytes.len() as u64;
        let head_hash = stage1_head_hash(&mut Cursor::new(&bytes), size).unwrap();
        let full_hash = stage2_full_hash(&mut Cursor::new(&bytes)).unwrap();
        let target_key = crate::path_key::normalize_keep_drive(&target_path);
        let origin_key = crate::path_key::normalize_keep_drive(&temp.path().join("origin.png"));
        let origin = test_ledger_entry(&origin_key, size, &head_hash, &full_hash, 123);

        {
            let db = ContentIdentityDb::open_at(&db_path).unwrap();
            db.upsert(
                &ContentIdentitySource::new(&origin_key, ContentKind::Image),
                &RecordedFileState {
                    file_key: origin_key.clone(),
                    size,
                    hashed_mtime: 1,
                },
                &head_hash,
                &full_hash,
                123,
                ObservationRole::RestorableContent,
            )
            .unwrap();
            db.conn
                .execute(
                    "INSERT INTO restore_declined(full_hash, target_key) VALUES (?1, ?2)",
                    rusqlite::params![full_hash, target_key],
                )
                .unwrap();
        }

        let result = run_detection_at(
            &db_path,
            vec![DetectionTarget {
                source: ContentIdentitySource::new(&target_path, ContentKind::Image),
                file_key: target_key.clone(),
                size,
                origins: vec![origin],
            }],
            7,
            crate::path_key::normalize_keep_drive(temp.path()),
            &AtomicBool::new(false),
            &crate::io_semaphore::GlobalIoSemaphore::new(1),
        )
        .unwrap()
        .unwrap();

        assert!(result.candidates.is_empty(), "辞退済み候補は UI へ返さない");
        assert_eq!(result.ledger_updates.len(), 1, "段 2 完了結果は cache する");
        assert_eq!(result.ledger_updates[0].last_edit_at, 0);
        assert!(!result.ledger_updates[0].has_restorable_content);
        let db = ContentIdentityDb::open_at(&db_path).unwrap();
        let cached = db.ledger_entry(&target_key).unwrap().unwrap();
        assert_eq!(cached.last_edit_at, 0);
        assert!(!cached.has_restorable_content);
    }

    #[test]
    fn stage2_mismatch_is_cached_without_creating_a_candidate() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("content_identity.db");
        let target_path = temp.path().join("target.png");
        let mut origin_bytes = vec![5_u8; HEAD_HASH_BYTES as usize + 1];
        let mut target_bytes = origin_bytes.clone();
        *origin_bytes.last_mut().unwrap() = 6;
        *target_bytes.last_mut().unwrap() = 7;
        std::fs::write(&target_path, &target_bytes).unwrap();
        let size = target_bytes.len() as u64;
        let head_hash = stage1_head_hash(&mut Cursor::new(&origin_bytes), size).unwrap();
        let origin_full_hash = stage2_full_hash(&mut Cursor::new(&origin_bytes)).unwrap();
        let target_key = crate::path_key::normalize_keep_drive(&target_path);

        let result = run_detection_at(
            &db_path,
            vec![DetectionTarget {
                source: ContentIdentitySource::new(&target_path, ContentKind::Image),
                file_key: target_key.clone(),
                size,
                origins: vec![test_ledger_entry(
                    "c:/origin.png",
                    size,
                    head_hash,
                    origin_full_hash,
                    123,
                )],
            }],
            8,
            crate::path_key::normalize_keep_drive(temp.path()),
            &AtomicBool::new(false),
            &crate::io_semaphore::GlobalIoSemaphore::new(1),
        )
        .unwrap()
        .unwrap();

        assert!(result.candidates.is_empty());
        assert_eq!(
            result.ledger_updates.len(),
            1,
            "段 2 の結果を再訪用に cache"
        );
        assert_eq!(result.ledger_updates[0].last_edit_at, 0);
        assert!(!result.ledger_updates[0].has_restorable_content);
        let db = ContentIdentityDb::open_at(&db_path).unwrap();
        assert!(db.ledger_entry(&target_key).unwrap().is_some());
    }

    #[test]
    fn detection_cache_does_not_suppress_second_visit_and_reuses_hashes_without_reading() {
        struct CountingReader<'a> {
            inner: Cursor<Vec<u8>>,
            reads: &'a std::cell::Cell<usize>,
        }

        impl Read for CountingReader<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                self.reads.set(self.reads.get() + 1);
                self.inner.read(buffer)
            }
        }

        impl Seek for CountingReader<'_> {
            fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
                self.inner.seek(position)
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let target_path = temp.path().join("target.png");
        let bytes = vec![11_u8; HEAD_HASH_BYTES as usize + 17];
        std::fs::write(&target_path, &bytes).unwrap();
        let size = bytes.len() as u64;
        let head_hash = stage1_head_hash(&mut Cursor::new(&bytes), size).unwrap();
        let full_hash = stage2_full_hash(&mut Cursor::new(&bytes)).unwrap();
        let origin_key = crate::path_key::normalize_keep_drive(&temp.path().join("origin.png"));
        let target_key = crate::path_key::normalize_keep_drive(&target_path);
        let origin = test_ledger_entry(&origin_key, size, &head_hash, &full_hash, 123);
        let db = ContentIdentityDb::open_at(&temp.path().join("content_identity.db")).unwrap();
        db.upsert(
            &ContentIdentitySource::new(PathBuf::from(&origin_key), ContentKind::Image),
            &RecordedFileState {
                file_key: origin_key.clone(),
                size,
                hashed_mtime: 1,
            },
            &head_hash,
            &full_hash,
            123,
            ObservationRole::RestorableContent,
        )
        .unwrap();

        let target = DetectionTarget {
            source: ContentIdentitySource::new(&target_path, ContentKind::Image),
            file_key: target_key.clone(),
            size,
            origins: vec![origin.clone()],
        };
        let reads = std::cell::Cell::new(0);
        let opens = std::cell::Cell::new(0);
        let (first_candidate, cache_update) =
            detect_target_with_opener(&db, target, &AtomicBool::new(false), |_| {
                opens.set(opens.get() + 1);
                Ok(CountingReader {
                    inner: Cursor::new(bytes.clone()),
                    reads: &reads,
                })
            })
            .unwrap()
            .unwrap();
        assert!(first_candidate.is_some());
        assert!(reads.get() > 0);
        assert_eq!(opens.get(), 1);
        assert!(!cache_update.has_restorable_content);
        assert_eq!(cache_update.last_edit_at, 0);

        let mut same_session_index = ContentIdentityIndex::from_entries(vec![origin]);
        same_session_index.upsert(cache_update);
        let second_target = stage0_target(
            &same_session_index,
            ContentIdentitySource::new(&target_path, ContentKind::Image),
            size,
        )
        .expect("detection cache must not suppress the target in the same session");
        let reads_after_first = reads.get();
        let (second_candidate, second_update) =
            detect_target_with_opener(&db, second_target, &AtomicBool::new(false), |_| {
                opens.set(opens.get() + 1);
                Ok(CountingReader {
                    inner: Cursor::new(bytes.clone()),
                    reads: &reads,
                })
            })
            .unwrap()
            .unwrap();
        assert!(second_candidate.is_some());
        assert_eq!(opens.get(), 1, "一致 cache があればファイルを開かない");
        assert_eq!(
            reads.get(),
            reads_after_first,
            "2 回目の検出はファイルを 1 byte も読まない"
        );
        assert!(!second_update.has_restorable_content);

        let restarted_index = db.load_index(&AtomicBool::new(false)).unwrap().unwrap();
        assert!(restarted_index.contains_file_key(&origin_key));
        assert!(!restarted_index.contains_file_key(&target_key));
        assert!(
            restarted_index.contains_ledger_file_key(&target_key),
            "backfill の ledger 有無判定には detection cache 行も含める"
        );
        assert!(
            stage0_target(
                &restarted_index,
                ContentIdentitySource::new(&target_path, ContentKind::Image),
                size,
            )
            .is_some(),
            "再起動後の index にも detection cache 行を載せない"
        );
    }

    #[test]
    fn a1_observations_promote_detection_cache_without_unwanted_rehash_or_timestamp() {
        let temp = tempfile::tempdir().unwrap();
        let db = ContentIdentityDb::open_at(&temp.path().join("content_identity.db")).unwrap();
        let source = ContentIdentitySource::new("C:/books/book.cbz", ContentKind::Zip);
        let state = RecordedFileState {
            file_key: "c:/books/book.cbz".to_string(),
            size: 42,
            hashed_mtime: 100,
        };
        db.upsert(
            &source,
            &state,
            "head",
            "full",
            0,
            ObservationRole::DetectionCache,
        )
        .unwrap();
        assert!(
            !db.ledger_entry(&state.file_key)
                .unwrap()
                .unwrap()
                .has_restorable_content
        );

        record_observation_with_hasher(
            &db,
            &source,
            &state,
            ContentIdentityTrigger::ViewingState,
            ObservationRole::RestorableContent,
            999,
            || panic!("同一 size/mtime の A1 昇格では再 hash しない"),
        )
        .unwrap();

        let promoted = db.ledger_entry(&state.file_key).unwrap().unwrap();
        assert!(promoted.has_restorable_content);
        assert_eq!(
            promoted.last_edit_at, 0,
            "本の続きは復元元だが編集時刻を進めない"
        );
        assert!(
            db.load_index(&AtomicBool::new(false))
                .unwrap()
                .unwrap()
                .contains_file_key(&state.file_key)
        );

        let edit_source = ContentIdentitySource::new("C:/images/edited.png", ContentKind::Image);
        let edit_state = RecordedFileState {
            file_key: "c:/images/edited.png".to_string(),
            ..state
        };
        db.upsert(
            &edit_source,
            &edit_state,
            "head",
            "full",
            0,
            ObservationRole::DetectionCache,
        )
        .unwrap();
        record_observation_with_hasher(
            &db,
            &edit_source,
            &edit_state,
            ContentIdentityTrigger::Edit,
            ObservationRole::RestorableContent,
            777,
            || panic!("同一 size/mtime の A1 昇格では再 hash しない"),
        )
        .unwrap();
        let promoted_edit = db.ledger_entry(&edit_state.file_key).unwrap().unwrap();
        assert!(promoted_edit.has_restorable_content);
        assert_eq!(promoted_edit.last_edit_at, 777);
    }

    #[test]
    fn detection_cache_update_never_downgrades_a_restorable_row() {
        let temp = tempfile::tempdir().unwrap();
        let db = ContentIdentityDb::open_at(&temp.path().join("content_identity.db")).unwrap();
        let source = ContentIdentitySource::new("C:/images/a.png", ContentKind::Image);
        let old_state = RecordedFileState {
            file_key: "c:/images/a.png".to_string(),
            size: 42,
            hashed_mtime: 100,
        };
        db.upsert(
            &source,
            &old_state,
            "head-old",
            "full-old",
            123,
            ObservationRole::RestorableContent,
        )
        .unwrap();
        let changed_state = RecordedFileState {
            hashed_mtime: 101,
            ..old_state
        };
        record_observation_with_hasher(
            &db,
            &source,
            &changed_state,
            ContentIdentityTrigger::ViewingState,
            ObservationRole::DetectionCache,
            999,
            || Ok(Some(("head-new".to_string(), "full-new".to_string()))),
        )
        .unwrap();

        let entry = db.ledger_entry(&changed_state.file_key).unwrap().unwrap();
        assert!(entry.has_restorable_content);
        assert_eq!(entry.last_edit_at, 123);
        assert_eq!(entry.full_hash.as_deref(), Some("full-new"));
    }

    fn restore_source(file_key: &str, last_edit_at: i64) -> RestoreSourceCandidate {
        RestoreSourceCandidate {
            file_key: file_key.to_string(),
            path: PathBuf::from(file_key),
            kind: ContentKind::Image,
            last_edit_at,
            source_exists: true,
        }
    }

    #[test]
    fn restore_sources_sort_by_last_edit_desc_then_file_key_asc() {
        let mut sources = vec![
            restore_source("c:/zero.png", 0),
            restore_source("c:/same-z.png", 20),
            restore_source("c:/old.png", 10),
            restore_source("c:/same-a.png", 20),
            restore_source("c:/new.png", 30),
        ];

        sort_restore_sources(&mut sources);

        assert_eq!(
            sources
                .iter()
                .map(|source| source.file_key.as_str())
                .collect::<Vec<_>>(),
            vec![
                "c:/new.png",
                "c:/same-a.png",
                "c:/same-z.png",
                "c:/old.png",
                "c:/zero.png",
            ]
        );
    }

    #[test]
    fn all_zero_restore_sources_still_sort_stably_by_file_key() {
        let mut sources = vec![
            restore_source("c:/z.png", 0),
            restore_source("c:/a.png", 0),
            restore_source("c:/m.png", 0),
        ];

        sort_restore_sources(&mut sources);

        assert_eq!(
            sources
                .iter()
                .map(|source| source.file_key.as_str())
                .collect::<Vec<_>>(),
            vec!["c:/a.png", "c:/m.png", "c:/z.png"]
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
        db.upsert(
            &source,
            &original,
            "head-0",
            "full-0",
            1,
            ObservationRole::RestorableContent,
        )
        .unwrap();

        let calls = std::cell::Cell::new(0);
        record_observation_with_hasher(
            &db,
            &source,
            &original,
            ContentIdentityTrigger::Edit,
            ObservationRole::RestorableContent,
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
            ObservationRole::RestorableContent,
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
            ObservationRole::RestorableContent,
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
        db.upsert(
            &source,
            &state,
            "head",
            "full",
            10,
            ObservationRole::RestorableContent,
        )
        .unwrap();

        record_observation_with_hasher(
            &db,
            &source,
            &state,
            ContentIdentityTrigger::ViewingState,
            ObservationRole::RestorableContent,
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
            ObservationRole::RestorableContent,
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
        db.upsert(
            &source,
            &state,
            "head",
            "full",
            10,
            ObservationRole::RestorableContent,
        )
        .unwrap();
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
            ObservationRole::RestorableContent,
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
    fn opens_a1_database_and_upgrades_existing_rows_to_restorable() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("content_identity.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE edit_origin (
                     file_key TEXT PRIMARY KEY,
                     size INTEGER NOT NULL,
                     head_hash TEXT NOT NULL,
                     full_hash TEXT,
                     hashed_mtime INTEGER NOT NULL,
                     kind TEXT NOT NULL,
                     last_edit_at INTEGER NOT NULL
                 );
                 CREATE INDEX edit_origin_full ON edit_origin(full_hash);
                 CREATE TABLE restore_declined (
                     full_hash TEXT NOT NULL,
                     target_key TEXT NOT NULL,
                     PRIMARY KEY(full_hash, target_key)
                 );
                 INSERT INTO edit_origin
                     (file_key, size, head_hash, full_hash, hashed_mtime, kind, last_edit_at)
                 VALUES ('c:/legacy.png', 42, 'head', 'full', 100, 'image', 123);",
            )
            .unwrap();
        drop(connection);

        let db = ContentIdentityDb::open_at(&path).unwrap();
        let version: i64 = db
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, CONTENT_IDENTITY_SCHEMA_VERSION);
        let migrated: i64 = db
            .conn
            .query_row(
                "SELECT has_restorable_content FROM edit_origin WHERE file_key = 'c:/legacy.png'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(migrated, 1, "A1 の全行は実際の復元元だった");
        assert!(
            table_columns(&db.conn, "edit_origin")
                .unwrap()
                .contains("has_restorable_content")
        );
        drop(db);

        let reopened = ContentIdentityDb::open_at(&path).unwrap();
        let rows: i64 = reopened
            .conn
            .query_row("SELECT COUNT(*) FROM edit_origin", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1, "current schema open は冪等で行を壊さない");
    }

    #[test]
    fn unsupported_schema_version_is_an_observable_open_failure() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("content_identity.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "user_version", CONTENT_IDENTITY_SCHEMA_VERSION + 1)
            .unwrap();
        drop(connection);

        let error = match ContentIdentityDb::open_at(&path) {
            Ok(_) => panic!("future schema must not become an empty usable ledger"),
            Err(error) => error,
        };
        assert!(error.contains("unsupported content identity schema version"));
    }

    #[test]
    fn folder_backfill_then_copy_detection_produces_restore_candidate_without_gui() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("content_identity.db");
        let source_folder = temp.path().join("source");
        let target_folder = temp.path().join("target");
        std::fs::create_dir_all(&source_folder).unwrap();
        std::fs::create_dir_all(&target_folder).unwrap();
        let source_path = source_folder.join("edited.png");
        let target_path = target_folder.join("copied.png");
        let bytes = vec![19_u8; HEAD_HASH_BYTES as usize + 31];
        std::fs::write(&source_path, &bytes).unwrap();
        give_test_file_restorable_content(temp.path(), &source_path);
        let cancel = AtomicBool::new(false);
        let io_sem = crate::io_semaphore::GlobalIoSemaphore::new(1);

        let backfill = run_backfill_at(
            &db_path,
            vec![ContentIdentitySource::new(&source_path, ContentKind::Image)],
            &cancel,
            &io_sem,
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(backfill.ledger_updates.len(), 1);
        assert!(backfill.ledger_updates[0].has_restorable_content);
        assert_eq!(
            backfill.ledger_updates[0].last_edit_at, 0,
            "既存編集の実時刻は不明なので ViewingState の 0 を維持する"
        );

        std::fs::copy(&source_path, &target_path).unwrap();
        let db = ContentIdentityDb::open_at(&db_path).unwrap();
        let index = db.load_index(&cancel).unwrap().unwrap();
        let target = stage0_target(
            &index,
            ContentIdentitySource::new(&target_path, ContentKind::Image),
            bytes.len() as u64,
        )
        .expect("backfilled source size must enqueue the copied target");
        drop(db);

        let detected = run_detection_at(
            &db_path,
            vec![target],
            7,
            crate::path_key::normalize_keep_drive(&target_folder),
            &cancel,
            &io_sem,
        )
        .unwrap()
        .unwrap();
        assert_eq!(detected.candidates.len(), 1);
        assert_eq!(
            detected.candidates[0].sources[0].file_key,
            crate::path_key::normalize_keep_drive(&source_path)
        );
    }

    /// 編集を入れてすぐ取り消したファイルは復元元にならない。
    ///
    /// 台帳の flag は「編集操作をした」で立ち、取り消し経路 (マスク削除、trim override
    /// 削除、標準値と同じで破棄された補正) も同じ record を通る。その結果、実データが
    /// 1 つも無い file_key が復元元として残り、中身が同じコピーを開くたびに
    /// 「何も編集していないのに復元ダイアログが出る」ことになる (2026-08-25 報告)。
    #[test]
    fn an_origin_whose_edits_were_all_removed_stops_being_a_restore_source() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("content_identity.db");
        let source_folder = temp.path().join("source");
        let target_folder = temp.path().join("target");
        std::fs::create_dir_all(&source_folder).unwrap();
        std::fs::create_dir_all(&target_folder).unwrap();
        let source_path = source_folder.join("edited-then-undone.png");
        let target_path = target_folder.join("copied.png");
        let bytes = vec![31_u8; HEAD_HASH_BYTES as usize + 13];
        std::fs::write(&source_path, &bytes).unwrap();
        let cancel = AtomicBool::new(false);
        let io_sem = crate::io_semaphore::GlobalIoSemaphore::new(1);

        // 実データを一度置いて台帳に復元元として登録し、そのあと編集だけ取り消す。
        // 台帳側は 0 -> 1 の単調遷移なので 1 のまま残る。
        give_test_file_restorable_content(temp.path(), &source_path);
        run_backfill_at(
            &db_path,
            vec![ContentIdentitySource::new(&source_path, ContentKind::Image)],
            &cancel,
            &io_sem,
            None,
        )
        .unwrap()
        .unwrap();
        let rating = crate::rating_db::RatingDb::open_at(temp.path().join("rating.db")).unwrap();
        rating
            .set(&crate::adjustment_db::normalize_path(&source_path), 0)
            .unwrap();
        drop(rating);
        let source_key = crate::path_key::normalize_keep_drive(&source_path);
        assert!(
            ContentIdentityDb::open_at(&db_path)
                .unwrap()
                .ledger_entry(&source_key)
                .unwrap()
                .unwrap()
                .has_restorable_content,
            "台帳の flag は編集の取り消しでは下がらない (この test の前提)",
        );

        std::fs::copy(&source_path, &target_path).unwrap();
        let db = ContentIdentityDb::open_at(&db_path).unwrap();
        let index = db.load_index(&cancel).unwrap().unwrap();
        let target = stage0_target(
            &index,
            ContentIdentitySource::new(&target_path, ContentKind::Image),
            bytes.len() as u64,
        )
        .expect("size index still lists the stale origin");
        drop(db);

        let detected = run_detection_at(
            &db_path,
            vec![target],
            3,
            crate::path_key::normalize_keep_drive(&target_folder),
            &cancel,
            &io_sem,
        )
        .unwrap()
        .unwrap();

        assert!(
            detected.candidates.is_empty(),
            "運ぶ行が無い復元元でダイアログを出さない: {:?}",
            detected.candidates,
        );
        assert!(
            !ContentIdentityDb::open_at(&db_path)
                .unwrap()
                .ledger_entry(&source_key)
                .unwrap()
                .unwrap()
                .has_restorable_content,
            "同じ探索を毎回繰り返さないよう flag も下ろす",
        );
    }

    /// 逆側。実データが残っている限り、候補から外さない。
    #[test]
    fn an_origin_that_still_has_edits_remains_a_restore_source() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("content_identity.db");
        let source_folder = temp.path().join("source");
        let target_folder = temp.path().join("target");
        std::fs::create_dir_all(&source_folder).unwrap();
        std::fs::create_dir_all(&target_folder).unwrap();
        let source_path = source_folder.join("still-edited.png");
        let target_path = target_folder.join("copied.png");
        let bytes = vec![37_u8; HEAD_HASH_BYTES as usize + 17];
        std::fs::write(&source_path, &bytes).unwrap();
        let cancel = AtomicBool::new(false);
        let io_sem = crate::io_semaphore::GlobalIoSemaphore::new(1);

        give_test_file_restorable_content(temp.path(), &source_path);
        run_backfill_at(
            &db_path,
            vec![ContentIdentitySource::new(&source_path, ContentKind::Image)],
            &cancel,
            &io_sem,
            None,
        )
        .unwrap()
        .unwrap();

        std::fs::copy(&source_path, &target_path).unwrap();
        let db = ContentIdentityDb::open_at(&db_path).unwrap();
        let index = db.load_index(&cancel).unwrap().unwrap();
        let target = stage0_target(
            &index,
            ContentIdentitySource::new(&target_path, ContentKind::Image),
            bytes.len() as u64,
        )
        .unwrap();
        drop(db);

        let detected = run_detection_at(
            &db_path,
            vec![target],
            4,
            crate::path_key::normalize_keep_drive(&target_folder),
            &cancel,
            &io_sem,
        )
        .unwrap()
        .unwrap();

        assert_eq!(detected.candidates.len(), 1);
        assert_eq!(
            detected.candidates[0].sources[0].file_key,
            crate::path_key::normalize_keep_drive(&source_path)
        );
    }

    #[test]
    fn book_byte_copy_is_declined_but_explorer_copy_in_same_book_is_detected() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let db_path = data_dir.join("content_identity.db");
        let source_folder = temp.path().join("source");
        let books_root = temp.path().join("books");
        std::fs::create_dir_all(&source_folder).unwrap();
        let source_path = source_folder.join("rated-only.jpg");
        let bytes = vec![23_u8; HEAD_HASH_BYTES as usize + 47];
        std::fs::write(&source_path, &bytes).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        give_test_file_restorable_content(&data_dir, &source_path);

        let cancel = AtomicBool::new(false);
        let io_sem = crate::io_semaphore::GlobalIoSemaphore::new(1);
        let backfill = run_backfill_at(
            &db_path,
            vec![ContentIdentitySource::new(&source_path, ContentKind::Image)],
            &cancel,
            &io_sem,
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(backfill.ledger_updates.len(), 1);

        let appended = crate::books::append_pages_at(
            data_dir.clone(),
            books_root.clone(),
            "target".to_string(),
            vec![crate::books::BookPageSource::File {
                src: source_path.clone(),
                original_name: "rated-only.jpg".to_string(),
            }],
        )
        .unwrap();
        let crate::books::BookOpResult::Append(summary) = appended else {
            panic!("expected book append");
        };
        let book_copy = summary.first_path.unwrap();
        assert_eq!(std::fs::read(&book_copy).unwrap(), bytes);

        let db = ContentIdentityDb::open_at(&db_path).unwrap();
        let index = db.load_index(&cancel).unwrap().unwrap();
        let source_hash = db
            .ledger_entry(&crate::path_key::normalize_keep_drive(&source_path))
            .unwrap()
            .unwrap()
            .full_hash
            .unwrap();
        assert!(
            db.restore_was_declined(
                &source_hash,
                &crate::path_key::normalize_keep_drive(&book_copy)
            )
            .unwrap()
        );
        drop(db);
        assert!(
            detect_test_image_copy(&db_path, &index, &book_copy).is_empty(),
            "mIV-created book copy must not be offered for restore"
        );

        let explorer_copy = books_root.join("target").join("explorer-copy.jpg");
        std::fs::copy(&source_path, &explorer_copy).unwrap();
        let candidates = detect_test_image_copy(&db_path, &index, &explorer_copy);
        assert_eq!(
            candidates.len(),
            1,
            "book location itself must not suppress an Explorer-created copy"
        );
        assert_eq!(
            candidates[0].sources[0].file_key,
            crate::path_key::normalize_keep_drive(&source_path)
        );
    }

    #[test]
    fn book_composited_page_does_not_create_a_restore_candidate() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let db_path = data_dir.join("content_identity.db");
        let source_path = temp.path().join("edited.png");
        let books_root = temp.path().join("books");
        let mut image = image::RgbaImage::new(2, 1);
        image.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        image.put_pixel(1, 0, image::Rgba([0, 0, 255, 255]));
        image.save(&source_path).unwrap();

        let cancel = AtomicBool::new(false);
        let io_sem = crate::io_semaphore::GlobalIoSemaphore::new(1);
        run_backfill_at(
            &db_path,
            vec![ContentIdentitySource::new(&source_path, ContentKind::Image)],
            &cancel,
            &io_sem,
            None,
        )
        .unwrap()
        .unwrap();

        let appended = crate::books::append_pages_at(
            data_dir,
            books_root,
            "target".to_string(),
            vec![crate::books::BookPageSource::Composited {
                source: crate::books::CompositeSource::File { path: source_path },
                basename: "edited.png".to_string(),
                edits: crate::books::BakedEditSnapshot {
                    params: crate::adjustment::AdjustParams::default(),
                    rotation: crate::rotation_db::Rotation::Cw90,
                    conceal: None,
                    erase: None,
                    local_adjust: None,
                    comic: None,
                    comic_source_dims: None,
                    export_crop: None,
                    crop_legacy_writeback: None,
                    format: crate::capture::CaptureFormat::Png,
                    jpeg_matte: crate::capture::JpegMatte::Black,
                },
            }],
        )
        .unwrap();
        let crate::books::BookOpResult::Append(summary) = appended else {
            panic!("expected book append");
        };
        let output = summary.first_path.unwrap();
        let db = ContentIdentityDb::open_at(&db_path).unwrap();
        let index = db.load_index(&cancel).unwrap().unwrap();
        drop(db);
        let output_size = std::fs::metadata(&output).unwrap().len();
        if let Some(target) = stage0_target(
            &index,
            ContentIdentitySource::new(&output, ContentKind::Image),
            output_size,
        ) {
            let result = run_detection_at(
                &db_path,
                vec![target],
                2,
                crate::path_key::normalize_keep_drive(output.parent().unwrap()),
                &cancel,
                &io_sem,
            )
            .unwrap()
            .unwrap();
            assert!(result.candidates.is_empty());
        }
    }

    #[test]
    fn schema_contains_both_a1_tables_and_full_hash_index() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("content_identity.db");
        let db = ContentIdentityDb::open_at(&path).unwrap();
        let version: i64 = db
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, CONTENT_IDENTITY_SCHEMA_VERSION);
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
        let columns = db
            .conn
            .prepare("PRAGMA table_info(edit_origin)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.iter().any(|name| name == "has_restorable_content"));
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
        let (update_tx, update_rx) = mpsc::channel();
        drop(rx);
        let recorder = ContentIdentityRecorder {
            tx: Some(tx),
            update_tx,
            update_rx,
            handle: None,
            shutdown: Arc::new(AtomicBool::new(false)),
        };
        recorder.record(
            ContentIdentitySource::new("C:/missing/image.png", ContentKind::Image),
            ContentIdentityTrigger::Edit,
        );
    }
}
