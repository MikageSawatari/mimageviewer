//! 明示操作によるポータブル・メタ情報のエクスポート / インポート。
//!
//! 既存の `mimageviewer.dat`（編集情報の自動 sidecar）とは責務を分離し、
//! `mimageviewer.meta.miv` directory に、フォルダ単位で分割したユーザー作成メタ情報をまとめる。
//! 自動 sidecar の設定が OFF でも、この manifest 単体で別環境へ復元できる。
//! このモジュールの公開 API は worker スレッドから呼ぶこと。

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SIDECAR_FILENAME: &str = crate::fs_entry::PORTABLE_METADATA_BUNDLE_DIRNAME;
/// v2.7.0では安定化のため非公開にしたが、v2.8.0の継続開発で再び有効化する。
/// 未リリース機能を緊急にrelease buildから外す場合だけ、このgateをfalseへ戻す。
pub(crate) const UI_ENABLED: bool = true;
const FORMAT_NAME: &str = "mimageviewer-portable-metadata";
const SHARD_FORMAT_NAME: &str = "mimageviewer-portable-metadata-shard";
const FORMAT_VERSION: u32 = 7;
const BUNDLE_MANIFEST_FILENAME: &str = "manifest.json";
const GENERATIONS_DIRNAME: &str = "generations";
const SHARDS_DIRNAME: &str = "shards";
const SHARD_EXTENSION: &str = "jsonl";
const EXPORT_BATCH_ENTRIES: usize = 4_096;
/// Importはこのいずれかへ達した時点でdurable commitする。item内はSAVEPOINTで
/// 隔離するため、1件の不正値は同じbatchの他itemを巻き戻さない。
///
/// プロセス異常終了時は現在batch（最大この予算）だけがrollbackされ、完了済みbatchは
/// 保持される。明示キャンセルと通常の走査エラーでは現在batchもcommitしてから終端へ進む。
const IMPORT_TRANSACTION_BATCH_ENTRIES: usize = 256;
const IMPORT_TRANSACTION_BATCH_BYTES: usize = 64 * 1024 * 1024;
const IMPORT_TRANSACTION_BATCH_TIME: std::time::Duration = std::time::Duration::from_millis(500);
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
/// 総exportサイズではなく、単一の物理項目recordにだけ適用する防御上限。
const MAX_RECORD_BYTES: u64 = 256 * 1024 * 1024;
const MAX_SHARD_HEADER_BYTES: u64 = 256 * 1024;
const MAX_PATH_CHARS: usize = 32_768;
const MAX_MEMBER_KEY_CHARS: usize = 65_536;
const MAX_TITLE_CHARS: usize = 1_024;
const MAX_BOOKMARKS_PER_ENTRY: usize = 100_000;
const MAX_RECURSION_DEPTH: usize = 40;
const MAX_MASK_PIXELS: u64 = 100_000_000;
const MAX_MASK_BYTES: usize = 64 * 1024 * 1024;
const MAX_VIDEO_THUMB_BYTES: usize = 16 * 1024 * 1024;
const MAX_REPORTED_PATHS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferPhase {
    Scanning,
    ReadingMetadata,
    WritingSidecar,
    ReadingSidecar,
    Importing,
}

#[derive(Clone, Debug)]
pub struct TransferProgress {
    pub phase: TransferPhase,
    pub processed: usize,
    pub total: usize,
    pub current_path: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportSummary {
    pub entries: usize,
    pub ratings: usize,
    pub tagged_items: usize,
    pub timed_bookmarks: usize,
    pub book_bookmarks: usize,
    pub page_states: usize,
    pub container_states: usize,
    pub thumbnail_pins: usize,
    /// セキュリティ上再帰しなかったjunction / symlink directoryの数。
    pub skipped_reparse_directories: usize,
    /// UIへ表示する先頭分。総数は`skipped_reparse_directories`を参照する。
    pub skipped_reparse_paths: Vec<String>,
    #[cfg(test)]
    pub metadata_batches: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportPreview {
    pub entries: usize,
    pub existing_entries: usize,
    pub missing_entries: usize,
    pub kind_mismatch_entries: usize,
    pub changed_files: usize,
    pub recursive: bool,
    pub exported_at_ms: i64,
}

#[derive(Clone, Debug, Default)]
pub struct ImportSummary {
    pub total_entries: usize,
    pub applied_entries: usize,
    pub skipped_missing: usize,
    pub skipped_kind_mismatch: usize,
    pub skipped_changed: usize,
    pub failed_entries: usize,
    /// UIへ表示する項目単位失敗の先頭分。総数は`failed_entries`を参照する。
    pub failed_items: Vec<ImportFailure>,
    pub cancelled: bool,
    /// preflight後の2回目の走査、または終端cache再取得で失敗した場合の終端エラー。
    /// それ以前にcommitしたbatchは保持される。
    pub incomplete_error: Option<String>,
    pub changed: ImportChangedSections,
    #[cfg(test)]
    pub transaction_batches: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportFailure {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportChangedSections {
    pub ratings: bool,
    pub tags: bool,
    pub timed_bookmarks: bool,
    pub book_bookmarks: bool,
    pub page_state: bool,
    pub container_state: bool,
    pub thumbnail_pins: bool,
}

impl ManifestSections {
    fn changed(self) -> ImportChangedSections {
        ImportChangedSections {
            ratings: self.ratings,
            tags: self.tags,
            timed_bookmarks: self.timed_bookmarks,
            book_bookmarks: self.book_bookmarks,
            page_state: self.page_state,
            container_state: self.container_state,
            thumbnail_pins: self.thumbnail_pins,
        }
    }
}

pub struct ImportPageStateSnapshot {
    pub adjusted: std::collections::BTreeSet<String>,
    pub local_adjusted: std::collections::BTreeSet<String>,
    pub masked: std::collections::BTreeSet<String>,
    pub concealed: std::collections::BTreeSet<String>,
    pub comic: std::collections::BTreeSet<String>,
    pub rotated: std::collections::BTreeSet<String>,
}

/// import完了後の全体編集badge索引をworker上で再構築する。UI側は各Setの所有権を
/// swapするだけなので、巨大familyの削除走査をUI frameへ持ち込まない。
pub fn load_import_page_state_snapshot(
    data_dir: &Path,
) -> Result<ImportPageStateSnapshot, TransferError> {
    load_import_page_state_snapshot_cancellable(data_dir, &AtomicBool::new(false))
}

/// 終端refresh worker向け。各DBの全体索引取得間で終了専用cancelを確認する。
pub fn load_import_page_state_snapshot_cancellable(
    data_dir: &Path,
    cancel: &AtomicBool,
) -> Result<ImportPageStateSnapshot, TransferError> {
    let adjusted = load_import_key_set(
        &data_dir.join("adjustment.db"),
        "SELECT page_path FROM page_params",
        cancel,
    )?;
    let local_adjusted = load_import_key_set(
        &data_dir.join("local_adjust.db"),
        "SELECT page_path FROM local_adjust_pages",
        cancel,
    )?;
    let masked = load_import_key_set(
        &data_dir.join("mask.db"),
        "SELECT path FROM masks WHERE path NOT LIKE '\\_\\_slot\\_%' ESCAPE '\\'",
        cancel,
    )?;
    let concealed = load_import_key_set(
        &data_dir.join("conceal.db"),
        "SELECT page_path FROM conceal_entries
         WHERE page_path NOT LIKE '\\_\\_slot\\_%' ESCAPE '\\'",
        cancel,
    )?;
    let comic = load_import_key_set(
        &data_dir.join("comic.db"),
        "SELECT page_path FROM comic_entries",
        cancel,
    )?;
    let rotated = load_import_key_set(
        &data_dir.join("rotation.db"),
        "SELECT path FROM rotations WHERE angle != 0",
        cancel,
    )?;
    Ok(ImportPageStateSnapshot {
        adjusted,
        local_adjusted,
        masked,
        concealed,
        comic,
        rotated,
    })
}

fn load_import_key_set(
    path: &Path,
    sql: &str,
    cancel: &AtomicBool,
) -> Result<std::collections::BTreeSet<String>, TransferError> {
    check_cancel(cancel)?;
    let conn = open_readonly(path)?;
    let mut statement = conn.prepare(sql).map_err(db_error)?;
    let mut rows = statement.query([]).map_err(db_error)?;
    let mut keys = std::collections::BTreeSet::new();
    let mut processed = 0usize;
    while let Some(row) = rows.next().map_err(db_error)? {
        if processed % 512 == 0 {
            check_cancel(cancel)?;
        }
        keys.insert(row.get::<_, String>(0).map_err(db_error)?);
        processed = processed.saturating_add(1);
    }
    check_cancel(cancel)?;
    Ok(keys)
}

#[derive(Debug)]
pub enum TransferError {
    Cancelled,
    Io(String),
    Invalid(String),
    Database(String),
}

impl fmt::Display for TransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => write!(f, "キャンセルされました"),
            Self::Io(message) => write!(f, "ファイル操作に失敗しました: {message}"),
            Self::Invalid(message) => write!(f, "メタ情報ファイルが不正です: {message}"),
            Self::Database(message) => write!(f, "メタ情報DBの操作に失敗しました: {message}"),
        }
    }
}

impl std::error::Error for TransferError {}

#[cfg(test)]
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    exported_at_ms: i64,
    recursive: bool,
    sections: ManifestSections,
    entries: Vec<PortableEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BundleManifest {
    format: String,
    version: u32,
    generation: String,
    exported_at_ms: i64,
    recursive: bool,
    sections: ManifestSections,
    shard_count: u64,
    entry_count: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ShardHeader {
    format: String,
    version: u32,
    generation: String,
    folder: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct ManifestSections {
    ratings: bool,
    tags: bool,
    timed_bookmarks: bool,
    book_bookmarks: bool,
    #[serde(default)]
    page_state: bool,
    #[serde(default)]
    container_state: bool,
    #[serde(default)]
    thumbnail_pins: bool,
}

impl Default for ManifestSections {
    fn default() -> Self {
        Self {
            ratings: true,
            tags: true,
            timed_bookmarks: true,
            book_bookmarks: true,
            page_state: true,
            container_state: true,
            thumbnail_pins: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PortableEntryKind {
    File,
    Directory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PortableMediaKind {
    Directory,
    Image,
    Video,
    Audio,
    Zip,
    Pdf,
    ConvertibleArchive,
    OtherFile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PortableVirtualKeyBase {
    Source,
    ConvertedCache,
}

impl Default for PortableVirtualKeyBase {
    fn default() -> Self {
        Self::Source
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FileFingerprint {
    size: u64,
    modified_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableEntry {
    path: String,
    kind: PortableEntryKind,
    media_kind: PortableMediaKind,
    virtual_key_base: PortableVirtualKeyBase,
    container_key_base: PortableVirtualKeyBase,
    fingerprint: Option<FileFingerprint>,
    rating: Option<PortableRating>,
    #[serde(default)]
    tags: Vec<PortableTag>,
    tags_decided: bool,
    #[serde(default)]
    timed_bookmarks: Vec<PortableTimedBookmark>,
    #[serde(default)]
    book_bookmarks: Vec<PortableBookBookmark>,
    #[serde(default, skip_serializing_if = "PortablePageState::is_empty")]
    page_state: PortablePageState,
    #[serde(default, skip_serializing_if = "PortableContainerState::is_empty")]
    container_state: PortableContainerState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    nested_containers: Vec<PortableNestedContainer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    video_pin: Option<PortableVideoPin>,
    #[serde(default)]
    virtual_items: Vec<PortableVirtualItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableVirtualItem {
    member_key: String,
    rating: Option<PortableRating>,
    #[serde(default)]
    tags: Vec<PortableTag>,
    tags_decided: bool,
    #[serde(default, skip_serializing_if = "PortablePageState::is_empty")]
    page_state: PortablePageState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableTag {
    name: String,
    /// `tags.db.item_tags.applied_at` と同じUnix秒。
    applied_at: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PortablePageState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rotation_degrees: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adjustment: Option<crate::adjustment::AdjustParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mask: Option<crate::sidecar::SidecarMask>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conceal: Option<crate::sidecar::SidecarMask>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_adjust_layers: Option<Vec<local_adjust_core::LocalAdjustmentLayer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    export_crop: Option<crate::export_crop::CropSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    comic: Option<Vec<comic_core::AnnotationObject>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    view_trim: Option<crate::view_trim::ViewTrimPageOverride>,
}

impl PortablePageState {
    fn is_empty(&self) -> bool {
        self.rotation_degrees.is_none()
            && self.adjustment.is_none()
            && self.mask.is_none()
            && self.conceal.is_none()
            && self.local_adjust_layers.is_none()
            && self.export_crop.is_none()
            && self.comic.is_none()
            && self.view_trim.is_none()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PortableContainerState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spread: Option<PortableSpreadState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    view_trim: Option<crate::view_trim::ViewTrimBookState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    folder_thumb_pin: Option<PortableFolderThumbPin>,
}

impl PortableContainerState {
    fn is_empty(&self) -> bool {
        self.spread.is_none() && self.view_trim.is_none() && self.folder_thumb_pin.is_none()
    }

    fn has_view_state(&self) -> bool {
        self.spread.is_some() || self.view_trim.is_some()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableNestedContainer {
    member_key: String,
    #[serde(default)]
    state: PortableContainerState,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct PortableSpreadState {
    mode: i32,
    flow: i32,
    direction: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableFolderThumbPin {
    source_kind: String,
    source_rel: String,
    source_entry: Option<String>,
    source_page: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableVideoPin {
    pin_pts_secs: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thumb_webp_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thumb_pts_secs: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableRating {
    stars: u8,
    rated_at_ms: Option<i64>,
    kind: Option<String>,
    entry_name: Option<String>,
    page_num: Option<u32>,
    dir_prefix: Option<String>,
    archive_format: Option<String>,
    zipdir_is_archive: Option<bool>,
    zipdir_representative: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableTimedBookmark {
    pts_secs: f64,
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thumb_webp_base64: Option<String>,
    created_at_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableBookBookmark {
    container_kind: String,
    page_kind: String,
    page_value: String,
    page_index_hint: usize,
    created_at_ms: i64,
    title: Option<String>,
}

struct EnumeratedEntry {
    path: PathBuf,
    portable: PortableEntry,
}

struct ShardedEnumeratedEntry {
    shard_path: PathBuf,
    entry: EnumeratedEntry,
}

#[derive(Default)]
struct BundleCounts {
    shards: u64,
    entries: u64,
}

/// `root` と同じフォルダの固定 sidecar directory へエクスポートする。
/// folder shardをbatch単位で逐次生成し、完成したgenerationだけをpointerで公開する。
pub fn export_at<F>(
    data_dir: &Path,
    root: &Path,
    recursive: bool,
    cancel: &AtomicBool,
    mut progress: F,
) -> Result<ExportSummary, TransferError>
where
    F: FnMut(TransferProgress),
{
    validate_root(root)?;
    progress(TransferProgress {
        phase: TransferPhase::Scanning,
        processed: 0,
        total: 0,
        current_path: None,
    });

    let exported_at_ms = now_ms();
    let generation = uuid::Uuid::new_v4().simple().to_string();
    let mut staging = BundleStaging::create(root, &generation)?;
    let mut summary = ExportSummary::default();
    let mut counts = BundleCounts::default();
    let mut batch = Vec::with_capacity(EXPORT_BATCH_ENTRIES);
    let mut visited = DiskDuplicateGuard::new()?;
    let _ = visited.insert(&crate::fs_entry::directory_visit_key(root))?;
    let shards_dir = staging.shards_dir();
    let export_result = export_directory_shard(
        data_dir,
        root,
        root,
        recursive,
        0,
        &generation,
        &shards_dir,
        cancel,
        &mut progress,
        &mut visited,
        &mut batch,
        &mut counts,
        &mut summary,
    );
    if let Err(error) = export_result {
        staging.abort();
        return Err(error);
    }
    flush_export_batch(
        data_dir,
        &mut batch,
        cancel,
        &mut progress,
        &mut counts,
        &mut summary,
    )?;
    check_cancel(cancel)?;

    let manifest = BundleManifest {
        format: FORMAT_NAME.to_string(),
        version: FORMAT_VERSION,
        generation,
        exported_at_ms,
        recursive,
        sections: ManifestSections::default(),
        shard_count: counts.shards,
        entry_count: counts.entries,
    };
    validate_bundle_manifest(&manifest)?;
    progress(TransferProgress {
        phase: TransferPhase::WritingSidecar,
        processed: 0,
        total: usize_from_count(manifest.entry_count, "項目数")?,
        current_path: Some(SIDECAR_FILENAME.to_string()),
    });
    staging.publish(&manifest, cancel)?;
    progress(TransferProgress {
        phase: TransferPhase::WritingSidecar,
        processed: usize_from_count(manifest.entry_count, "項目数")?,
        total: usize_from_count(manifest.entry_count, "項目数")?,
        current_path: Some(SIDECAR_FILENAME.to_string()),
    });
    Ok(summary)
}

/// sidecar を検証し、適用前の確認画面に必要な件数を返す。DB は変更しない。
pub fn inspect_import_at<F>(
    root: &Path,
    cancel: &AtomicBool,
    mut progress: F,
) -> Result<ImportPreview, TransferError>
where
    F: FnMut(TransferProgress),
{
    validate_root(root)?;
    progress(TransferProgress {
        phase: TransferPhase::ReadingSidecar,
        processed: 0,
        total: 0,
        current_path: Some(SIDECAR_FILENAME.to_string()),
    });
    let manifest = read_bundle_manifest(root, cancel)?;
    let total = usize_from_count(manifest.entry_count, "項目数")?;
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| TransferError::Io(format!("{}: {error}", root.display())))?;
    let mut preview = ImportPreview {
        entries: total,
        recursive: manifest.recursive,
        exported_at_ms: manifest.exported_at_ms,
        ..ImportPreview::default()
    };
    visit_bundle_entries(root, &manifest, cancel, |entry, index, _record_bytes| {
        check_cancel(cancel)?;
        let path = resolve_entry_path(root, &canonical_root, &entry.path)?;
        validate_bookmark_page_targets(&path, entry)?;
        match verify_target(&path, entry, cancel)? {
            TargetState::Ready => preview.existing_entries += 1,
            TargetState::Missing => preview.missing_entries += 1,
            TargetState::KindMismatch => preview.kind_mismatch_entries += 1,
            TargetState::Changed => preview.changed_files += 1,
        }
        progress(TransferProgress {
            phase: TransferPhase::ReadingSidecar,
            processed: index + 1,
            total,
            current_path: Some(entry.path.clone()),
        });
        Ok(())
    })?;
    Ok(preview)
}

/// sidecar に記載された物理項目を 1 件ずつ上書きする。
/// キャンセル時は完了済み項目を保持し、未着手項目には触れない。
pub fn import_at<F>(
    data_dir: &Path,
    root: &Path,
    cancel: &AtomicBool,
    progress: F,
) -> Result<ImportSummary, TransferError>
where
    F: FnMut(TransferProgress),
{
    let started = std::time::Instant::now();
    if crate::perf::is_enabled() {
        crate::perf::event("metadata_import", "begin", None, 0, &[]);
    }
    let result = import_at_batched(data_dir, root, cancel, progress);
    let outcome = match &result {
        Ok(summary) if summary.cancelled => "cancelled",
        Ok(summary) if summary.incomplete_error.is_some() || summary.failed_entries > 0 => {
            "partial"
        }
        Ok(_) => "success",
        Err(_) => "error",
    };
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    if crate::perf::is_enabled() {
        crate::perf::event(
            "metadata_import",
            "end",
            None,
            0,
            &[
                ("outcome", serde_json::Value::from(outcome)),
                ("ms", serde_json::Value::from(elapsed_ms)),
            ],
        );
    }
    let error_suffix = result
        .as_ref()
        .err()
        .map(|error| format!(" error={error}"))
        .unwrap_or_default();
    crate::logger::log(format!(
        "metadata import end: outcome={outcome} total_ms={elapsed_ms:.1}{error_suffix}"
    ));
    result
}

fn import_at_batched<F>(
    data_dir: &Path,
    root: &Path,
    cancel: &AtomicBool,
    mut progress: F,
) -> Result<ImportSummary, TransferError>
where
    F: FnMut(TransferProgress),
{
    validate_root(root)?;
    let manifest_started = std::time::Instant::now();
    let manifest = read_bundle_manifest(root, cancel)?;
    let manifest_ms = manifest_started.elapsed().as_secs_f64() * 1000.0;
    let total = usize_from_count(manifest.entry_count, "項目数")?;
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| TransferError::Io(format!("{}: {error}", root.display())))?;
    let preflight_started = std::time::Instant::now();
    visit_bundle_entries(root, &manifest, cancel, |entry, index, _record_bytes| {
        let path = resolve_entry_path(root, &canonical_root, &entry.path)?;
        validate_bookmark_page_targets(&path, entry)?;
        progress(TransferProgress {
            phase: TransferPhase::ReadingSidecar,
            processed: index + 1,
            total,
            current_path: Some(entry.path.clone()),
        });
        Ok(())
    })?;
    let preflight_ms = preflight_started.elapsed().as_secs_f64() * 1000.0;

    let database_open_started = std::time::Instant::now();
    ensure_database_schemas(data_dir)?;
    let conn = open_import_connection(data_dir)?;
    let database_open_ms = database_open_started.elapsed().as_secs_f64() * 1000.0;
    let mut summary = ImportSummary {
        total_entries: total,
        changed: manifest.sections.changed(),
        ..ImportSummary::default()
    };
    let mut batch_entries = 0usize;
    let mut batch_bytes = 0usize;
    let mut batch_applied = 0usize;
    let mut batch_started = std::time::Instant::now();
    let mut batch_active = false;
    let apply_started = std::time::Instant::now();
    let mut target_verify_ms = 0.0_f64;
    let mut sql_apply_ms = 0.0_f64;
    let mut commit_ms = 0.0_f64;
    let mut max_commit_ms = 0.0_f64;
    let mut transaction_batches = 0usize;
    let mut applied_virtual_items = 0usize;
    let mut applied_record_bytes = 0usize;
    let mut automatic_sidecar_sync_cache: HashMap<PathBuf, Option<(String, i64)>> = HashMap::new();
    let mut automatic_sidecar_adjustment_sync_written: HashSet<String> = HashSet::new();
    let mut automatic_sidecar_tag_sync_written: HashSet<String> = HashSet::new();

    let apply_result =
        visit_bundle_entries(root, &manifest, cancel, |entry, index, record_bytes| {
            progress(TransferProgress {
                phase: TransferPhase::Importing,
                processed: index,
                total,
                current_path: Some(entry.path.clone()),
            });
            let target_started = std::time::Instant::now();
            let path = resolve_entry_path(root, &canonical_root, &entry.path)?;
            validate_bookmark_page_targets(&path, entry)?;
            let target_state = verify_target(&path, entry, cancel)?;
            target_verify_ms += target_started.elapsed().as_secs_f64() * 1000.0;
            match target_state {
                TargetState::Missing => summary.skipped_missing += 1,
                TargetState::KindMismatch => summary.skipped_kind_mismatch += 1,
                TargetState::Changed => summary.skipped_changed += 1,
                TargetState::Ready => {
                    if !batch_active {
                        begin_import_batch(&conn)?;
                        batch_active = true;
                        batch_started = std::time::Instant::now();
                    }
                    let discovered_sidecar_sync = automatic_sidecar_sync_cached(
                        &mut automatic_sidecar_sync_cache,
                        &path,
                        entry.kind,
                    );
                    let sidecar_sync =
                        discovered_sidecar_sync
                            .as_ref()
                            .and_then(|(folder_key, modified_secs)| {
                                let write_adjustment = manifest.sections.page_state
                                    && matches!(
                                        entry.media_kind,
                                        PortableMediaKind::Image
                                            | PortableMediaKind::Zip
                                            | PortableMediaKind::Pdf
                                            | PortableMediaKind::ConvertibleArchive
                                    )
                                    && !automatic_sidecar_adjustment_sync_written
                                        .contains(folder_key);
                                let write_tags = manifest.sections.tags
                                    && !automatic_sidecar_tag_sync_written.contains(folder_key);
                                (write_adjustment || write_tags).then(|| {
                                    (
                                        folder_key.clone(),
                                        *modified_secs,
                                        write_adjustment,
                                        write_tags,
                                    )
                                })
                            });
                    let sidecar_sync_flags = sidecar_sync.as_ref().map(
                        |(folder_key, _, write_adjustment, write_tags)| {
                            (folder_key.clone(), *write_adjustment, *write_tags)
                        },
                    );
                    let item_apply_started = std::time::Instant::now();
                    conn.execute_batch("SAVEPOINT metadata_import_item")
                        .map_err(db_error)?;
                    match apply_entry(
                        &conn,
                        data_dir,
                        &path,
                        entry,
                        manifest.sections,
                        manifest.exported_at_ms,
                        sidecar_sync,
                    ) {
                        Ok(()) => {
                            conn.execute_batch("RELEASE metadata_import_item")
                                .map_err(db_error)?;
                            if let Some((folder_key, write_adjustment, write_tags)) =
                                sidecar_sync_flags
                            {
                                if write_adjustment {
                                    automatic_sidecar_adjustment_sync_written
                                        .insert(folder_key.clone());
                                }
                                if write_tags {
                                    automatic_sidecar_tag_sync_written.insert(folder_key);
                                }
                            }
                            batch_applied = batch_applied.saturating_add(1);
                        }
                        Err(error) => {
                            conn.execute_batch(
                                "ROLLBACK TO metadata_import_item; RELEASE metadata_import_item",
                            )
                            .map_err(db_error)?;
                            crate::logger::log(format!(
                                "metadata import: failed to apply {}: {error}",
                                entry.path
                            ));
                            summary.failed_entries = summary.failed_entries.saturating_add(1);
                            if summary.failed_items.len() < MAX_REPORTED_PATHS {
                                summary.failed_items.push(ImportFailure {
                                    path: entry.path.clone(),
                                    reason: error.to_string(),
                                });
                            }
                        }
                    }
                    sql_apply_ms += item_apply_started.elapsed().as_secs_f64() * 1000.0;
                    batch_entries = batch_entries.saturating_add(1);
                    batch_bytes = batch_bytes.saturating_add(record_bytes);
                    applied_record_bytes = applied_record_bytes.saturating_add(record_bytes);
                    applied_virtual_items =
                        applied_virtual_items.saturating_add(entry.virtual_items.len());
                }
            }
            if batch_entries > 0
                && (batch_entries >= IMPORT_TRANSACTION_BATCH_ENTRIES
                    || batch_bytes >= IMPORT_TRANSACTION_BATCH_BYTES
                    || batch_started.elapsed() >= IMPORT_TRANSACTION_BATCH_TIME)
            {
                let commit_started = std::time::Instant::now();
                commit_import_batch(&conn)?;
                let current_commit_ms = commit_started.elapsed().as_secs_f64() * 1000.0;
                commit_ms += current_commit_ms;
                max_commit_ms = max_commit_ms.max(current_commit_ms);
                transaction_batches = transaction_batches.saturating_add(1);
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "metadata_import",
                        "batch_commit",
                        None,
                        0,
                        &[
                            ("batch", serde_json::Value::from(transaction_batches as u64)),
                            ("entries", serde_json::Value::from(batch_entries as u64)),
                            ("applied", serde_json::Value::from(batch_applied as u64)),
                            ("bytes", serde_json::Value::from(batch_bytes as u64)),
                            ("commit_ms", serde_json::Value::from(current_commit_ms)),
                        ],
                    );
                }
                summary.applied_entries = summary.applied_entries.saturating_add(batch_applied);
                #[cfg(test)]
                {
                    summary.transaction_batches += 1;
                }
                batch_active = false;
                batch_entries = 0;
                batch_bytes = 0;
                batch_applied = 0;
                batch_started = std::time::Instant::now();
            }
            progress(TransferProgress {
                phase: TransferPhase::Importing,
                processed: index + 1,
                total,
                current_path: Some(entry.path.clone()),
            });
            Ok(())
        });

    // cancel / shard I/O errorでも、仕様どおり完了済みitemは保持する。異常終了だけは
    // SQLiteが現在batchを一括rollbackする。
    let commit_result = if batch_active {
        let commit_started = std::time::Instant::now();
        let result = commit_import_batch(&conn);
        let current_commit_ms = commit_started.elapsed().as_secs_f64() * 1000.0;
        commit_ms += current_commit_ms;
        max_commit_ms = max_commit_ms.max(current_commit_ms);
        if result.is_ok() {
            transaction_batches = transaction_batches.saturating_add(1);
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "metadata_import",
                    "batch_commit",
                    None,
                    0,
                    &[
                        ("batch", serde_json::Value::from(transaction_batches as u64)),
                        ("entries", serde_json::Value::from(batch_entries as u64)),
                        ("applied", serde_json::Value::from(batch_applied as u64)),
                        ("bytes", serde_json::Value::from(batch_bytes as u64)),
                        ("commit_ms", serde_json::Value::from(current_commit_ms)),
                    ],
                );
            }
        }
        result
    } else {
        Ok(())
    };
    if commit_result.is_ok() {
        summary.applied_entries = summary.applied_entries.saturating_add(batch_applied);
        #[cfg(test)]
        if batch_active {
            summary.transaction_batches += 1;
        }
    }
    let terminal = apply_result.and(commit_result);
    match terminal {
        Ok(()) => {}
        Err(TransferError::Cancelled) => summary.cancelled = true,
        Err(error) => summary.incomplete_error = Some(error.to_string()),
    }
    let apply_ms = apply_started.elapsed().as_secs_f64() * 1000.0;
    let outcome = if summary.cancelled {
        "cancelled"
    } else if summary.incomplete_error.is_some() || summary.failed_entries > 0 {
        "partial"
    } else {
        "success"
    };
    crate::logger::log(format!(
        "metadata import: outcome={outcome} entries={} applied={} virtual={} bytes={} \
         manifest_ms={manifest_ms:.1} preflight_ms={preflight_ms:.1} db_open_ms={database_open_ms:.1} \
         apply_ms={apply_ms:.1} target_verify_ms={target_verify_ms:.1} sql_apply_ms={sql_apply_ms:.1} \
         commits={transaction_batches} commit_ms={commit_ms:.1} max_commit_ms={max_commit_ms:.1}",
        summary.total_entries, summary.applied_entries, applied_virtual_items, applied_record_bytes,
    ));
    if crate::perf::is_enabled() {
        crate::perf::event(
            "metadata_import",
            "apply_summary",
            None,
            0,
            &[
                ("outcome", serde_json::Value::from(outcome)),
                (
                    "entries",
                    serde_json::Value::from(summary.total_entries as u64),
                ),
                (
                    "applied",
                    serde_json::Value::from(summary.applied_entries as u64),
                ),
                (
                    "virtual_items",
                    serde_json::Value::from(applied_virtual_items as u64),
                ),
                (
                    "bytes",
                    serde_json::Value::from(applied_record_bytes as u64),
                ),
                ("manifest_ms", serde_json::Value::from(manifest_ms)),
                ("preflight_ms", serde_json::Value::from(preflight_ms)),
                ("db_open_ms", serde_json::Value::from(database_open_ms)),
                ("apply_ms", serde_json::Value::from(apply_ms)),
                (
                    "target_verify_ms",
                    serde_json::Value::from(target_verify_ms),
                ),
                ("sql_apply_ms", serde_json::Value::from(sql_apply_ms)),
                (
                    "commits",
                    serde_json::Value::from(transaction_batches as u64),
                ),
                ("commit_ms", serde_json::Value::from(commit_ms)),
                ("max_commit_ms", serde_json::Value::from(max_commit_ms)),
            ],
        );
    }
    Ok(summary)
}

fn begin_import_batch(conn: &Connection) -> Result<(), TransferError> {
    conn.execute_batch("BEGIN IMMEDIATE").map_err(db_error)
}

fn commit_import_batch(conn: &Connection) -> Result<(), TransferError> {
    conn.execute_batch("COMMIT").map_err(db_error)
}

/// 仮想項目のDB identity。ZIP member名は表示・manifestではcaseを保持するが、
/// mIVのpage keyはcase-insensitiveなので全store・refresh経路で同じ形に揃える。
fn canonical_virtual_item_key(base_key: &str, member_key: &str) -> String {
    format!("{base_key}::{}", canonical_member_key(member_key))
}

/// archive member / 仮想containerの共通identity。manifestではWindows由来の`\`も
/// 有効な区切りとして受理するため、重複検査と全DB key生成の前に`/`へ統一する。
fn canonical_member_key(member_key: &str) -> String {
    member_key.replace('\\', "/").to_lowercase()
}

fn validate_root(root: &Path) -> Result<(), TransferError> {
    if !root.is_dir() {
        return Err(TransferError::Invalid(format!(
            "対象フォルダが存在しません: {}",
            root.display()
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn export_directory_shard<F>(
    data_dir: &Path,
    root: &Path,
    dir: &Path,
    recursive: bool,
    depth: usize,
    generation: &str,
    shards_dir: &Path,
    cancel: &AtomicBool,
    progress: &mut F,
    visited: &mut DiskDuplicateGuard,
    batch: &mut Vec<ShardedEnumeratedEntry>,
    counts: &mut BundleCounts,
    summary: &mut ExportSummary,
) -> Result<(), TransferError>
where
    F: FnMut(TransferProgress),
{
    if depth > MAX_RECURSION_DEPTH {
        return Err(TransferError::Invalid(format!(
            "フォルダ階層が深すぎます（上限 {MAX_RECURSION_DEPTH} 階層）"
        )));
    }
    check_cancel(cancel)?;
    let folder = if dir == root {
        ".".to_string()
    } else {
        relative_string(root, dir)?
    };
    let shard_path = shards_dir.join(shard_filename(&folder));
    let file = File::options()
        .write(true)
        .create_new(true)
        .open(&shard_path)
        .map_err(|error| {
            TransferError::Io(format!(
                "{}: {error}（大小文字だけが異なるフォルダがないか確認してください）",
                shard_path.display()
            ))
        })?;
    let mut writer = BufWriter::new(file);
    write_json_line(
        &mut writer,
        &ShardHeader {
            format: SHARD_FORMAT_NAME.to_string(),
            version: FORMAT_VERSION,
            generation: generation.to_string(),
            folder: folder.clone(),
        },
        cancel,
        MAX_SHARD_HEADER_BYTES,
    )?;
    counts.shards = counts
        .shards
        .checked_add(1)
        .ok_or_else(|| TransferError::Invalid("shard数が表現範囲を超えています".into()))?;
    writer
        .flush()
        .map_err(|error| TransferError::Io(format!("{}: {error}", shard_path.display())))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|error| TransferError::Io(format!("{}: {error}", shard_path.display())))?;
    drop(writer);

    if dir == root {
        batch.push(ShardedEnumeratedEntry {
            shard_path: shard_path.clone(),
            entry: make_enumerated(root, ".".to_string(), PortableEntryKind::Directory, cancel)?,
        });
    }
    let children = fs::read_dir(dir)
        .map_err(|error| TransferError::Io(format!("{}: {error}", dir.display())))?;
    for child in children {
        check_cancel(cancel)?;
        let child = child.map_err(|error| TransferError::Io(error.to_string()))?;
        if is_sidecar_name(&child.file_name())
            || is_automatic_sidecar_name(&child.file_name())
            || is_temp_sidecar_name(&child.file_name())
        {
            continue;
        }
        let file_type = child
            .file_type()
            .map_err(|error| TransferError::Io(error.to_string()))?;
        let kind = crate::fs_entry::classify_dir_entry(&child, &file_type);
        let portable_kind = match kind {
            crate::fs_entry::DirEntryKind::Directory
            | crate::fs_entry::DirEntryKind::ReparseDirectory => PortableEntryKind::Directory,
            crate::fs_entry::DirEntryKind::File => PortableEntryKind::File,
            crate::fs_entry::DirEntryKind::Other => continue,
        };
        let path = child.path();
        let rel = relative_string(root, &path)?;
        batch.push(ShardedEnumeratedEntry {
            shard_path: shard_path.clone(),
            entry: make_enumerated(&path, rel.clone(), portable_kind, cancel)?,
        });
        if recursive && kind == crate::fs_entry::DirEntryKind::ReparseDirectory {
            summary.skipped_reparse_directories =
                summary.skipped_reparse_directories.saturating_add(1);
            if summary.skipped_reparse_paths.len() < MAX_REPORTED_PATHS {
                summary.skipped_reparse_paths.push(rel.clone());
            }
        }
        progress(TransferProgress {
            phase: TransferPhase::Scanning,
            processed: usize_from_count(
                counts.entries.saturating_add(batch.len() as u64),
                "項目数",
            )?,
            total: 0,
            current_path: Some(rel),
        });
        if batch.len() >= EXPORT_BATCH_ENTRIES {
            flush_export_batch(data_dir, batch, cancel, progress, counts, summary)?;
        }
        if recursive
            && kind == crate::fs_entry::DirEntryKind::Directory
            && visited.insert(&crate::fs_entry::directory_visit_key(&path))?
        {
            export_directory_shard(
                data_dir,
                root,
                &path,
                true,
                depth + 1,
                generation,
                shards_dir,
                cancel,
                progress,
                visited,
                batch,
                counts,
                summary,
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn flush_export_batch<F>(
    data_dir: &Path,
    batch: &mut Vec<ShardedEnumeratedEntry>,
    cancel: &AtomicBool,
    progress: &mut F,
    counts: &mut BundleCounts,
    summary: &mut ExportSummary,
) -> Result<(), TransferError>
where
    F: FnMut(TransferProgress),
{
    if batch.is_empty() {
        return Ok(());
    }
    #[cfg(test)]
    {
        summary.metadata_batches += 1;
    }
    let drained = std::mem::replace(batch, Vec::with_capacity(EXPORT_BATCH_ENTRIES));
    let mut destinations = drained
        .iter()
        .map(|entry| entry.shard_path.clone())
        .collect::<Vec<_>>();
    let mut entries = drained
        .into_iter()
        .map(|entry| entry.entry)
        .collect::<Vec<_>>();
    attach_metadata(data_dir, &mut entries, cancel, progress)?;
    let mut entries = entries.into_iter();
    debug_assert_eq!(entries.len(), destinations.len());
    let mut current_writer: Option<(PathBuf, BufWriter<File>)> = None;
    for (entry, shard_path) in entries.by_ref().zip(destinations.drain(..)) {
        check_cancel(cancel)?;
        validate_portable_entry(&entry.portable)?;
        if current_writer
            .as_ref()
            .is_none_or(|(current, _)| current != &shard_path)
        {
            if let Some((path, mut writer)) = current_writer.take() {
                flush_and_sync_shard(&path, &mut writer)?;
            }
            let file = File::options()
                .append(true)
                .open(&shard_path)
                .map_err(|error| TransferError::Io(format!("{}: {error}", shard_path.display())))?;
            current_writer = Some((shard_path.clone(), BufWriter::new(file)));
        }
        write_json_line(
            &mut current_writer.as_mut().expect("opened above").1,
            &entry.portable,
            cancel,
            MAX_RECORD_BYTES,
        )?;
        summarize_entry(summary, &entry.portable);
        counts.entries = counts
            .entries
            .checked_add(1)
            .ok_or_else(|| TransferError::Invalid("項目数が表現範囲を超えています".into()))?;
    }
    if let Some((path, mut writer)) = current_writer.take() {
        flush_and_sync_shard(&path, &mut writer)?;
    }
    Ok(())
}

fn flush_and_sync_shard(path: &Path, writer: &mut BufWriter<File>) -> Result<(), TransferError> {
    writer
        .flush()
        .map_err(|error| TransferError::Io(format!("{}: {error}", path.display())))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|error| TransferError::Io(format!("{}: {error}", path.display())))
}

fn make_enumerated(
    path: &Path,
    rel: String,
    kind: PortableEntryKind,
    cancel: &AtomicBool,
) -> Result<EnumeratedEntry, TransferError> {
    check_cancel(cancel)?;
    let media_kind = portable_media_kind(path, kind);
    let fingerprint = if kind == PortableEntryKind::File {
        let metadata = fs::metadata(path)
            .map_err(|error| TransferError::Io(format!("{}: {error}", path.display())))?;
        Some(FileFingerprint {
            size: metadata.len(),
            modified_ms: metadata.modified().ok().and_then(system_time_ms),
        })
    } else {
        None
    };
    Ok(EnumeratedEntry {
        path: path.to_path_buf(),
        portable: PortableEntry {
            path: rel,
            kind,
            media_kind,
            virtual_key_base: PortableVirtualKeyBase::Source,
            container_key_base: PortableVirtualKeyBase::Source,
            fingerprint,
            rating: None,
            tags: Vec::new(),
            tags_decided: false,
            timed_bookmarks: Vec::new(),
            book_bookmarks: Vec::new(),
            page_state: PortablePageState::default(),
            container_state: PortableContainerState::default(),
            nested_containers: Vec::new(),
            video_pin: None,
            virtual_items: Vec::new(),
        },
    })
}

fn portable_media_kind(path: &Path, kind: PortableEntryKind) -> PortableMediaKind {
    if kind == PortableEntryKind::Directory {
        return PortableMediaKind::Directory;
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if crate::folder_tree::is_recognized_image_ext(&extension) {
        PortableMediaKind::Image
    } else if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        PortableMediaKind::Video
    } else if crate::folder_tree::SUPPORTED_AUDIO_EXTENSIONS.contains(&extension.as_str()) {
        PortableMediaKind::Audio
    } else if crate::folder_tree::is_zip_extension(&extension) {
        PortableMediaKind::Zip
    } else if extension == "pdf" {
        PortableMediaKind::Pdf
    } else if crate::archive_converter::ArchiveFormat::from_extension(&extension).is_some() {
        PortableMediaKind::ConvertibleArchive
    } else {
        PortableMediaKind::OtherFile
    }
}

/// read-only のメタ情報 DB ごとに、一時DBへ今回の物理キーだけを登録する。
/// `CROSS JOIN` の外側をこの小さい表に固定し、主DBの path index を exact/range seek
/// することで、対象フォルダが小さいときにグローバルDB全体を走査しない。
fn prepare_metadata_scope(
    conn: &mut Connection,
    physical_index: &HashMap<String, usize>,
    page_index: &HashMap<String, usize>,
    cancel: &AtomicBool,
) -> Result<(), TransferError> {
    conn.execute_batch(
        "CREATE TEMP TABLE metadata_transfer_scope (
            item_key      TEXT PRIMARY KEY,
            virtual_lower TEXT NOT NULL,
            virtual_upper TEXT NOT NULL
         ) WITHOUT ROWID;
         CREATE TEMP TABLE metadata_transfer_physical_scope (
             item_key TEXT PRIMARY KEY
         ) WITHOUT ROWID;",
    )
    .map_err(db_error)?;
    let tx = conn.transaction().map_err(db_error)?;
    {
        let mut insert_page = tx
            .prepare(
                "INSERT INTO metadata_transfer_scope
                    (item_key, virtual_lower, virtual_upper)
                 VALUES (?1, ?2, ?3)",
            )
            .map_err(db_error)?;
        for key in page_index.keys() {
            check_cancel(cancel)?;
            insert_page
                .execute(params![key, format!("{key}::"), format!("{key}:;")])
                .map_err(db_error)?;
        }
        let mut insert_physical = tx
            .prepare("INSERT INTO metadata_transfer_physical_scope (item_key) VALUES (?1)")
            .map_err(db_error)?;
        for key in physical_index.keys() {
            check_cancel(cancel)?;
            insert_physical.execute([key]).map_err(db_error)?;
        }
    }
    tx.commit().map_err(db_error)
}

struct ExportPageIndex {
    keys: HashMap<String, usize>,
    cache_bases: HashMap<String, usize>,
    active_cache_entries: HashSet<usize>,
}

fn prepare_export_page_index(
    data_dir: &Path,
    entries: &[EnumeratedEntry],
    physical_index: &HashMap<String, usize>,
) -> Result<ExportPageIndex, TransferError> {
    let mut page_index = ExportPageIndex {
        keys: physical_index.clone(),
        cache_bases: HashMap::new(),
        active_cache_entries: HashSet::new(),
    };
    let has_convertible_pages = entries.iter().any(|entry| {
        matches!(
            entry.portable.media_kind,
            PortableMediaKind::Zip | PortableMediaKind::ConvertibleArchive
        )
    });
    if !has_convertible_pages {
        return Ok(page_index);
    }
    for (entry_index, entry) in entries.iter().enumerate() {
        if !matches!(
            entry.portable.media_kind,
            PortableMediaKind::Zip | PortableMediaKind::ConvertibleArchive
        ) {
            continue;
        }
        let cache_base = crate::path_key::normalize_keep_drive(
            &crate::archive_cache::cache_zip_path_for_data_dir(data_dir, &entry.path),
        );
        if let Some(previous) = page_index.keys.insert(cache_base.clone(), entry_index)
            && previous != entry_index
        {
            return Err(TransferError::Database(format!(
                "変換cache keyが別項目と重複しています: {}",
                entry.path.display()
            )));
        }
        page_index.cache_bases.insert(cache_base, entry_index);
    }
    let db_path = data_dir.join("archive_cache.db");
    match db_path.try_exists() {
        Ok(false) => return Ok(page_index),
        Ok(true) => {}
        Err(error) => {
            return Err(TransferError::Io(format!(
                "{}を確認できません: {error}",
                db_path.display()
            )));
        }
    }
    let connection = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        TransferError::Database(format!("{}を読み取れません: {error}", db_path.display()))
    })?;
    for (entry_index, entry) in entries.iter().enumerate() {
        if !matches!(
            entry.portable.media_kind,
            PortableMediaKind::Zip | PortableMediaKind::ConvertibleArchive
        ) {
            continue;
        }
        let Some(fingerprint) = entry.portable.fingerprint.as_ref() else {
            continue;
        };
        let source_key = crate::path_key::normalize(&entry.path);
        let row = match connection.query_row(
            "SELECT src_mtime, src_size, cached_zip_path
                   FROM converted_archives
                  WHERE src_path_key = ?1",
            [&source_key],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        ) {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => continue,
            Err(error) => {
                return Err(TransferError::Database(format!(
                    "archive_cache.db: {}: {error}",
                    entry.path.display()
                )));
            }
        };
        let (source_mtime, source_size, cached_path) = row;
        let fingerprint_mtime = fingerprint.modified_ms.map(|value| value.div_euclid(1000));
        let cached_path = PathBuf::from(cached_path);
        let expected_cached_path =
            crate::archive_cache::cache_zip_path_for_data_dir(data_dir, &entry.path);
        if source_size < 0
            || source_size as u64 != fingerprint.size
            || fingerprint_mtime != Some(source_mtime)
            || crate::path_key::normalize_keep_drive(&cached_path)
                != crate::path_key::normalize_keep_drive(&expected_cached_path)
            || !expected_cached_path.is_file()
        {
            continue;
        }
        page_index.active_cache_entries.insert(entry_index);
    }
    Ok(page_index)
}

fn locate_export_page_key(
    key: &str,
    page_index: &ExportPageIndex,
    page_origins: &mut [Option<PortableVirtualKeyBase>],
    entries: &[EnumeratedEntry],
) -> Result<Option<(usize, Option<String>)>, TransferError> {
    let Some((entry_index, member)) = locate_item_key(key, &page_index.keys) else {
        return Ok(None);
    };
    if member.is_some() {
        let base = key.split_once("::").map(|(base, _)| base).unwrap_or(key);
        let origin = if page_index.cache_bases.get(base) == Some(&entry_index) {
            PortableVirtualKeyBase::ConvertedCache
        } else {
            PortableVirtualKeyBase::Source
        };
        if let Some(previous) = page_origins[entry_index]
            && previous != origin
        {
            return Err(TransferError::Database(format!(
                "変換ページのメタ情報がsource keyとcache keyの両方にあります: {}。同じアーカイブを直接閲覧と変換キャッシュの両方で編集した可能性があります",
                entries[entry_index].path.display()
            )));
        }
        page_origins[entry_index] = Some(origin);
    }
    Ok(Some((entry_index, member)))
}

fn finalize_export_page_bases(
    entries: &mut [EnumeratedEntry],
    page_index: &ExportPageIndex,
    page_origins: &[Option<PortableVirtualKeyBase>],
) {
    for (entry_index, entry) in entries.iter_mut().enumerate() {
        if !matches!(
            entry.portable.media_kind,
            PortableMediaKind::Zip | PortableMediaKind::ConvertibleArchive
        ) {
            continue;
        }
        entry.portable.virtual_key_base = match page_origins[entry_index] {
            // 実際にDBで見つかったoriginは、拡張子やcache管理行からの推測より強い。
            // 直接閲覧RAR/CBRはConvertibleArchiveだがsource keyを参照する。
            Some(origin) => origin,
            None if page_index.active_cache_entries.contains(&entry_index)
                || entry.portable.media_kind == PortableMediaKind::ConvertibleArchive =>
            {
                PortableVirtualKeyBase::ConvertedCache
            }
            None => PortableVirtualKeyBase::Source,
        };
    }
}

fn attach_metadata<F>(
    data_dir: &Path,
    entries: &mut [EnumeratedEntry],
    cancel: &AtomicBool,
    progress: &mut F,
) -> Result<(), TransferError>
where
    F: FnMut(TransferProgress),
{
    let index: HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (crate::path_key::normalize_keep_drive(&entry.path), index))
        .collect();
    let page_index = prepare_export_page_index(data_dir, entries, &index)?;
    let mut page_origins = vec![None; entries.len()];
    let mut virtual_index: HashMap<(usize, String), usize> = HashMap::new();
    let mut metadata_rows = 0usize;

    let rating_path = data_dir.join("rating.db");
    if rating_path.is_file() {
        let mut conn = open_readonly(&rating_path)?;
        prepare_metadata_scope(&mut conn, &index, &page_index.keys, cancel)?;
        let mut stmt = conn
            .prepare(
                "SELECT r.path, r.stars, r.rated_at_ms, r.kind, r.entry_name, r.page_num,
                        r.dir_prefix, r.archive_format, r.zipdir_is_archive,
                        r.zipdir_representative
                   FROM metadata_transfer_physical_scope AS s
                  CROSS JOIN ratings AS r
                  WHERE r.path = s.item_key
                 UNION ALL
                 SELECT r.path, r.stars, r.rated_at_ms, r.kind, r.entry_name, r.page_num,
                        r.dir_prefix, r.archive_format, r.zipdir_is_archive,
                        r.zipdir_representative
                   FROM metadata_transfer_scope AS s
                  CROSS JOIN ratings AS r
                  WHERE r.path >= s.virtual_lower AND r.path < s.virtual_upper",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                let key: String = row.get(0)?;
                let stars: i64 = row.get(1)?;
                let kind: Option<i64> = row.get(3)?;
                if !(1..=5).contains(&stars) {
                    return Err(invalid_db_value(
                        1,
                        rusqlite::types::Type::Integer,
                        format!("rating.db: {key}: stars={stars}"),
                    ));
                }
                let kind = kind
                    .map(|value| {
                        rating_kind_name(value).ok_or_else(|| {
                            invalid_db_value(
                                3,
                                rusqlite::types::Type::Integer,
                                format!("rating.db: {key}: kind={value}"),
                            )
                        })
                    })
                    .transpose()?
                    .map(str::to_string);
                let page_num = row
                    .get::<_, Option<i64>>(5)?
                    .map(|value| {
                        u32::try_from(value).map_err(|_| {
                            invalid_db_value(
                                5,
                                rusqlite::types::Type::Integer,
                                format!("rating.db: {key}: page_num={value}"),
                            )
                        })
                    })
                    .transpose()?;
                Ok((
                    key,
                    PortableRating {
                        stars: stars as u8,
                        rated_at_ms: row.get(2)?,
                        kind,
                        entry_name: row.get(4)?,
                        page_num,
                        dir_prefix: row.get(6)?,
                        archive_format: row.get(7)?,
                        zipdir_is_archive: row.get::<_, Option<i64>>(8)?.map(|value| value != 0),
                        zipdir_representative: row.get(9)?,
                    },
                ))
            })
            .map_err(db_error)?;
        for row in rows {
            check_cancel(cancel)?;
            let (key, rating) = row.map_err(db_error)?;
            report_metadata_progress(&mut metadata_rows, &key, progress);
            let located = locate_export_page_key(&key, &page_index, &mut page_origins, entries)?
                .or_else(|| {
                    index
                        .get(&key)
                        .copied()
                        .map(|entry_index| (entry_index, None))
                });
            if let Some((entry_index, member)) = located {
                if let Some(member) = member {
                    let virtual_item =
                        get_virtual_item(entries, &mut virtual_index, entry_index, member);
                    virtual_item.rating = Some(rating);
                } else {
                    entries[entry_index].portable.rating = Some(rating);
                }
            }
        }
    }

    let tags_path = data_dir.join("tags.db");
    if tags_path.is_file() {
        let mut conn = open_readonly(&tags_path)?;
        prepare_metadata_scope(&mut conn, &index, &page_index.keys, cancel)?;
        let mut tags_by_key: HashMap<String, Vec<PortableTag>> = HashMap::new();
        {
            let mut stmt = conn
                .prepare(
                    "SELECT t.item_key, t.tag, t.applied_at, t.tag_key
                       FROM metadata_transfer_physical_scope AS s
                      CROSS JOIN item_tags AS t
                      WHERE t.item_key = s.item_key
                     UNION ALL
                     SELECT t.item_key, t.tag, t.applied_at, t.tag_key
                       FROM metadata_transfer_scope AS s
                      CROSS JOIN item_tags AS t
                      WHERE t.item_key >= s.virtual_lower
                        AND t.item_key < s.virtual_upper
                      ORDER BY 3, 4",
                )
                .map_err(db_error)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        PortableTag {
                            name: row.get(1)?,
                            applied_at: row.get(2)?,
                        },
                    ))
                })
                .map_err(db_error)?;
            for row in rows {
                check_cancel(cancel)?;
                let (key, tag) = row.map_err(db_error)?;
                report_metadata_progress(&mut metadata_rows, &key, progress);
                tags_by_key.entry(key).or_default().push(tag);
            }
        }
        let mut decided = HashSet::new();
        {
            let mut stmt = conn
                .prepare(
                    "SELECT t.item_key
                       FROM metadata_transfer_physical_scope AS s
                      CROSS JOIN tag_item_state AS t
                      WHERE t.item_key = s.item_key
                     UNION ALL
                     SELECT t.item_key
                       FROM metadata_transfer_scope AS s
                      CROSS JOIN tag_item_state AS t
                      WHERE t.item_key >= s.virtual_lower
                        AND t.item_key < s.virtual_upper",
                )
                .map_err(db_error)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(db_error)?;
            for row in rows {
                check_cancel(cancel)?;
                let key = row.map_err(db_error)?;
                report_metadata_progress(&mut metadata_rows, &key, progress);
                decided.insert(key);
            }
        }
        decided.extend(tags_by_key.keys().cloned());
        for key in decided {
            check_cancel(cancel)?;
            let tags = tags_by_key.remove(&key).unwrap_or_default();
            let located = locate_export_page_key(&key, &page_index, &mut page_origins, entries)?
                .or_else(|| {
                    index
                        .get(&key)
                        .copied()
                        .map(|entry_index| (entry_index, None))
                });
            if let Some((entry_index, member)) = located {
                if let Some(member) = member {
                    let item = get_virtual_item(entries, &mut virtual_index, entry_index, member);
                    item.tags = tags;
                    item.tags_decided = true;
                } else {
                    entries[entry_index].portable.tags = tags;
                    entries[entry_index].portable.tags_decided = true;
                }
            }
        }
    }

    let video_path = data_dir.join("video_bookmarks.db");
    if video_path.is_file() {
        let mut conn = open_readonly(&video_path)?;
        prepare_metadata_scope(&mut conn, &index, &index, cancel)?;
        let mut stmt = conn
            .prepare(
                "SELECT v.path, v.pts_secs, v.title, v.thumb_webp, v.created_at
                   FROM metadata_transfer_physical_scope AS s
                  CROSS JOIN video_bookmarks AS v
                  WHERE v.path = s.item_key
                  ORDER BY v.path, v.pts_secs, v.id",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                let thumb_webp_base64 = row
                    .get::<_, Option<Vec<u8>>>(3)?
                    .filter(|bytes| !bytes.is_empty())
                    .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes));
                let created_at: i64 = row.get(4)?;
                Ok((
                    row.get::<_, String>(0)?,
                    PortableTimedBookmark {
                        pts_secs: row.get(1)?,
                        title: row
                            .get::<_, Option<String>>(2)?
                            .filter(|value| !value.is_empty()),
                        thumb_webp_base64,
                        created_at_ms: created_at.saturating_mul(1000),
                    },
                ))
            })
            .map_err(db_error)?;
        for row in rows {
            check_cancel(cancel)?;
            let (key, bookmark) = row.map_err(db_error)?;
            report_metadata_progress(&mut metadata_rows, &key, progress);
            if let Some(&entry_index) = index.get(&key) {
                entries[entry_index].portable.timed_bookmarks.push(bookmark);
            }
        }
    }

    let book_path = data_dir.join("book_bookmarks.db");
    if book_path.is_file() {
        let mut conn = open_readonly(&book_path)?;
        prepare_metadata_scope(&mut conn, &index, &index, cancel)?;
        let mut stmt = conn
            .prepare(
                "SELECT b.container_key, b.container_kind, b.page_kind, b.page_value,
                        b.page_index_hint, b.created_at_ms, b.title
                   FROM metadata_transfer_physical_scope AS s
                  CROSS JOIN book_bookmarks AS b
                  WHERE b.container_key = s.item_key
                  ORDER BY b.container_key, b.page_index_hint, b.id",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                let key: String = row.get(0)?;
                let raw_page_index: i64 = row.get(4)?;
                let page_index_hint = usize::try_from(raw_page_index).map_err(|_| {
                    invalid_db_value(
                        4,
                        rusqlite::types::Type::Integer,
                        format!("book_bookmarks.db: {key}: page_index_hint={raw_page_index}"),
                    )
                })?;
                Ok((
                    key,
                    PortableBookBookmark {
                        container_kind: row.get(1)?,
                        page_kind: row.get(2)?,
                        page_value: row.get(3)?,
                        page_index_hint,
                        created_at_ms: row.get(5)?,
                        title: row
                            .get::<_, Option<String>>(6)?
                            .filter(|value| !value.is_empty()),
                    },
                ))
            })
            .map_err(db_error)?;
        for row in rows {
            check_cancel(cancel)?;
            let (key, bookmark) = row.map_err(db_error)?;
            report_metadata_progress(&mut metadata_rows, &key, progress);
            if let Some(&entry_index) = index.get(&key) {
                entries[entry_index].portable.book_bookmarks.push(bookmark);
            }
        }
    }

    attach_extended_metadata(
        data_dir,
        entries,
        &index,
        &page_index,
        &mut page_origins,
        &mut virtual_index,
        cancel,
        &mut metadata_rows,
        progress,
    )?;

    finalize_export_page_bases(entries, &page_index, &page_origins);
    for entry in entries {
        entry
            .portable
            .virtual_items
            .sort_by(|a, b| a.member_key.cmp(&b.member_key));
        entry
            .portable
            .nested_containers
            .sort_by(|a, b| a.member_key.cmp(&b.member_key));
    }
    progress(TransferProgress {
        phase: TransferPhase::ReadingMetadata,
        processed: metadata_rows,
        total: 0,
        current_path: None,
    });
    Ok(())
}

fn page_state_for_key_mut<'a>(
    entries: &'a mut [EnumeratedEntry],
    virtual_index: &mut HashMap<(usize, String), usize>,
    physical_index: &HashMap<String, usize>,
    key: &str,
) -> Option<&'a mut PortablePageState> {
    let (entry_index, member) = locate_item_key(key, physical_index)?;
    Some(if let Some(member) = member {
        &mut get_virtual_item(entries, virtual_index, entry_index, member).page_state
    } else {
        &mut entries[entry_index].portable.page_state
    })
}

fn get_nested_container<'a>(
    entries: &'a mut [EnumeratedEntry],
    index: &mut HashMap<(usize, String), usize>,
    entry_index: usize,
    member: String,
) -> &'a mut PortableContainerState {
    let key = (entry_index, member.clone());
    let container_index = if let Some(&index) = index.get(&key) {
        index
    } else {
        let new_index = entries[entry_index].portable.nested_containers.len();
        entries[entry_index]
            .portable
            .nested_containers
            .push(PortableNestedContainer {
                member_key: member,
                state: PortableContainerState::default(),
            });
        index.insert(key, new_index);
        new_index
    };
    &mut entries[entry_index].portable.nested_containers[container_index].state
}

fn locate_container_key(
    key: &str,
    physical_index: &HashMap<String, usize>,
    entries: &[EnumeratedEntry],
) -> Option<(usize, Option<String>)> {
    if let Some(&index) = physical_index.get(key) {
        return Some((index, None));
    }
    let mut end = key.len();
    while let Some(offset) = key[..end].rfind('/') {
        let base = &key[..offset];
        if let Some(&index) = physical_index.get(base) {
            if entries[index].portable.kind == PortableEntryKind::File {
                let member = &key[offset + 1..];
                if !member.is_empty() {
                    return Some((index, Some(member.to_string())));
                }
            }
        }
        end = offset;
    }
    None
}

fn prepare_container_scope(
    conn: &mut Connection,
    entries: &[EnumeratedEntry],
    physical_index: &HashMap<String, usize>,
    cancel: &AtomicBool,
) -> Result<(), TransferError> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS temp.metadata_transfer_container_scope;
         CREATE TEMP TABLE metadata_transfer_container_scope (
             item_key TEXT PRIMARY KEY,
             nested_lower TEXT NOT NULL,
             nested_upper TEXT NOT NULL,
             include_nested INTEGER NOT NULL
         ) WITHOUT ROWID;",
    )
    .map_err(db_error)?;
    let tx = conn.transaction().map_err(db_error)?;
    {
        let mut insert = tx
            .prepare(
                "INSERT INTO metadata_transfer_container_scope
                    (item_key, nested_lower, nested_upper, include_nested)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(db_error)?;
        for (key, &entry_index) in physical_index {
            check_cancel(cancel)?;
            insert
                .execute(params![
                    key,
                    format!("{key}/"),
                    format!("{key}0"),
                    i64::from(entries[entry_index].portable.kind == PortableEntryKind::File),
                ])
                .map_err(db_error)?;
        }
    }
    tx.commit().map_err(db_error)
}

struct ExportContainerIndex {
    keys: HashMap<String, usize>,
    cache_bases: HashMap<String, usize>,
}

fn prepare_export_container_index(
    data_dir: &Path,
    entries: &[EnumeratedEntry],
    source_index: &HashMap<String, usize>,
) -> Result<ExportContainerIndex, TransferError> {
    let mut container_index = ExportContainerIndex {
        keys: source_index.clone(),
        cache_bases: HashMap::new(),
    };
    for (entry_index, entry) in entries.iter().enumerate() {
        if !matches!(
            entry.portable.media_kind,
            PortableMediaKind::Zip | PortableMediaKind::ConvertibleArchive
        ) {
            continue;
        }
        let cache_base = crate::path_key::normalize(
            &crate::archive_cache::cache_zip_path_for_data_dir(data_dir, &entry.path),
        );
        if let Some(previous) = container_index.keys.insert(cache_base.clone(), entry_index)
            && previous != entry_index
        {
            return Err(TransferError::Database(format!(
                "変換cache keyが別項目と重複しています: {}",
                entry.path.display()
            )));
        }
        container_index.cache_bases.insert(cache_base, entry_index);
    }
    Ok(container_index)
}

fn locate_export_container_key(
    key: &str,
    container_index: &ExportContainerIndex,
    container_origins: &mut [Option<PortableVirtualKeyBase>],
    entries: &[EnumeratedEntry],
) -> Result<Option<(usize, Option<String>)>, TransferError> {
    let located = if let Some(&entry_index) = container_index.keys.get(key) {
        Some((entry_index, None, key))
    } else {
        let mut located = None;
        let mut end = key.len();
        while let Some(offset) = key[..end].rfind('/') {
            let base = &key[..offset];
            if let Some(&entry_index) = container_index.keys.get(base)
                && entries[entry_index].portable.kind == PortableEntryKind::File
            {
                let member = &key[offset + 1..];
                if !member.is_empty() {
                    located = Some((entry_index, Some(member.to_string()), base));
                    break;
                }
            }
            end = offset;
        }
        located
    };
    let Some((entry_index, member, base)) = located else {
        return Ok(None);
    };
    let origin = if container_index.cache_bases.get(base) == Some(&entry_index) {
        PortableVirtualKeyBase::ConvertedCache
    } else {
        PortableVirtualKeyBase::Source
    };
    if let Some(previous) = container_origins[entry_index]
        && previous != origin
    {
        return Err(TransferError::Database(format!(
            "変換コンテナのメタ情報がsource keyとcache keyの両方にあります: {}。同じアーカイブを直接閲覧と変換キャッシュの両方で編集した可能性があります",
            entries[entry_index].path.display()
        )));
    }
    container_origins[entry_index] = Some(origin);
    Ok(Some((entry_index, member)))
}

fn attach_extended_metadata<F>(
    data_dir: &Path,
    entries: &mut [EnumeratedEntry],
    physical_index: &HashMap<String, usize>,
    page_index: &ExportPageIndex,
    page_origins: &mut [Option<PortableVirtualKeyBase>],
    virtual_index: &mut HashMap<(usize, String), usize>,
    cancel: &AtomicBool,
    metadata_rows: &mut usize,
    progress: &mut F,
) -> Result<(), TransferError>
where
    F: FnMut(TransferProgress),
{
    let mut attach_page_rows = |db_name: &str,
                                sql: &str,
                                mut apply: Box<
        dyn FnMut(&rusqlite::Row<'_>, &str, &mut PortablePageState) -> rusqlite::Result<()>,
    >|
     -> Result<(), TransferError> {
        let path = data_dir.join(db_name);
        if !path.is_file() {
            return Ok(());
        }
        let mut conn = open_readonly(&path)?;
        prepare_metadata_scope(&mut conn, physical_index, &page_index.keys, cancel)?;
        let mut stmt = conn.prepare(sql).map_err(db_error)?;
        let mut rows = stmt.query([]).map_err(db_error)?;
        while let Some(row) = rows.next().map_err(db_error)? {
            check_cancel(cancel)?;
            let key: String = row.get(0).map_err(db_error)?;
            report_metadata_progress(metadata_rows, &key, progress);
            locate_export_page_key(&key, page_index, page_origins, entries)?;
            if let Some(state) =
                page_state_for_key_mut(entries, virtual_index, &page_index.keys, &key)
            {
                apply(row, &key, state).map_err(db_error)?;
            }
        }
        Ok(())
    };

    attach_page_rows(
        "rotation.db",
        "SELECT r.path, r.angle
           FROM metadata_transfer_scope AS s CROSS JOIN rotations AS r
          WHERE r.path = s.item_key
         UNION ALL
         SELECT r.path, r.angle
           FROM metadata_transfer_scope AS s CROSS JOIN rotations AS r
          WHERE r.path >= s.virtual_lower AND r.path < s.virtual_upper",
        Box::new(|row, key, state| {
            let angle = row.get::<_, i32>(1)?;
            if !matches!(angle, 90 | 180 | 270) {
                return Err(invalid_db_value(
                    1,
                    rusqlite::types::Type::Integer,
                    format!("rotation.db: {key}: angle={angle}"),
                ));
            }
            state.rotation_degrees = Some(angle);
            Ok(())
        }),
    )?;
    attach_page_rows(
        "adjustment.db",
        "SELECT p.page_path, p.params_json
           FROM metadata_transfer_scope AS s CROSS JOIN page_params AS p
          WHERE p.page_path = s.item_key
         UNION ALL
         SELECT p.page_path, p.params_json
           FROM metadata_transfer_scope AS s CROSS JOIN page_params AS p
          WHERE p.page_path >= s.virtual_lower AND p.page_path < s.virtual_upper",
        Box::new(|row, _key, state| {
            let json: String = row.get(1)?;
            state.adjustment = Some(parse_db_json(&json, 1)?);
            Ok(())
        }),
    )?;
    attach_page_rows(
        "mask.db",
        "SELECT m.path, m.mask_data, m.width, m.height, m.vectors
           FROM metadata_transfer_scope AS s CROSS JOIN masks AS m
          WHERE m.path = s.item_key
         UNION ALL
         SELECT m.path, m.mask_data, m.width, m.height, m.vectors
           FROM metadata_transfer_scope AS s CROSS JOIN masks AS m
          WHERE m.path >= s.virtual_lower AND m.path < s.virtual_upper",
        Box::new(|row, key, state| {
            let raw: Vec<u8> = row.get(1)?;
            let width: i64 = row.get(2)?;
            let height: i64 = row.get(3)?;
            let vectors: Option<String> = row.get(4)?;
            let width = u32::try_from(width)
                .ok()
                .filter(|value| *value != 0)
                .ok_or_else(|| {
                    invalid_db_value(
                        2,
                        rusqlite::types::Type::Integer,
                        format!("mask.db: {key}: width={width}"),
                    )
                })?;
            let height = u32::try_from(height)
                .ok()
                .filter(|value| *value != 0)
                .ok_or_else(|| {
                    invalid_db_value(
                        3,
                        rusqlite::types::Type::Integer,
                        format!("mask.db: {key}: height={height}"),
                    )
                })?;
            let shapes = vectors
                .as_deref()
                .map(crate::mask_db::try_shapes_from_json)
                .transpose()
                .map_err(|error| {
                    invalid_db_value(
                        4,
                        rusqlite::types::Type::Text,
                        format!("mask.db: {key}: vectors JSON: {error}"),
                    )
                })?
                .unwrap_or_default();
            state.mask = Some(crate::sidecar::SidecarMask::from_raw(
                &raw, &shapes, width, height,
            ));
            Ok(())
        }),
    )?;
    attach_page_rows(
        "conceal.db",
        "SELECT c.page_path, c.bitmap_data, c.bitmap_w, c.bitmap_h, c.shapes
           FROM metadata_transfer_scope AS s CROSS JOIN conceal_entries AS c
          WHERE c.page_path = s.item_key
         UNION ALL
         SELECT c.page_path, c.bitmap_data, c.bitmap_w, c.bitmap_h, c.shapes
           FROM metadata_transfer_scope AS s CROSS JOIN conceal_entries AS c
          WHERE c.page_path >= s.virtual_lower AND c.page_path < s.virtual_upper",
        Box::new(|row, key, state| {
            let raw: Vec<u8> = row.get(1)?;
            let width: i64 = row.get(2)?;
            let height: i64 = row.get(3)?;
            let vectors: Option<String> = row.get(4)?;
            let width = u32::try_from(width)
                .ok()
                .filter(|value| *value != 0)
                .ok_or_else(|| {
                    invalid_db_value(
                        2,
                        rusqlite::types::Type::Integer,
                        format!("conceal.db: {key}: width={width}"),
                    )
                })?;
            let height = u32::try_from(height)
                .ok()
                .filter(|value| *value != 0)
                .ok_or_else(|| {
                    invalid_db_value(
                        3,
                        rusqlite::types::Type::Integer,
                        format!("conceal.db: {key}: height={height}"),
                    )
                })?;
            let shapes = vectors
                .as_deref()
                .map(crate::mask_db::try_shapes_from_json)
                .transpose()
                .map_err(|error| {
                    invalid_db_value(
                        4,
                        rusqlite::types::Type::Text,
                        format!("conceal.db: {key}: shapes JSON: {error}"),
                    )
                })?
                .unwrap_or_default();
            state.conceal = Some(crate::sidecar::SidecarMask::from_raw(
                &raw, &shapes, width, height,
            ));
            Ok(())
        }),
    )?;
    attach_page_rows(
        "local_adjust.db",
        "SELECT p.page_path, p.layers_json
           FROM metadata_transfer_scope AS s CROSS JOIN local_adjust_pages AS p
          WHERE p.page_path = s.item_key
         UNION ALL
         SELECT p.page_path, p.layers_json
           FROM metadata_transfer_scope AS s CROSS JOIN local_adjust_pages AS p
          WHERE p.page_path >= s.virtual_lower AND p.page_path < s.virtual_upper",
        Box::new(|row, _key, state| {
            let json: String = row.get(1)?;
            state.local_adjust_layers = Some(parse_db_json(&json, 1)?);
            Ok(())
        }),
    )?;
    attach_page_rows(
        "export_crop.db",
        "SELECT p.page_path, p.min_x, p.min_y, p.max_x, p.max_y, p.aspect_mode
           FROM metadata_transfer_scope AS s CROSS JOIN export_crop_pages AS p
          WHERE p.page_path = s.item_key
         UNION ALL
         SELECT p.page_path, p.min_x, p.min_y, p.max_x, p.max_y, p.aspect_mode
           FROM metadata_transfer_scope AS s CROSS JOIN export_crop_pages AS p
          WHERE p.page_path >= s.virtual_lower AND p.page_path < s.virtual_upper",
        Box::new(|row, _key, state| {
            let aspect: String = row.get(5)?;
            state.export_crop = Some(crate::export_crop::CropSettings {
                rect: crate::export_crop::CropRect {
                    min_x: row.get(1)?,
                    min_y: row.get(2)?,
                    max_x: row.get(3)?,
                    max_y: row.get(4)?,
                },
                aspect_mode: crate::export_crop::CropAspectMode::from_stable_key(&aspect),
            });
            Ok(())
        }),
    )?;
    attach_page_rows(
        "comic.db",
        "SELECT c.page_path, c.doc_json
           FROM metadata_transfer_scope AS s CROSS JOIN comic_entries AS c
          WHERE c.page_path = s.item_key
         UNION ALL
         SELECT c.page_path, c.doc_json
           FROM metadata_transfer_scope AS s CROSS JOIN comic_entries AS c
          WHERE c.page_path >= s.virtual_lower AND c.page_path < s.virtual_upper",
        Box::new(|row, _key, state| {
            let json: String = row.get(1)?;
            state.comic = Some(parse_db_json(&json, 1)?);
            Ok(())
        }),
    )?;
    attach_page_rows(
        "view_trim.db",
        "SELECT p.page_path, p.override_json
           FROM metadata_transfer_scope AS s CROSS JOIN view_trim_pages AS p
          WHERE p.page_path = s.item_key
         UNION ALL
         SELECT p.page_path, p.override_json
           FROM metadata_transfer_scope AS s CROSS JOIN view_trim_pages AS p
          WHERE p.page_path >= s.virtual_lower AND p.page_path < s.virtual_upper",
        Box::new(|row, _key, state| {
            let json: String = row.get(1)?;
            state.view_trim = Some(parse_db_json(&json, 1)?);
            Ok(())
        }),
    )?;
    drop(attach_page_rows);

    let stripped_index: HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (crate::path_key::normalize(&entry.path), index))
        .collect();
    let container_index = prepare_export_container_index(data_dir, entries, &stripped_index)?;
    let mut container_origins = vec![None; entries.len()];
    let mut nested_index = HashMap::new();

    let spread_path = data_dir.join("spread.db");
    if spread_path.is_file() {
        let mut conn = open_readonly(&spread_path)?;
        prepare_container_scope(&mut conn, entries, &container_index.keys, cancel)?;
        let mut stmt = conn
            .prepare(
                "SELECT p.path, p.mode, p.flow, p.direction
                   FROM metadata_transfer_container_scope AS s CROSS JOIN spreads AS p
                  WHERE p.path = s.item_key
                     OR (s.include_nested != 0 AND p.path >= s.nested_lower AND p.path < s.nested_upper)",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    PortableSpreadState {
                        mode: row.get(1)?,
                        flow: row.get(2)?,
                        direction: row.get(3)?,
                    },
                ))
            })
            .map_err(db_error)?;
        for row in rows {
            check_cancel(cancel)?;
            let (key, spread) = row.map_err(db_error)?;
            report_metadata_progress(metadata_rows, &key, progress);
            if let Some((entry_index, member)) = locate_export_container_key(
                &key,
                &container_index,
                &mut container_origins,
                entries,
            )? {
                if let Some(member) = member {
                    get_nested_container(entries, &mut nested_index, entry_index, member).spread =
                        Some(spread);
                } else {
                    entries[entry_index].portable.container_state.spread = Some(spread);
                }
            }
        }
    }

    let view_trim_path = data_dir.join("view_trim.db");
    if view_trim_path.is_file() {
        let mut conn = open_readonly(&view_trim_path)?;
        prepare_container_scope(&mut conn, entries, &container_index.keys, cancel)?;
        let mut stmt = conn
            .prepare(
                "SELECT p.book_key, p.state_json
                   FROM metadata_transfer_container_scope AS s CROSS JOIN view_trim_books AS p
                  WHERE p.book_key = s.item_key
                     OR (s.include_nested != 0 AND p.book_key >= s.nested_lower AND p.book_key < s.nested_upper)",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(db_error)?;
        for row in rows {
            check_cancel(cancel)?;
            let (key, json) = row.map_err(db_error)?;
            report_metadata_progress(metadata_rows, &key, progress);
            let state = serde_json::from_str(&json).map_err(|error| {
                TransferError::Database(format!("view_trim.db state_json: {error}"))
            })?;
            if let Some((entry_index, member)) = locate_export_container_key(
                &key,
                &container_index,
                &mut container_origins,
                entries,
            )? {
                if let Some(member) = member {
                    get_nested_container(entries, &mut nested_index, entry_index, member)
                        .view_trim = Some(state);
                } else {
                    entries[entry_index].portable.container_state.view_trim = Some(state);
                }
            }
        }
    }

    for (entry_index, entry) in entries.iter_mut().enumerate() {
        entry.portable.container_key_base =
            container_origins[entry_index].unwrap_or(PortableVirtualKeyBase::Source);
    }

    let folder_pin_path = data_dir.join("folder_thumb_pins.db");
    if folder_pin_path.is_file() {
        let mut conn = open_readonly(&folder_pin_path)?;
        prepare_container_scope(&mut conn, entries, physical_index, cancel)?;
        let mut stmt = conn
            .prepare(
                "SELECT p.container_key, p.source_kind, p.source_rel, p.source_entry, p.source_page
                   FROM metadata_transfer_container_scope AS s
                  CROSS JOIN folder_thumb_pins AS p
                  WHERE p.container_key = s.item_key
                     OR (s.include_nested != 0 AND p.container_key >= s.nested_lower AND p.container_key < s.nested_upper)",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                let key: String = row.get(0)?;
                let raw_page = row.get::<_, Option<i64>>(4)?;
                let source_page = raw_page
                    .map(|value| {
                        u32::try_from(value).map_err(|_| {
                            invalid_db_value(
                                4,
                                rusqlite::types::Type::Integer,
                                format!("folder_thumb_pins.db: {key}: source_page={value}"),
                            )
                        })
                    })
                    .transpose()?;
                Ok((
                    key,
                    PortableFolderThumbPin {
                        source_kind: row.get(1)?,
                        source_rel: row.get(2)?,
                        source_entry: row.get(3)?,
                        source_page,
                    },
                ))
            })
            .map_err(db_error)?;
        for row in rows {
            check_cancel(cancel)?;
            let (key, pin) = row.map_err(db_error)?;
            report_metadata_progress(metadata_rows, &key, progress);
            if let Some((entry_index, member)) = locate_container_key(&key, physical_index, entries)
            {
                if let Some(member) = member {
                    get_nested_container(entries, &mut nested_index, entry_index, member)
                        .folder_thumb_pin = Some(pin);
                } else {
                    entries[entry_index]
                        .portable
                        .container_state
                        .folder_thumb_pin = Some(pin);
                }
            }
        }
    }

    let video_pin_path = data_dir.join("video_pins.db");
    if video_pin_path.is_file() {
        let mut conn = open_readonly(&video_pin_path)?;
        prepare_metadata_scope(&mut conn, physical_index, physical_index, cancel)?;
        let mut stmt = conn
            .prepare(
                "SELECT p.path, p.pin_pts_secs, p.thumb_webp, p.thumb_pts_secs
                   FROM metadata_transfer_physical_scope AS s CROSS JOIN video_pins AS p
                  WHERE p.path = s.item_key",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                let webp: Option<Vec<u8>> = row.get(2)?;
                Ok((
                    row.get::<_, String>(0)?,
                    PortableVideoPin {
                        pin_pts_secs: row.get(1)?,
                        thumb_webp_base64: webp
                            .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes)),
                        thumb_pts_secs: row.get(3)?,
                    },
                ))
            })
            .map_err(db_error)?;
        for row in rows {
            check_cancel(cancel)?;
            let (key, pin) = row.map_err(db_error)?;
            report_metadata_progress(metadata_rows, &key, progress);
            if let Some(&entry_index) = physical_index.get(&key) {
                entries[entry_index].portable.video_pin = Some(pin);
            }
        }
    }
    Ok(())
}

fn parse_db_json<T: serde::de::DeserializeOwned>(json: &str, column: usize) -> rusqlite::Result<T> {
    serde_json::from_str(json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn report_metadata_progress<F>(processed: &mut usize, key: &str, progress: &mut F)
where
    F: FnMut(TransferProgress),
{
    *processed += 1;
    if *processed == 1 || *processed % 128 == 0 {
        progress(TransferProgress {
            phase: TransferPhase::ReadingMetadata,
            processed: *processed,
            total: 0,
            current_path: Some(key.to_string()),
        });
    }
}

fn get_virtual_item<'a>(
    entries: &'a mut [EnumeratedEntry],
    index: &mut HashMap<(usize, String), usize>,
    entry_index: usize,
    member: String,
) -> &'a mut PortableVirtualItem {
    let key = (entry_index, member.clone());
    let virtual_index = if let Some(&index) = index.get(&key) {
        index
    } else {
        let new_index = entries[entry_index].portable.virtual_items.len();
        entries[entry_index]
            .portable
            .virtual_items
            .push(PortableVirtualItem {
                member_key: member,
                rating: None,
                tags: Vec::new(),
                tags_decided: false,
                page_state: PortablePageState::default(),
            });
        index.insert(key, new_index);
        new_index
    };
    &mut entries[entry_index].portable.virtual_items[virtual_index]
}

fn locate_item_key(
    key: &str,
    physical_index: &HashMap<String, usize>,
) -> Option<(usize, Option<String>)> {
    if let Some(&index) = physical_index.get(key) {
        return Some((index, None));
    }
    let (base, member) = key.split_once("::")?;
    physical_index
        .get(base)
        .copied()
        .map(|index| (index, Some(member.to_string())))
}

fn summarize_entry(summary: &mut ExportSummary, entry: &PortableEntry) {
    summary.entries += 1;
    summary.ratings += usize::from(entry.rating.is_some());
    summary.tagged_items += usize::from(!entry.tags.is_empty());
    summary.timed_bookmarks += entry.timed_bookmarks.len();
    summary.book_bookmarks += entry.book_bookmarks.len();
    summary.page_states += usize::from(!entry.page_state.is_empty());
    summary.container_states += usize::from(entry.container_state.has_view_state());
    summary.container_states += entry
        .nested_containers
        .iter()
        .filter(|container| container.state.has_view_state())
        .count();
    summary.thumbnail_pins += usize::from(entry.video_pin.is_some());
    summary.thumbnail_pins += usize::from(entry.container_state.folder_thumb_pin.is_some());
    summary.thumbnail_pins += entry
        .nested_containers
        .iter()
        .filter(|container| container.state.folder_thumb_pin.is_some())
        .count();
    for virtual_item in &entry.virtual_items {
        summary.ratings += usize::from(virtual_item.rating.is_some());
        summary.tagged_items += usize::from(!virtual_item.tags.is_empty());
        summary.page_states += usize::from(!virtual_item.page_state.is_empty());
    }
}

fn read_bundle_manifest(root: &Path, cancel: &AtomicBool) -> Result<BundleManifest, TransferError> {
    check_cancel(cancel)?;
    let bundle_dir = root.join(SIDECAR_FILENAME);
    ensure_plain_directory(&bundle_dir)?;
    let path = bundle_dir.join(BUNDLE_MANIFEST_FILENAME);
    ensure_plain_file(&path)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| TransferError::Io(format!("{}: {error}", path.display())))?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(TransferError::Invalid(format!(
            "manifestサイズが上限 {} KiB を超えています",
            MAX_MANIFEST_BYTES / 1024
        )));
    }
    let file = File::open(&path)
        .map_err(|error| TransferError::Io(format!("{}: {error}", path.display())))?;
    let reader = BufReader::new(file.take(MAX_MANIFEST_BYTES + 1));
    let reader = CancelReader {
        inner: reader,
        cancel,
    };
    let manifest: BundleManifest = serde_json::from_reader(reader).map_err(|error| {
        if cancel.load(Ordering::Relaxed) {
            TransferError::Cancelled
        } else {
            TransferError::Invalid(format!("JSON: {error}"))
        }
    })?;
    validate_bundle_manifest(&manifest)?;
    let generation_dir = bundle_generation_dir(&bundle_dir, &manifest.generation);
    ensure_plain_directory(&generation_dir)?;
    ensure_plain_directory(&generation_dir.join(SHARDS_DIRNAME))?;
    check_cancel(cancel)?;
    Ok(manifest)
}

fn visit_bundle_entries<F>(
    root: &Path,
    manifest: &BundleManifest,
    cancel: &AtomicBool,
    mut visit: F,
) -> Result<(), TransferError>
where
    F: FnMut(&PortableEntry, usize, usize) -> Result<(), TransferError>,
{
    let bundle_dir = root.join(SIDECAR_FILENAME);
    let shards_dir = bundle_generation_dir(&bundle_dir, &manifest.generation).join(SHARDS_DIRNAME);
    ensure_plain_directory(&shards_dir)?;

    let mut shard_count = 0_u64;
    let mut entry_count = 0_u64;
    let mut saw_root_shard = false;
    let mut duplicate_guard = DiskDuplicateGuard::new()?;
    let shards = fs::read_dir(&shards_dir)
        .map_err(|error| TransferError::Io(format!("{}: {error}", shards_dir.display())))?;
    for shard in shards {
        check_cancel(cancel)?;
        let shard = shard.map_err(|error| TransferError::Io(error.to_string()))?;
        let path = shard.path();
        ensure_plain_file(&path)?;
        let name = shard.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| TransferError::Invalid("Unicodeでないshard名があります".into()))?;
        if !name.ends_with(&format!(".{SHARD_EXTENSION}")) {
            return Err(TransferError::Invalid(format!(
                "不明なshardファイルがあります: {name}"
            )));
        }

        let file = File::open(&path)
            .map_err(|error| TransferError::Io(format!("{}: {error}", path.display())))?;
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        if !read_bounded_line(
            &mut reader,
            &mut line,
            MAX_SHARD_HEADER_BYTES,
            cancel,
            &path,
        )? {
            return Err(TransferError::Invalid(format!(
                "shard headerがありません: {name}"
            )));
        }
        let header: ShardHeader = parse_json_record(&line, &path, 1)?;
        validate_shard_header(&header, manifest, name)?;
        saw_root_shard |= header.folder == ".";
        shard_count = shard_count
            .checked_add(1)
            .ok_or_else(|| TransferError::Invalid("shard数が表現範囲を超えています".into()))?;

        let mut line_number = 1_usize;
        while read_bounded_line(&mut reader, &mut line, MAX_RECORD_BYTES, cancel, &path)? {
            line_number += 1;
            let entry: PortableEntry = parse_json_record(&line, &path, line_number)?;
            validate_portable_entry(&entry)?;
            validate_shard_entry_path(&header.folder, &entry.path)?;
            let identity = entry.path.to_lowercase();
            if !duplicate_guard.insert(&identity)? {
                return Err(TransferError::Invalid(format!(
                    "bundle内のパスが重複しています: {}",
                    entry.path
                )));
            }
            let index = usize_from_count(entry_count, "項目数")?;
            visit(&entry, index, line.len())?;
            entry_count = entry_count
                .checked_add(1)
                .ok_or_else(|| TransferError::Invalid("項目数が表現範囲を超えています".into()))?;
        }
    }
    if !saw_root_shard {
        return Err(TransferError::Invalid(
            "root folderのshardがありません".into(),
        ));
    }
    if shard_count != manifest.shard_count || entry_count != manifest.entry_count {
        return Err(TransferError::Invalid(format!(
            "manifest件数とshard内容が一致しません（shard {shard_count}/{}, 項目 {entry_count}/{}）",
            manifest.shard_count, manifest.entry_count
        )));
    }
    Ok(())
}

/// 件数に比例するHashSetをRAMへ保持せず、OSの一時領域に置いたSQLite indexで
/// 完全な重複検査を行う。import pathとexport済みdirectoryの双方で使い、
/// cacheを4MiBへ制限して巨大な単一フォルダでも検証メモリを抑える。
struct DiskDuplicateGuard {
    conn: Connection,
    // Connectionを先にdropしてからWindows上の一時DBを削除するため、この順序を保つ。
    _temp_dir: tempfile::TempDir,
}

impl DiskDuplicateGuard {
    fn new() -> Result<Self, TransferError> {
        let temp_dir = tempfile::Builder::new()
            .prefix("miv-metadata-duplicates-")
            .tempdir()
            .map_err(|error| TransferError::Io(format!("重複検査用一時領域: {error}")))?;
        let path = temp_dir.path().join("seen.db");
        let conn = Connection::open(&path).map_err(db_error)?;
        conn.execute_batch(
            "PRAGMA journal_mode=OFF;
             PRAGMA synchronous=OFF;
             PRAGMA temp_store=FILE;
             PRAGMA cache_size=-4096;
             CREATE TABLE seen_paths (
                 path TEXT PRIMARY KEY
             ) WITHOUT ROWID;
             BEGIN;",
        )
        .map_err(db_error)?;
        Ok(Self {
            conn,
            _temp_dir: temp_dir,
        })
    }

    fn insert(&mut self, path: &str) -> Result<bool, TransferError> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO seen_paths(path) VALUES (?1)",
                params![path],
            )
            .map(|changed| changed == 1)
            .map_err(db_error)
    }
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    output: &mut Vec<u8>,
    max_bytes: u64,
    cancel: &AtomicBool,
    path: &Path,
) -> Result<bool, TransferError> {
    output.clear();
    loop {
        check_cancel(cancel)?;
        let available = reader
            .fill_buf()
            .map_err(|error| TransferError::Io(format!("{}: {error}", path.display())))?;
        if available.is_empty() {
            return Ok(!output.is_empty());
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |position| position + 1);
        let new_len = (output.len() as u64)
            .checked_add(take as u64)
            .ok_or_else(|| TransferError::Invalid("recordサイズが表現範囲を超えています".into()))?;
        if new_len > max_bytes {
            return Err(TransferError::Invalid(format!(
                "単一recordが上限 {} MiBを超えています: {}",
                max_bytes / 1024 / 1024,
                path.display()
            )));
        }
        output.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            while output
                .last()
                .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
            {
                output.pop();
            }
            if output.is_empty() {
                return Err(TransferError::Invalid(format!(
                    "空のJSON recordがあります: {}",
                    path.display()
                )));
            }
            return Ok(true);
        }
    }
}

fn parse_json_record<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    path: &Path,
    line: usize,
) -> Result<T, TransferError> {
    serde_json::from_slice(bytes).map_err(|error| {
        TransferError::Invalid(format!("{}:{line}: JSON: {error}", path.display()))
    })
}

fn validate_bundle_manifest(manifest: &BundleManifest) -> Result<(), TransferError> {
    if manifest.format != FORMAT_NAME {
        return Err(TransferError::Invalid(
            "形式識別子が一致しません".to_string(),
        ));
    }
    if manifest.version != FORMAT_VERSION {
        return Err(TransferError::Invalid(format!(
            "未対応のバージョンです: {}（v{}だけを受け入れます）",
            manifest.version, FORMAT_VERSION
        )));
    }
    validate_generation(&manifest.generation)?;
    if manifest.shard_count == 0 || manifest.entry_count == 0 {
        return Err(TransferError::Invalid(
            "shard数または項目数が0です".to_string(),
        ));
    }
    if !manifest.recursive && manifest.shard_count != 1 {
        return Err(TransferError::Invalid(
            "非再帰manifestにはroot shard以外を含められません".to_string(),
        ));
    }
    usize_from_count(manifest.entry_count, "項目数")?;
    Ok(())
}

fn validate_shard_header(
    header: &ShardHeader,
    manifest: &BundleManifest,
    filename: &str,
) -> Result<(), TransferError> {
    if header.format != SHARD_FORMAT_NAME
        || header.version != FORMAT_VERSION
        || header.generation != manifest.generation
    {
        return Err(TransferError::Invalid(format!(
            "shard headerがmanifestと一致しません: {filename}"
        )));
    }
    validate_relative_path(&header.folder)?;
    if filename != shard_filename(&header.folder) {
        return Err(TransferError::Invalid(format!(
            "folderとshard名が一致しません: {filename}"
        )));
    }
    Ok(())
}

fn validate_shard_entry_path(folder: &str, entry_path: &str) -> Result<(), TransferError> {
    let valid = if folder == "." {
        entry_path == "." || !entry_path.contains('/')
    } else {
        entry_path.rsplit_once('/').is_some_and(|(parent, child)| {
            !child.is_empty() && parent.to_lowercase() == folder.to_lowercase()
        })
    };
    if valid {
        Ok(())
    } else {
        Err(TransferError::Invalid(format!(
            "項目がfolder shardの直下ではありません: {folder} -> {entry_path}"
        )))
    }
}

fn usize_from_count(value: u64, label: &str) -> Result<usize, TransferError> {
    usize::try_from(value)
        .map_err(|_| TransferError::Invalid(format!("{label}がこの環境の表現範囲を超えています")))
}

#[cfg(test)]
fn validate_manifest(manifest: &Manifest) -> Result<(), TransferError> {
    if manifest.format != FORMAT_NAME {
        return Err(TransferError::Invalid(
            "形式識別子が一致しません".to_string(),
        ));
    }
    if manifest.version != FORMAT_VERSION {
        return Err(TransferError::Invalid(format!(
            "未対応のバージョンです: {}（v{}だけを受け入れます）",
            manifest.version, FORMAT_VERSION
        )));
    }
    let mut paths = HashSet::new();
    for entry in &manifest.entries {
        let normalized = entry.path.to_lowercase();
        if !paths.insert(normalized) {
            return Err(TransferError::Invalid(format!(
                "パスが重複しています: {}",
                entry.path
            )));
        }
        validate_portable_entry(entry)?;
    }
    Ok(())
}

fn validate_portable_entry(entry: &PortableEntry) -> Result<(), TransferError> {
    validate_relative_path(&entry.path)?;
    let kind_pair_is_valid = matches!(
        (entry.kind, entry.media_kind),
        (PortableEntryKind::Directory, PortableMediaKind::Directory)
            | (
                PortableEntryKind::File,
                PortableMediaKind::Image
                    | PortableMediaKind::Video
                    | PortableMediaKind::Audio
                    | PortableMediaKind::Zip
                    | PortableMediaKind::Pdf
                    | PortableMediaKind::ConvertibleArchive
                    | PortableMediaKind::OtherFile
            )
    );
    if !kind_pair_is_valid
        || (entry.virtual_key_base == PortableVirtualKeyBase::ConvertedCache
            && !matches!(
                entry.media_kind,
                PortableMediaKind::Zip | PortableMediaKind::ConvertibleArchive
            ))
        || (entry.container_key_base == PortableVirtualKeyBase::ConvertedCache
            && !matches!(
                entry.media_kind,
                PortableMediaKind::Zip | PortableMediaKind::ConvertibleArchive
            ))
    {
        return Err(TransferError::Invalid(format!(
            "ファイル種別が不正です: {}",
            entry.path
        )));
    }
    if entry.kind == PortableEntryKind::File && entry.fingerprint.is_none() {
        return Err(TransferError::Invalid(format!(
            "ファイルの照合情報がありません: {}",
            entry.path
        )));
    }
    if !entry.timed_bookmarks.is_empty()
        && !matches!(
            entry.media_kind,
            PortableMediaKind::Video | PortableMediaKind::Audio
        )
    {
        return Err(TransferError::Invalid(format!(
            "時刻ブックマークを保存できないファイル種別です: {}",
            entry.path
        )));
    }
    if entry.video_pin.is_some() && entry.media_kind != PortableMediaKind::Video {
        return Err(TransferError::Invalid(format!(
            "動画ピンを保存できないファイル種別です: {}",
            entry.path
        )));
    }
    if !entry.page_state.is_empty() && entry.media_kind != PortableMediaKind::Image {
        return Err(TransferError::Invalid(format!(
            "ページ編集情報を保存できないファイル種別です: {}",
            entry.path
        )));
    }
    let is_container = matches!(
        entry.media_kind,
        PortableMediaKind::Directory
            | PortableMediaKind::Zip
            | PortableMediaKind::Pdf
            | PortableMediaKind::ConvertibleArchive
    );
    if (!entry.container_state.is_empty() || !entry.book_bookmarks.is_empty()) && !is_container {
        return Err(TransferError::Invalid(format!(
            "本・コンテナ情報を保存できないファイル種別です: {}",
            entry.path
        )));
    }
    if !entry.virtual_items.is_empty()
        && !matches!(
            entry.media_kind,
            PortableMediaKind::Zip | PortableMediaKind::Pdf | PortableMediaKind::ConvertibleArchive
        )
    {
        return Err(TransferError::Invalid(format!(
            "仮想ページを保存できないファイル種別です: {}",
            entry.path
        )));
    }
    if !entry.nested_containers.is_empty()
        && !matches!(
            entry.media_kind,
            PortableMediaKind::Zip | PortableMediaKind::ConvertibleArchive
        )
    {
        return Err(TransferError::Invalid(format!(
            "入れ子コンテナを保存できないファイル種別です: {}",
            entry.path
        )));
    }
    validate_rating(entry.rating.as_ref())?;
    if let Some(kind) = entry
        .rating
        .as_ref()
        .and_then(|rating| rating.kind.as_deref())
    {
        let compatible = matches!(
            (entry.media_kind, kind),
            (PortableMediaKind::Directory, "folder")
                | (PortableMediaKind::Image, "image")
                | (PortableMediaKind::Video, "video")
                | (PortableMediaKind::Audio, "audio")
                | (PortableMediaKind::Zip, "zip_file")
                | (PortableMediaKind::Pdf, "pdf_file")
                | (PortableMediaKind::ConvertibleArchive, "convertible_archive")
        );
        if !compatible {
            return Err(TransferError::Invalid(format!(
                "評価種別とファイル種別が一致しません: {}",
                entry.path
            )));
        }
    }
    if !entry.tags_decided && !entry.tags.is_empty() {
        return Err(TransferError::Invalid(format!(
            "タグ未決定の項目にタグがあります: {}",
            entry.path
        )));
    }
    validate_tags(&entry.tags)?;
    if entry.timed_bookmarks.len() > MAX_BOOKMARKS_PER_ENTRY
        || entry.book_bookmarks.len() > MAX_BOOKMARKS_PER_ENTRY
        || entry.virtual_items.len() > MAX_BOOKMARKS_PER_ENTRY
        || entry.nested_containers.len() > MAX_BOOKMARKS_PER_ENTRY
    {
        return Err(TransferError::Invalid(format!(
            "1項目あたりのメタ情報が多すぎます: {}",
            entry.path
        )));
    }
    for bookmark in &entry.timed_bookmarks {
        if !bookmark.pts_secs.is_finite() || bookmark.pts_secs < 0.0 {
            return Err(TransferError::Invalid(format!(
                "ブックマーク位置が不正です: {}",
                entry.path
            )));
        }
        validate_title(bookmark.title.as_deref())?;
        validate_timed_bookmark_thumb(bookmark, &entry.path)?;
    }
    for bookmark in &entry.book_bookmarks {
        validate_book_bookmark(bookmark, entry.media_kind, &entry.path)?;
        validate_title(bookmark.title.as_deref())?;
    }
    validate_page_state(&entry.page_state, &entry.path)?;
    validate_container_state(&entry.container_state, &entry.path)?;
    validate_video_pin(entry.video_pin.as_ref(), &entry.path)?;
    let mut member_keys = HashSet::new();
    for item in &entry.virtual_items {
        let canonical_member = canonical_member_key(&item.member_key);
        if validate_member_key(&item.member_key).is_err()
            || !member_keys.insert(canonical_member.clone())
        {
            return Err(TransferError::Invalid(format!(
                "仮想項目キーが不正または重複しています: {}",
                entry.path
            )));
        }
        validate_rating(item.rating.as_ref())?;
        if entry.media_kind == PortableMediaKind::Pdf {
            let page_num = portable_pdf_page_num(&canonical_member).ok_or_else(|| {
                TransferError::Invalid(format!(
                    "PDF仮想ページキーが不正です: {} / {}",
                    entry.path, item.member_key
                ))
            })?;
            if let Some(rating) = &item.rating
                && (rating.kind.as_deref() != Some("pdf_page") || rating.page_num != Some(page_num))
            {
                return Err(TransferError::Invalid(format!(
                    "PDF仮想ページの評価情報がキーと一致しません: {} / {}",
                    entry.path, item.member_key
                )));
            }
        }
        if let Some(kind) = item
            .rating
            .as_ref()
            .and_then(|rating| rating.kind.as_deref())
        {
            let compatible = matches!(
                (entry.media_kind, kind),
                (
                    PortableMediaKind::Zip | PortableMediaKind::ConvertibleArchive,
                    "zip_image" | "zip_dir",
                ) | (PortableMediaKind::Pdf, "pdf_page")
            );
            if !compatible {
                return Err(TransferError::Invalid(format!(
                    "仮想項目の評価種別が不正です: {}",
                    entry.path
                )));
            }
        }
        if !item.tags_decided && !item.tags.is_empty() {
            return Err(TransferError::Invalid(format!(
                "タグ未決定の仮想項目にタグがあります: {} / {}",
                entry.path, item.member_key
            )));
        }
        validate_tags(&item.tags)?;
        validate_page_state(&item.page_state, &entry.path)?;
    }
    let mut container_keys = HashSet::new();
    for container in &entry.nested_containers {
        if entry.kind != PortableEntryKind::File
            || validate_member_key(&container.member_key).is_err()
            || !container_keys.insert(canonical_member_key(&container.member_key))
        {
            return Err(TransferError::Invalid(format!(
                "仮想コンテナキーが不正または重複しています: {}",
                entry.path
            )));
        }
        validate_container_state(&container.state, &entry.path)?;
    }
    Ok(())
}

fn validate_member_key(value: &str) -> Result<(), TransferError> {
    validate_bookmark_page_path(value, "仮想項目")
}

fn portable_pdf_page_num(member_key: &str) -> Option<u32> {
    let value = member_key.strip_prefix("page_")?;
    let page_num = value.parse::<u32>().ok()?;
    (format!("page_{page_num}") == member_key).then_some(page_num)
}

fn validate_page_state(state: &PortablePageState, entry_path: &str) -> Result<(), TransferError> {
    if state
        .rotation_degrees
        .is_some_and(|degrees| !matches!(degrees, 90 | 180 | 270))
    {
        return Err(TransferError::Invalid(format!(
            "回転角が不正です: {entry_path}"
        )));
    }
    if let Some(mask) = &state.mask {
        validate_mask(mask, entry_path)?;
    }
    if let Some(mask) = &state.conceal {
        validate_mask(mask, entry_path)?;
    }
    if state
        .local_adjust_layers
        .as_ref()
        .is_some_and(|layers| layers.len() > MAX_BOOKMARKS_PER_ENTRY)
        || state
            .comic
            .as_ref()
            .is_some_and(|objects| objects.len() > MAX_BOOKMARKS_PER_ENTRY)
    {
        return Err(TransferError::Invalid(format!(
            "ページ編集情報が多すぎます: {entry_path}"
        )));
    }
    if let Some(crop) = state.export_crop {
        let rect = crop.rect;
        let values = [rect.min_x, rect.min_y, rect.max_x, rect.max_y];
        if values.iter().any(|value| !value.is_finite())
            || rect.min_x < 0.0
            || rect.min_y < 0.0
            || rect.min_x >= rect.max_x
            || rect.min_y >= rect.max_y
        {
            return Err(TransferError::Invalid(format!(
                "書き出しクロップが不正です: {entry_path}"
            )));
        }
    }
    if let Some(page_trim) = state.view_trim {
        validate_trim_margins(page_trim.margins, entry_path)?;
    }
    Ok(())
}

fn validate_mask(
    mask: &crate::sidecar::SidecarMask,
    entry_path: &str,
) -> Result<(), TransferError> {
    let pixels = u64::from(mask.w).saturating_mul(u64::from(mask.h));
    if mask.w == 0
        || mask.h == 0
        || pixels > MAX_MASK_PIXELS
        || mask.vectors.len() > MAX_BOOKMARKS_PER_ENTRY
    {
        return Err(TransferError::Invalid(format!(
            "マスク寸法または要素数が不正です: {entry_path}"
        )));
    }
    let decoded = mask.decode().ok_or_else(|| {
        TransferError::Invalid(format!("マスクの base64 が不正です: {entry_path}"))
    })?;
    if decoded.len() > MAX_MASK_BYTES {
        return Err(TransferError::Invalid(format!(
            "マスクデータが大きすぎます: {entry_path}"
        )));
    }
    let expected_bytes = usize::try_from((pixels + 7) / 8)
        .map_err(|_| TransferError::Invalid(format!("マスク寸法が不正です: {entry_path}")))?;
    let mut unpacked = Vec::with_capacity(expected_bytes.min(1024 * 1024));
    let mut decoder = flate2::read::DeflateDecoder::new(decoded.as_slice())
        .take(u64::try_from(expected_bytes).unwrap_or(u64::MAX) + 1);
    decoder
        .read_to_end(&mut unpacked)
        .map_err(|_| TransferError::Invalid(format!("マスク圧縮データが不正です: {entry_path}")))?;
    if unpacked.len() != expected_bytes {
        return Err(TransferError::Invalid(format!(
            "マスク圧縮データの長さが不正です: {entry_path}"
        )));
    }
    Ok(())
}

fn validate_trim_margins(
    margins: crate::view_trim::ViewTrimMargins,
    entry_path: &str,
) -> Result<(), TransferError> {
    let values = [margins.left, margins.top, margins.right, margins.bottom];
    if values.iter().any(|value| {
        !value.is_finite() || *value < 0.0 || *value > crate::view_trim::MAX_VIEW_TRIM_MARGIN
    }) {
        return Err(TransferError::Invalid(format!(
            "表示トリム値が不正です: {entry_path}"
        )));
    }
    Ok(())
}

fn validate_container_state(
    state: &PortableContainerState,
    entry_path: &str,
) -> Result<(), TransferError> {
    if let Some(spread) = state.spread {
        if !(0..=5).contains(&spread.mode)
            || !(0..=2).contains(&spread.flow)
            || !(0..=1).contains(&spread.direction)
        {
            return Err(TransferError::Invalid(format!(
                "見開き設定が不正です: {entry_path}"
            )));
        }
    }
    if let Some(trim) = state.view_trim {
        validate_trim_margins(trim.book_settings.single, entry_path)?;
        validate_trim_margins(trim.book_settings.spread_left, entry_path)?;
        validate_trim_margins(trim.book_settings.spread_right, entry_path)?;
        let linked = trim.book_settings.spread_linked;
        for value in [linked.top, linked.bottom, linked.inner, linked.outer] {
            if !value.is_finite() || value < 0.0 || value > crate::view_trim::MAX_VIEW_TRIM_MARGIN {
                return Err(TransferError::Invalid(format!(
                    "表示トリム値が不正です: {entry_path}"
                )));
            }
        }
    }
    if let Some(pin) = &state.folder_thumb_pin {
        for value in [Some(pin.source_rel.as_str()), pin.source_entry.as_deref()]
            .into_iter()
            .flatten()
        {
            if value.contains('\0') || value.chars().count() > MAX_MEMBER_KEY_CHARS {
                return Err(TransferError::Invalid(format!(
                    "代表サムネ設定が不正です: {entry_path}"
                )));
            }
        }
        if let Some(entry) = pin.source_entry.as_deref() {
            let entry = entry.trim_end_matches('/');
            if entry.is_empty() || validate_member_key(entry).is_err() {
                return Err(TransferError::Invalid(format!(
                    "代表サムネ設定が不正です: {entry_path}"
                )));
            }
        }
        let source = portable_folder_pin_source(pin).ok_or_else(|| {
            TransferError::Invalid(format!("代表サムネ設定が不正です: {entry_path}"))
        })?;
        crate::folder_thumb_pins::validate_source(&source).map_err(|error| {
            TransferError::Invalid(format!("代表サムネ設定が不正です: {entry_path}: {error}"))
        })?;
    }
    Ok(())
}

fn validate_timed_bookmark_thumb(
    bookmark: &PortableTimedBookmark,
    entry_path: &str,
) -> Result<(), TransferError> {
    let Some(encoded) = &bookmark.thumb_webp_base64 else {
        return Ok(());
    };
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.as_bytes())
        .map_err(|_| {
            TransferError::Invalid(format!(
                "時刻ブックマークの動画サムネの base64 が不正です: {entry_path}"
            ))
        })?;
    if decoded.len() > MAX_VIDEO_THUMB_BYTES {
        return Err(TransferError::Invalid(format!(
            "時刻ブックマークの動画サムネが大きすぎます: {entry_path}"
        )));
    }
    Ok(())
}

fn validate_video_pin(
    pin: Option<&PortableVideoPin>,
    entry_path: &str,
) -> Result<(), TransferError> {
    let Some(pin) = pin else {
        return Ok(());
    };
    if !pin.pin_pts_secs.is_finite()
        || pin.pin_pts_secs < 0.0
        || pin
            .thumb_pts_secs
            .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        return Err(TransferError::Invalid(format!(
            "動画ピン位置が不正です: {entry_path}"
        )));
    }
    if let Some(encoded) = &pin.thumb_webp_base64 {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.as_bytes())
            .map_err(|_| {
                TransferError::Invalid(format!("動画サムネの base64 が不正です: {entry_path}"))
            })?;
        if decoded.len() > MAX_VIDEO_THUMB_BYTES {
            return Err(TransferError::Invalid(format!(
                "動画サムネが大きすぎます: {entry_path}"
            )));
        }
    }
    Ok(())
}

fn portable_folder_pin_source(
    pin: &PortableFolderThumbPin,
) -> Option<crate::folder_thumb_pins::FolderPinSource> {
    use crate::folder_thumb_pins::{FileKind, FolderPinSource};
    match pin.source_kind.as_str() {
        "image" | "video" | "folder" | "zipfile" | "pdffile"
            if pin.source_entry.is_none() && pin.source_page.is_none() =>
        {
            Some(FolderPinSource::File {
                rel: pin.source_rel.clone(),
                kind: FileKind::from_db_str(&pin.source_kind)?,
            })
        }
        "zipentry" if pin.source_page.is_none() => Some(FolderPinSource::ZipEntry {
            zip_rel: pin.source_rel.clone(),
            entry: pin.source_entry.clone()?,
        }),
        "zipdir" if pin.source_page.is_none() => Some(FolderPinSource::ZipDir {
            zip_rel: pin.source_rel.clone(),
            dir_prefix: pin.source_entry.clone()?,
        }),
        "pdfpage" if pin.source_entry.is_none() => Some(FolderPinSource::PdfPage {
            pdf_rel: pin.source_rel.clone(),
            page: pin.source_page?,
        }),
        _ => None,
    }
}

fn validate_rating(rating: Option<&PortableRating>) -> Result<(), TransferError> {
    let Some(rating) = rating else {
        return Ok(());
    };
    if !(1..=5).contains(&rating.stars) {
        return Err(TransferError::Invalid(
            "評価値は1〜5である必要があります".to_string(),
        ));
    }
    if let Some(kind) = &rating.kind {
        if rating_kind_value(kind).is_none() {
            return Err(TransferError::Invalid(format!(
                "評価項目の種類が不正です: {kind}"
            )));
        }
    }
    for value in [
        rating.entry_name.as_deref(),
        rating.dir_prefix.as_deref(),
        rating.archive_format.as_deref(),
        rating.zipdir_representative.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if value.chars().count() > MAX_MEMBER_KEY_CHARS || value.contains('\0') {
            return Err(TransferError::Invalid(
                "評価復元情報が長すぎます".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_tags(tags: &[PortableTag]) -> Result<(), TransferError> {
    if tags.len() > MAX_BOOKMARKS_PER_ENTRY {
        return Err(TransferError::Invalid("タグ数が多すぎます".to_string()));
    }
    let mut keys = HashSet::with_capacity(tags.len());
    for tag in tags {
        if !crate::tags_db::is_valid_tag_display_name(&tag.name)
            || !keys.insert(crate::tags_db::normalize_tag_key(&tag.name))
        {
            return Err(TransferError::Invalid(
                "不正または重複したタグ名があります".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_title(title: Option<&str>) -> Result<(), TransferError> {
    if title.is_some_and(|value| value.chars().count() > MAX_TITLE_CHARS || value.contains('\0')) {
        Err(TransferError::Invalid(
            "ブックマーク名が長すぎます".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn validate_book_bookmark(
    bookmark: &PortableBookBookmark,
    media_kind: PortableMediaKind,
    entry_path: &str,
) -> Result<(), TransferError> {
    let valid_pair = matches!(
        (
            bookmark.container_kind.as_str(),
            bookmark.page_kind.as_str(),
            media_kind,
        ),
        (
            "compiled_book" | "image_folder",
            "relative_path",
            PortableMediaKind::Directory,
        ) | ("zip", "archive_entry", PortableMediaKind::Zip)
            | (
                "other_archive",
                "archive_entry",
                PortableMediaKind::ConvertibleArchive,
            )
            | ("pdf", "pdf_page", PortableMediaKind::Pdf)
    );
    if !valid_pair {
        return Err(TransferError::Invalid(format!(
            "本ブックマークのコンテナとページ種別の組み合わせが不正です: {entry_path}"
        )));
    }

    match bookmark.page_kind.as_str() {
        "relative_path" | "archive_entry" => {
            validate_bookmark_page_path(&bookmark.page_value, entry_path)
        }
        "pdf_page" => {
            let valid = !bookmark.page_value.is_empty()
                && bookmark.page_value.chars().count() <= MAX_MEMBER_KEY_CHARS
                && !bookmark.page_value.contains('\0')
                && bookmark
                    .page_value
                    .parse::<u32>()
                    .is_ok_and(|page| page.to_string() == bookmark.page_value);
            if valid {
                Ok(())
            } else {
                Err(TransferError::Invalid(format!(
                    "本ブックマークのページ指定が不正です: {entry_path}"
                )))
            }
        }
        _ => Err(TransferError::Invalid(format!(
            "本ブックマークのページ種別が不正です: {entry_path}"
        ))),
    }
}

/// 本ブックマークのページ指定は OS パスや archive member として後段へ渡る。
/// `\\` も区切りとして扱い、絶対パス、drive-relative path、`.` / `..` を拒否する。
fn validate_bookmark_page_path(value: &str, entry_path: &str) -> Result<(), TransferError> {
    let normalized = value.replace('\\', "/");
    let has_drive_prefix = normalized.as_bytes().get(1) == Some(&b':')
        && normalized
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic);
    let invalid = normalized.is_empty()
        || normalized.chars().count() > MAX_MEMBER_KEY_CHARS
        || normalized.contains('\0')
        || normalized.starts_with('/')
        || has_drive_prefix
        || normalized
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..");
    if invalid {
        Err(TransferError::Invalid(format!(
            "本ブックマークのページ指定が不正です: {entry_path}"
        )))
    } else {
        Ok(())
    }
}

fn validate_relative_path(value: &str) -> Result<(), TransferError> {
    if value.is_empty()
        || value.chars().count() > MAX_PATH_CHARS
        || value.contains('\0')
        || value.contains('\\')
    {
        return Err(TransferError::Invalid(format!(
            "不正な相対パスです: {value}"
        )));
    }
    if value == "." {
        return Ok(());
    }
    let path = Path::new(value);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
        || value
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(TransferError::Invalid(format!(
            "不正な相対パスです: {value}"
        )));
    }
    Ok(())
}

fn resolve_entry_path(
    root: &Path,
    canonical_root: &Path,
    relative: &str,
) -> Result<PathBuf, TransferError> {
    validate_relative_path(relative)?;
    let path = if relative == "." {
        root.to_path_buf()
    } else {
        root.join(relative)
    };
    if let Ok(canonical_target) = fs::canonicalize(&path) {
        let root_key = crate::path_key::normalize_keep_drive(canonical_root);
        let target_key = crate::path_key::normalize_keep_drive(&canonical_target);
        let descendant_prefix = if root_key.ends_with('/') {
            root_key.clone()
        } else {
            format!("{root_key}/")
        };
        if target_key != root_key && !target_key.starts_with(&descendant_prefix) {
            return Err(TransferError::Invalid(format!(
                "フォルダ外を指すパスです: {relative}"
            )));
        }
    }
    Ok(path)
}

fn validate_bookmark_page_targets(
    container_path: &Path,
    entry: &PortableEntry,
) -> Result<(), TransferError> {
    for bookmark in &entry.book_bookmarks {
        if bookmark.page_kind != "relative_path" {
            continue;
        }
        if matches!(
            crate::book_bookmarks::resolve_relative_page_path(container_path, &bookmark.page_value),
            crate::book_bookmarks::RelativePagePathResolution::Unsafe
        ) {
            return Err(TransferError::Invalid(format!(
                "本ブックマークがコンテナ外を指しています: {} / {}",
                entry.path, bookmark.page_value
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetState {
    Ready,
    Missing,
    KindMismatch,
    Changed,
}

fn image_other_file_pair(left: PortableMediaKind, right: PortableMediaKind) -> bool {
    matches!(
        (left, right),
        (PortableMediaKind::Image, PortableMediaKind::OtherFile)
            | (PortableMediaKind::OtherFile, PortableMediaKind::Image)
    )
}

fn verify_target(
    path: &Path,
    entry: &PortableEntry,
    cancel: &AtomicBool,
) -> Result<TargetState, TransferError> {
    check_cancel(cancel)?;
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(TargetState::Missing);
    };
    let local_media_kind = portable_media_kind(path, entry.kind);
    if local_media_kind != entry.media_kind
        && !image_other_file_pair(local_media_kind, entry.media_kind)
    {
        return Ok(TargetState::KindMismatch);
    }
    Ok(match entry.kind {
        PortableEntryKind::Directory if metadata.is_dir() => TargetState::Ready,
        PortableEntryKind::File if metadata.is_file() => {
            let Some(fingerprint) = entry.fingerprint.as_ref() else {
                return Ok(TargetState::Changed);
            };
            if fingerprint.size != metadata.len() {
                return Ok(TargetState::Changed);
            }
            TargetState::Ready
        }
        _ => TargetState::Changed,
    })
}

fn ensure_database_schemas(data_dir: &Path) -> Result<(), TransferError> {
    fs::create_dir_all(data_dir).map_err(|error| TransferError::Io(error.to_string()))?;
    drop(
        crate::rating_db::RatingDb::open_at(data_dir.join("rating.db"))
            .map_err(|error| schema_open_error("rating.db", error))?,
    );
    drop(
        crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db"))
            .map_err(|error| schema_open_error("tags.db", error))?,
    );
    drop(
        crate::video_bookmarks::VideoBookmarkDb::open_at(&data_dir.join("video_bookmarks.db"))
            .map_err(|error| schema_open_error("video_bookmarks.db", error))?,
    );
    crate::book_bookmarks::ensure_schema_at(&data_dir.join("book_bookmarks.db"))
        .map_err(|error| schema_open_error("book_bookmarks.db", error))?;
    drop(
        crate::adjustment_db::AdjustmentDb::open_at(&data_dir.join("adjustment.db"))
            .map_err(|error| schema_open_error("adjustment.db", error))?,
    );
    drop(
        crate::mask_db::MaskDb::open_at(&data_dir.join("mask.db"))
            .map_err(|error| schema_open_error("mask.db", error))?,
    );
    drop(
        crate::conceal_db::ConcealDb::open_at(&data_dir.join("conceal.db"))
            .map_err(|error| schema_open_error("conceal.db", error))?,
    );
    drop(
        crate::local_adjust_db::LocalAdjustDb::open_at(&data_dir.join("local_adjust.db"))
            .map_err(|error| schema_open_error("local_adjust.db", error))?,
    );
    drop(
        crate::export_crop::CropDb::open_at(&data_dir.join("export_crop.db"))
            .map_err(|error| schema_open_error("export_crop.db", error))?,
    );
    drop(
        crate::comic_db::ComicDb::open_at(&data_dir.join("comic.db"))
            .map_err(|error| schema_open_error("comic.db", error))?,
    );
    drop(
        crate::view_trim_db::ViewTrimDb::open_at(&data_dir.join("view_trim.db"))
            .map_err(|error| schema_open_error("view_trim.db", error))?,
    );
    drop(
        crate::rotation_db::RotationDb::open_at(&data_dir.join("rotation.db"))
            .map_err(|error| schema_open_error("rotation.db", error))?,
    );
    drop(
        crate::spread_db::SpreadDb::open_at(&data_dir.join("spread.db"))
            .map_err(|error| schema_open_error("spread.db", error))?,
    );
    drop(
        crate::folder_thumb_pins::FolderThumbPinDb::open_at(&data_dir.join("folder_thumb_pins.db"))
            .map_err(|error| schema_open_error("folder_thumb_pins.db", error))?,
    );
    drop(
        crate::video_pins::VideoPinDb::open_at(&data_dir.join("video_pins.db"))
            .map_err(|error| schema_open_error("video_pins.db", error))?,
    );
    Ok(())
}

fn schema_open_error(filename: &str, error: rusqlite::Error) -> TransferError {
    TransferError::Database(format!(
        "{filename}を開いてスキーマを確認できませんでした: {error}"
    ))
}

fn attach_error(filename: &str, error: rusqlite::Error) -> TransferError {
    TransferError::Database(format!("{filename}をATTACHできませんでした: {error}"))
}

fn switch_tags_to_rollback_journal(conn: &Connection) -> Result<String, TransferError> {
    const ATTEMPTS: usize = 5;
    const ATTEMPT_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(375);
    const IMPORT_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    // journal mode切替だけは短い上限で再試行する。5回のbusy待ちと4回の間隔を
    // 合わせても約2秒で打ち切り、UI側で解放できなかった接続を無限に待たない。
    conn.busy_timeout(ATTEMPT_BUSY_TIMEOUT).map_err(|error| {
        TransferError::Database(format!(
            "tags.dbのjournal mode切替待機を設定できませんでした: {error}"
        ))
    })?;
    for attempt in 1..=ATTEMPTS {
        match conn.query_row("PRAGMA tags.journal_mode = DELETE", [], |row| row.get(0)) {
            Ok(mode) => {
                conn.busy_timeout(IMPORT_BUSY_TIMEOUT).map_err(|error| {
                    TransferError::Database(format!(
                        "tags.dbのimport待機時間を復元できませんでした: {error}"
                    ))
                })?;
                return Ok(mode);
            }
            Err(error) => {
                crate::logger::log(format!(
                    "metadata import: tags.db journal mode switch failed ({attempt}/{ATTEMPTS}): {error}"
                ));
                if attempt < ATTEMPTS {
                    std::thread::sleep(RETRY_DELAY);
                }
            }
        }
    }
    let _ = conn.busy_timeout(IMPORT_BUSY_TIMEOUT);
    Err(TransferError::Database(
        "他の接続がtags.dbを開いたままのため、tags.dbをrollback journalへ切り替えられませんでした"
            .to_string(),
    ))
}

fn open_import_connection(data_dir: &Path) -> Result<Connection, TransferError> {
    let conn = Connection::open(data_dir.join("rating.db"))
        .map_err(|error| schema_open_error("rating.db", error))?;
    // 15 storeのfamily delete / sparse insertを項目ごとに再利用する。既定16では
    // statement種類数を下回り、巨大importでprepare/finalizeが再発する。
    conn.set_prepared_statement_cache_capacity(64);
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(db_error)?;
    conn.execute(
        "ATTACH DATABASE ?1 AS tags",
        [data_dir.join("tags.db").to_string_lossy().as_ref()],
    )
    .map_err(|error| attach_error("tags.db", error))?;
    conn.execute(
        "ATTACH DATABASE ?1 AS video",
        [data_dir
            .join("video_bookmarks.db")
            .to_string_lossy()
            .as_ref()],
    )
    .map_err(|error| attach_error("video_bookmarks.db", error))?;
    conn.execute(
        "ATTACH DATABASE ?1 AS book",
        [data_dir
            .join("book_bookmarks.db")
            .to_string_lossy()
            .as_ref()],
    )
    .map_err(|error| attach_error("book_bookmarks.db", error))?;
    for (schema, filename) in [
        ("adjustment", "adjustment.db"),
        ("mask", "mask.db"),
        ("conceal", "conceal.db"),
        ("local_adjust", "local_adjust.db"),
        ("crop", "export_crop.db"),
        ("comic", "comic.db"),
        ("view_trim", "view_trim.db"),
        ("rotation", "rotation.db"),
        ("spread", "spread.db"),
        ("folder_pin", "folder_thumb_pins.db"),
        ("video_pin", "video_pins.db"),
    ] {
        conn.execute(
            &format!("ATTACH DATABASE ?1 AS {schema}"),
            [data_dir.join(filename).to_string_lossy().as_ref()],
        )
        .map_err(|error| attach_error(filename, error))?;
    }

    // TagsDb normally uses WAL for concurrent UI reads.  A transaction spanning
    // WAL and rollback-journal databases has no SQLite super-journal and is not
    // crash-atomic.  The UI transfers ownership of its idle TagsDb connection to
    // the import worker before this point, so it is safe to switch tags.db to a
    // rollback journal for the duration of import.  TagsDb::open restores WAL
    // after the import connection has been dropped.
    let tag_mode = switch_tags_to_rollback_journal(&conn)?;
    if !tag_mode.eq_ignore_ascii_case("delete") {
        return Err(TransferError::Database(format!(
            "tags.db を安全なjournal modeへ切り替えられませんでした: {tag_mode}"
        )));
    }

    // Same invariant as edit_bundle::apply_atomic: every participant must use a
    // disk rollback journal so SQLite can coordinate them with a super-journal.
    for schema in [
        "main",
        "tags",
        "video",
        "book",
        "adjustment",
        "mask",
        "conceal",
        "local_adjust",
        "crop",
        "comic",
        "view_trim",
        "rotation",
        "spread",
        "folder_pin",
        "video_pin",
    ] {
        let mode: String = conn
            .query_row(&format!("PRAGMA {schema}.journal_mode"), [], |row| {
                row.get(0)
            })
            .map_err(db_error)?;
        if ["wal", "memory", "off"]
            .iter()
            .any(|unsafe_mode| mode.eq_ignore_ascii_case(unsafe_mode))
        {
            return Err(TransferError::Database(format!(
                "{schema} DBが{mode}モードのため、安全なメタ情報importを実行できません"
            )));
        }
    }
    Ok(conn)
}

fn apply_entry(
    tx: &Connection,
    data_dir: &Path,
    path: &Path,
    entry: &PortableEntry,
    sections: ManifestSections,
    fallback_time_ms: i64,
    automatic_sidecar_sync: Option<(String, i64, bool, bool)>,
) -> Result<(), TransferError> {
    let base_key = crate::path_key::normalize_keep_drive(path);
    let deterministic_cache_key = matches!(
        entry.media_kind,
        PortableMediaKind::Zip | PortableMediaKind::ConvertibleArchive
    )
    .then(|| {
        crate::path_key::normalize_keep_drive(&crate::archive_cache::cache_zip_path_for_data_dir(
            data_dir, path,
        ))
    });
    let page_base_key = match entry.virtual_key_base {
        PortableVirtualKeyBase::Source => base_key.clone(),
        PortableVirtualKeyBase::ConvertedCache => deterministic_cache_key
            .clone()
            .ok_or_else(|| TransferError::Invalid("変換cacheのファイル種別が不正です".into()))?,
    };
    let container_source_key = crate::path_key::normalize(path);
    let container_cache_key = matches!(
        entry.media_kind,
        PortableMediaKind::Zip | PortableMediaKind::ConvertibleArchive
    )
    .then(|| {
        crate::path_key::normalize(&crate::archive_cache::cache_zip_path_for_data_dir(
            data_dir, path,
        ))
    });
    let container_target_key = match entry.container_key_base {
        PortableVirtualKeyBase::Source => container_source_key.clone(),
        PortableVirtualKeyBase::ConvertedCache => container_cache_key
            .clone()
            .ok_or_else(|| TransferError::Invalid("変換cacheのファイル種別が不正です".into()))?,
    };
    let supports_timed_bookmarks = matches!(
        entry.media_kind,
        PortableMediaKind::Video | PortableMediaKind::Audio
    );
    let supports_container = matches!(
        entry.media_kind,
        PortableMediaKind::Directory
            | PortableMediaKind::Zip
            | PortableMediaKind::Pdf
            | PortableMediaKind::ConvertibleArchive
    );
    let supports_page_state = entry.media_kind == PortableMediaKind::Image
        || matches!(
            entry.media_kind,
            PortableMediaKind::Zip | PortableMediaKind::Pdf | PortableMediaKind::ConvertibleArchive
        );
    if sections.ratings {
        delete_page_key_family(
            tx,
            "ratings",
            "path",
            &base_key,
            &page_base_key,
            deterministic_cache_key.as_deref(),
        )?;
        if let Some(rating) = &entry.rating {
            insert_rating(tx, &base_key, path, rating)?;
        }
        for item in &entry.virtual_items {
            if let Some(rating) = &item.rating {
                insert_rating(
                    tx,
                    &canonical_virtual_item_key(&page_base_key, &item.member_key),
                    path,
                    rating,
                )?;
            }
        }
    }
    if sections.tags {
        delete_page_key_family(
            tx,
            "tags.item_tags",
            "item_key",
            &base_key,
            &page_base_key,
            deterministic_cache_key.as_deref(),
        )?;
        delete_page_key_family(
            tx,
            "tags.tag_item_state",
            "item_key",
            &base_key,
            &page_base_key,
            deterministic_cache_key.as_deref(),
        )?;
        insert_tags(
            tx,
            &base_key,
            &entry.tags,
            entry.tags_decided,
            fallback_time_ms,
        )?;
        for item in &entry.virtual_items {
            insert_tags(
                tx,
                &canonical_virtual_item_key(&page_base_key, &item.member_key),
                &item.tags,
                item.tags_decided,
                fallback_time_ms,
            )?;
        }
    }
    if sections.timed_bookmarks && supports_timed_bookmarks {
        tx.prepare_cached("DELETE FROM video.video_bookmarks WHERE path = ?1")
            .map_err(db_error)?
            .execute([&base_key])
            .map_err(db_error)?;
        for bookmark in &entry.timed_bookmarks {
            let thumb_webp = bookmark
                .thumb_webp_base64
                .as_ref()
                .map(|encoded| base64::engine::general_purpose::STANDARD.decode(encoded.as_bytes()))
                .transpose()
                .map_err(|_| {
                    TransferError::Invalid(format!(
                        "時刻ブックマークの動画サムネの base64 が不正です: {}",
                        entry.path
                    ))
                })?
                .filter(|bytes| !bytes.is_empty());
            tx.prepare_cached(
                "INSERT INTO video.video_bookmarks
                    (path, pts_secs, title, thumb_webp, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .map_err(db_error)?
            .execute(params![
                base_key,
                bookmark.pts_secs,
                bookmark.title.as_deref(),
                thumb_webp,
                bookmark.created_at_ms.div_euclid(1000),
            ])
            .map_err(db_error)?;
        }
    }
    if sections.book_bookmarks && supports_container {
        tx.prepare_cached("DELETE FROM book.book_bookmarks WHERE container_key = ?1")
            .map_err(db_error)?
            .execute([&base_key])
            .map_err(db_error)?;
        for bookmark in &entry.book_bookmarks {
            let page_key = match bookmark.page_kind.as_str() {
                "relative_path" | "archive_entry" => canonical_member_key(&bookmark.page_value),
                "pdf_page" => bookmark.page_value.clone(),
                _ => {
                    return Err(TransferError::Invalid(
                        "本ブックマーク種別が不正です".into(),
                    ));
                }
            };
            tx.prepare_cached(
                "INSERT INTO book.book_bookmarks
                    (container_key, container_path, container_kind, page_kind, page_value,
                     page_key, page_index_hint, created_at_ms, title)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )
            .map_err(db_error)?
            .execute(params![
                base_key,
                path.to_string_lossy().as_ref(),
                bookmark.container_kind,
                bookmark.page_kind,
                bookmark.page_value,
                page_key,
                bookmark.page_index_hint.min(i64::MAX as usize) as i64,
                bookmark.created_at_ms,
                bookmark.title.as_deref(),
            ])
            .map_err(db_error)?;
        }
    }
    if let Some((folder_key, modified_secs, write_adjustment, write_tags)) = automatic_sidecar_sync
    {
        // 明示 import が正本になった後、同じ場所へ残っている自動 backup の旧値を
        // 「DB に行が無い」項目へ再 import して復活させない。明示bundleのsectionが
        // 対象外なら、その種類の自動sidecar同期状態には触れない。
        if write_adjustment {
            tx.prepare_cached(
                "INSERT INTO adjustment.sidecar_sync (folder_key, sidecar_mtime)
                 VALUES (?1, ?2)
                 ON CONFLICT(folder_key) DO UPDATE SET sidecar_mtime = ?2",
            )
            .map_err(db_error)?
            .execute(params![folder_key, modified_secs])
            .map_err(db_error)?;
        }
        if write_tags {
            tx.prepare_cached(
                "INSERT INTO tags.tag_sidecar_sync (folder_key, sidecar_mtime)
                 VALUES (?1, ?2)
                 ON CONFLICT(folder_key) DO UPDATE SET sidecar_mtime = ?2",
            )
            .map_err(db_error)?
            .execute(params![folder_key, modified_secs])
            .map_err(db_error)?;
        }
    }
    if sections.page_state && supports_page_state {
        delete_page_key_family(
            tx,
            "adjustment.page_params",
            "page_path",
            &base_key,
            &page_base_key,
            deterministic_cache_key.as_deref(),
        )?;
        delete_page_key_family(
            tx,
            "mask.masks",
            "path",
            &base_key,
            &page_base_key,
            deterministic_cache_key.as_deref(),
        )?;
        delete_page_key_family(
            tx,
            "conceal.conceal_entries",
            "page_path",
            &base_key,
            &page_base_key,
            deterministic_cache_key.as_deref(),
        )?;
        delete_page_key_family(
            tx,
            "local_adjust.local_adjust_pages",
            "page_path",
            &base_key,
            &page_base_key,
            deterministic_cache_key.as_deref(),
        )?;
        delete_page_key_family(
            tx,
            "crop.export_crop_pages",
            "page_path",
            &base_key,
            &page_base_key,
            deterministic_cache_key.as_deref(),
        )?;
        delete_page_key_family(
            tx,
            "comic.comic_entries",
            "page_path",
            &base_key,
            &page_base_key,
            deterministic_cache_key.as_deref(),
        )?;
        delete_page_key_family(
            tx,
            "view_trim.view_trim_pages",
            "page_path",
            &base_key,
            &page_base_key,
            deterministic_cache_key.as_deref(),
        )?;
        delete_page_key_family(
            tx,
            "rotation.rotations",
            "path",
            &base_key,
            &page_base_key,
            deterministic_cache_key.as_deref(),
        )?;
        insert_page_state(tx, &base_key, &entry.page_state)?;
        for item in &entry.virtual_items {
            insert_page_state(
                tx,
                &canonical_virtual_item_key(&page_base_key, &item.member_key),
                &item.page_state,
            )?;
        }
    }
    if sections.container_state && supports_container {
        let include_nested = entry.kind == PortableEntryKind::File;
        delete_container_key_family(
            tx,
            "spread.spreads",
            "path",
            &container_source_key,
            container_cache_key.as_deref(),
            include_nested,
        )?;
        delete_container_key_family(
            tx,
            "view_trim.view_trim_books",
            "book_key",
            &container_source_key,
            container_cache_key.as_deref(),
            include_nested,
        )?;
        insert_container_state(tx, &container_target_key, &entry.container_state)?;
        for container in &entry.nested_containers {
            insert_container_state(
                tx,
                &join_container_key(&container_target_key, &container.member_key),
                &container.state,
            )?;
        }
    }
    if sections.thumbnail_pins {
        if supports_container {
            let include_nested = entry.kind == PortableEntryKind::File;
            delete_container_family(
                tx,
                "folder_pin.folder_thumb_pins",
                "container_key",
                &base_key,
                include_nested,
            )?;
            insert_folder_pin(
                tx,
                &base_key,
                entry.container_state.folder_thumb_pin.as_ref(),
            )?;
            for container in &entry.nested_containers {
                insert_folder_pin(
                    tx,
                    &join_container_key(&base_key, &container.member_key),
                    container.state.folder_thumb_pin.as_ref(),
                )?;
            }
        }
        if entry.media_kind == PortableMediaKind::Video {
            tx.prepare_cached("DELETE FROM video_pin.video_pins WHERE path = ?1")
                .map_err(db_error)?
                .execute([&base_key])
                .map_err(db_error)?;
        }
        if let Some(pin) = &entry.video_pin {
            let webp = pin
                .thumb_webp_base64
                .as_ref()
                .map(|encoded| base64::engine::general_purpose::STANDARD.decode(encoded.as_bytes()))
                .transpose()
                .map_err(|_| TransferError::Invalid("動画サムネの base64 が不正です".into()))?;
            tx.prepare_cached(
                "INSERT INTO video_pin.video_pins
                        (path, pin_pts_secs, thumb_webp, thumb_pts_secs)
                     VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(db_error)?
            .execute(params![
                base_key,
                pin.pin_pts_secs,
                webp,
                pin.thumb_pts_secs
            ])
            .map_err(db_error)?;
        }
    }
    Ok(())
}

fn automatic_sidecar_sync_cached(
    cache: &mut HashMap<PathBuf, Option<(String, i64)>>,
    path: &Path,
    kind: PortableEntryKind,
) -> Option<(String, i64)> {
    let folder = match kind {
        PortableEntryKind::Directory => path,
        PortableEntryKind::File => path.parent()?,
    };
    if let Some(cached) = cache.get(folder) {
        return cached.clone();
    }
    let value = fs::metadata(folder.join(crate::sidecar::SIDECAR_FILENAME))
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|modified| {
            (
                crate::adjustment_db::normalize_path(folder),
                modified.as_secs().min(i64::MAX as u64) as i64,
            )
        });
    cache.insert(folder.to_path_buf(), value.clone());
    value
}

fn insert_page_state(
    tx: &Connection,
    key: &str,
    state: &PortablePageState,
) -> Result<(), TransferError> {
    if let Some(angle) = state.rotation_degrees {
        tx.prepare_cached("INSERT INTO rotation.rotations (path, angle) VALUES (?1, ?2)")
            .map_err(db_error)?
            .execute(params![key, angle])
            .map_err(db_error)?;
    }
    if let Some(adjustment) = &state.adjustment {
        let json = serde_json::to_string(adjustment)
            .map_err(|error| TransferError::Invalid(format!("画像補正: {error}")))?;
        tx.prepare_cached(
            "INSERT INTO adjustment.page_params (page_path, params_json) VALUES (?1, ?2)",
        )
        .map_err(db_error)?
        .execute(params![key, json])
        .map_err(db_error)?;
    }
    insert_mask(tx, "mask.masks", "path", key, state.mask.as_ref())?;
    insert_mask(
        tx,
        "conceal.conceal_entries",
        "page_path",
        key,
        state.conceal.as_ref(),
    )?;
    if let Some(layers) = &state.local_adjust_layers {
        let json = serde_json::to_string(layers)
            .map_err(|error| TransferError::Invalid(format!("部分補正: {error}")))?;
        tx.prepare_cached(
            "INSERT INTO local_adjust.local_adjust_pages (page_path, layers_json, updated_at)
             VALUES (?1, ?2, unixepoch())",
        )
        .map_err(db_error)?
        .execute(params![key, json])
        .map_err(db_error)?;
    }
    if let Some(crop) = state.export_crop {
        tx.prepare_cached(
            "INSERT INTO crop.export_crop_pages
                (page_path, min_x, min_y, max_x, max_y, aspect_mode, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())",
        )
        .map_err(db_error)?
        .execute(params![
            key,
            crop.rect.min_x,
            crop.rect.min_y,
            crop.rect.max_x,
            crop.rect.max_y,
            crop.aspect_mode.stable_key(),
        ])
        .map_err(db_error)?;
    }
    if let Some(objects) = &state.comic {
        let json = serde_json::to_string(objects)
            .map_err(|error| TransferError::Invalid(format!("テキスト注釈: {error}")))?;
        tx.prepare_cached(
            "INSERT INTO comic.comic_entries (page_path, doc_version, doc_json)
             VALUES (?1, ?2, ?3)",
        )
        .map_err(db_error)?
        .execute(params![key, crate::comic_db::DOC_VERSION, json])
        .map_err(db_error)?;
    }
    if let Some(view_trim) = state.view_trim {
        let json = serde_json::to_string(&view_trim)
            .map_err(|error| TransferError::Invalid(format!("表示トリム: {error}")))?;
        tx.prepare_cached(
            "INSERT INTO view_trim.view_trim_pages (page_path, override_json, updated_at)
             VALUES (?1, ?2, unixepoch())",
        )
        .map_err(db_error)?
        .execute(params![key, json])
        .map_err(db_error)?;
    }
    Ok(())
}

fn insert_mask(
    tx: &Connection,
    table: &str,
    key_column: &str,
    key: &str,
    mask: Option<&crate::sidecar::SidecarMask>,
) -> Result<(), TransferError> {
    let Some(mask) = mask else {
        return Ok(());
    };
    let data = mask
        .decode()
        .ok_or_else(|| TransferError::Invalid("マスクの base64 が不正です".into()))?;
    let vectors = crate::mask_db::shapes_to_json(&mask.vectors);
    let sql = if table == "mask.masks" {
        format!(
            "INSERT INTO {table} ({key_column}, mask_data, width, height, vectors)
             VALUES (?1, ?2, ?3, ?4, ?5)"
        )
    } else {
        format!(
            "INSERT INTO {table} ({key_column}, bitmap_data, bitmap_w, bitmap_h, shapes)
             VALUES (?1, ?2, ?3, ?4, ?5)"
        )
    };
    tx.prepare_cached(&sql)
        .map_err(db_error)?
        .execute(params![key, data, mask.w, mask.h, vectors])
        .map_err(db_error)?;
    Ok(())
}

fn insert_container_state(
    tx: &Connection,
    stripped_key: &str,
    state: &PortableContainerState,
) -> Result<(), TransferError> {
    if let Some(spread) = state.spread {
        tx.prepare_cached(
            "INSERT INTO spread.spreads (path, mode, flow, direction)
                 VALUES (?1, ?2, ?3, ?4)",
        )
        .map_err(db_error)?
        .execute(params![
            stripped_key,
            spread.mode,
            spread.flow,
            spread.direction
        ])
        .map_err(db_error)?;
    }
    if let Some(view_trim) = state.view_trim {
        let json = serde_json::to_string(&view_trim)
            .map_err(|error| TransferError::Invalid(format!("表示トリム: {error}")))?;
        tx.prepare_cached(
            "INSERT INTO view_trim.view_trim_books (book_key, state_json, updated_at)
             VALUES (?1, ?2, unixepoch())",
        )
        .map_err(db_error)?
        .execute(params![stripped_key, json])
        .map_err(db_error)?;
    }
    Ok(())
}

fn insert_folder_pin(
    tx: &Connection,
    key: &str,
    pin: Option<&PortableFolderThumbPin>,
) -> Result<(), TransferError> {
    let Some(pin) = pin else {
        return Ok(());
    };
    portable_folder_pin_source(pin)
        .ok_or_else(|| TransferError::Invalid("代表サムネ設定が不正です".into()))?;
    tx.prepare_cached(
        "INSERT INTO folder_pin.folder_thumb_pins
                (container_key, source_kind, source_rel, source_entry, source_page)
             VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .map_err(db_error)?
    .execute(params![
        key,
        pin.source_kind,
        pin.source_rel,
        pin.source_entry,
        pin.source_page,
    ])
    .map_err(db_error)?;
    Ok(())
}

fn delete_container_family(
    tx: &Connection,
    table: &str,
    column: &str,
    base_key: &str,
    include_nested: bool,
) -> Result<(), TransferError> {
    let sql = if include_nested {
        format!(
            "DELETE FROM {table} WHERE {column} = ?1
                OR ({column} >= ?1 || '/' AND {column} < ?1 || '0')"
        )
    } else {
        format!("DELETE FROM {table} WHERE {column} = ?1")
    };
    tx.prepare_cached(&sql)
        .map_err(db_error)?
        .execute([base_key])
        .map_err(db_error)?;
    Ok(())
}

fn delete_container_key_family(
    tx: &Connection,
    table: &str,
    column: &str,
    source_key: &str,
    deterministic_cache_key: Option<&str>,
    include_nested: bool,
) -> Result<(), TransferError> {
    let alternate_key = deterministic_cache_key.filter(|cache_key| *cache_key != source_key);
    if let Some(alternate_key) = alternate_key {
        let sql = if include_nested {
            format!(
                "DELETE FROM {table}
                  WHERE {column} = ?1
                     OR ({column} >= ?1 || '/' AND {column} < ?1 || '0')
                     OR {column} = ?2
                     OR ({column} >= ?2 || '/' AND {column} < ?2 || '0')"
            )
        } else {
            format!("DELETE FROM {table} WHERE {column} = ?1 OR {column} = ?2")
        };
        tx.prepare_cached(&sql)
            .map_err(db_error)?
            .execute(params![source_key, alternate_key])
            .map_err(db_error)?;
        Ok(())
    } else {
        delete_container_family(tx, table, column, source_key, include_nested)
    }
}

fn join_container_key(base: &str, member: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        canonical_member_key(member)
    )
}

fn delete_page_key_family(
    tx: &Connection,
    table: &str,
    column: &str,
    base_key: &str,
    page_base_key: &str,
    deterministic_cache_key: Option<&str>,
) -> Result<(), TransferError> {
    let alternate_key = if page_base_key != base_key {
        Some(page_base_key)
    } else {
        deterministic_cache_key.filter(|cache_key| *cache_key != base_key)
    };
    debug_assert!(
        deterministic_cache_key
            .map(|cache_key| cache_key == base_key || Some(cache_key) == alternate_key)
            .unwrap_or(true),
        "page base must be either the source key or deterministic cache key"
    );
    if let Some(alternate_key) = alternate_key {
        let sql = format!(
            "DELETE FROM {table}
              WHERE {column} = ?1
                 OR ({column} >= ?1 || '::' AND {column} < ?1 || ':;')
                 OR {column} = ?2
                 OR ({column} >= ?2 || '::' AND {column} < ?2 || ':;')"
        );
        tx.prepare_cached(&sql)
            .map_err(db_error)?
            .execute(params![base_key, alternate_key])
            .map_err(db_error)?;
    } else {
        delete_key_family(tx, table, column, base_key)?;
    }
    Ok(())
}

fn delete_key_family(
    tx: &Connection,
    table: &str,
    column: &str,
    base_key: &str,
) -> Result<(), TransferError> {
    let sql = format!(
        "DELETE FROM {table}
          WHERE {column} = ?1
             OR ({column} >= ?1 || '::' AND {column} < ?1 || ':;')"
    );
    tx.prepare_cached(&sql)
        .map_err(db_error)?
        .execute([base_key])
        .map_err(db_error)?;
    Ok(())
}

fn insert_rating(
    tx: &Connection,
    key: &str,
    source_path: &Path,
    rating: &PortableRating,
) -> Result<(), TransferError> {
    tx.prepare_cached(
        "INSERT INTO ratings
            (path, stars, rated_at_ms, source_path, kind, entry_name, page_num,
             dir_prefix, archive_format, zipdir_is_archive, zipdir_representative)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )
    .map_err(db_error)?
    .execute(params![
        key,
        rating.stars,
        rating.rated_at_ms,
        source_path.to_string_lossy().as_ref(),
        rating.kind.as_deref().and_then(rating_kind_value),
        rating.entry_name.as_deref(),
        rating.page_num.map(i64::from),
        rating.dir_prefix.as_deref(),
        rating.archive_format.as_deref(),
        rating.zipdir_is_archive.map(i64::from),
        rating.zipdir_representative.as_deref(),
    ])
    .map_err(db_error)?;
    Ok(())
}

fn insert_tags(
    tx: &Connection,
    key: &str,
    tags: &[PortableTag],
    tags_decided: bool,
    fallback_time_ms: i64,
) -> Result<(), TransferError> {
    for tag in tags {
        let display = crate::tags_db::normalize_tag_display_name(&tag.name);
        let tag_key = crate::tags_db::normalize_tag_key(&display);
        tx.prepare_cached(
            "INSERT INTO tags.item_tags (item_key, tag, tag_key, applied_at)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .map_err(db_error)?
        .execute(params![key, display, tag_key, tag.applied_at])
        .map_err(db_error)?;
    }
    if tags_decided {
        let decided_at = fallback_time_ms.div_euclid(1000);
        tx.prepare_cached(
            "INSERT INTO tags.tag_item_state (item_key, decided_at, source)
             VALUES (?1, ?2, ?3)",
        )
        .map_err(db_error)?
        .execute(params![
            key,
            decided_at,
            crate::tags_db::source::METADATA_IMPORT
        ])
        .map_err(db_error)?;
    }
    Ok(())
}

fn open_readonly(path: &Path) -> Result<Connection, TransferError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(db_error)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(db_error)?;
    Ok(conn)
}

enum BundlePublishMode {
    ExistingDirectory,
    NewBundle,
}

struct BundleStaging {
    destination: PathBuf,
    bundle_dir: PathBuf,
    generation_dir: PathBuf,
    mode: BundlePublishMode,
    published: bool,
}

impl BundleStaging {
    fn create(root: &Path, generation: &str) -> Result<Self, TransferError> {
        validate_generation(generation)?;
        let destination = root.join(SIDECAR_FILENAME);
        let existing = match fs::symlink_metadata(&destination) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(TransferError::Io(format!(
                    "{}: {error}",
                    destination.display()
                )));
            }
        };
        let (bundle_dir, mode, old_generation) = match existing {
            Some(metadata) if metadata.is_dir() && !metadata_is_reparse(&metadata) => {
                let old_generation = read_existing_generation(&destination);
                (
                    destination.clone(),
                    BundlePublishMode::ExistingDirectory,
                    old_generation,
                )
            }
            Some(metadata) if metadata.is_file() && !metadata_is_reparse(&metadata) => (
                root.join(format!(
                    ".{SIDECAR_FILENAME}.{}.tmp",
                    uuid::Uuid::new_v4().simple()
                )),
                BundlePublishMode::NewBundle,
                None,
            ),
            Some(_) => {
                return Err(TransferError::Invalid(format!(
                    "sidecarが通常のファイルまたはフォルダではありません: {}",
                    destination.display()
                )));
            }
            None => (
                root.join(format!(
                    ".{SIDECAR_FILENAME}.{}.tmp",
                    uuid::Uuid::new_v4().simple()
                )),
                BundlePublishMode::NewBundle,
                None,
            ),
        };

        if matches!(mode, BundlePublishMode::NewBundle) {
            fs::create_dir(&bundle_dir)
                .map_err(|error| TransferError::Io(format!("{}: {error}", bundle_dir.display())))?;
        } else {
            ensure_plain_directory(&bundle_dir)?;
            let manifest_path = bundle_dir.join(BUNDLE_MANIFEST_FILENAME);
            match fs::symlink_metadata(&manifest_path) {
                Ok(_) => ensure_plain_file(&manifest_path)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(TransferError::Io(format!(
                        "{}: {error}",
                        manifest_path.display()
                    )));
                }
            }
        }
        let generations_dir = bundle_dir.join(GENERATIONS_DIRNAME);
        match fs::symlink_metadata(&generations_dir) {
            Ok(_) => ensure_plain_directory(&generations_dir)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&generations_dir).map_err(|error| {
                    TransferError::Io(format!("{}: {error}", generations_dir.display()))
                })?;
            }
            Err(error) => {
                return Err(TransferError::Io(format!(
                    "{}: {error}",
                    generations_dir.display()
                )));
            }
        }
        // 前回crashでpointer公開前に残ったgenerationは、現在のmanifestが指す
        // generationだけを保護して次回export開始時にbest-effort回収する。
        if matches!(mode, BundlePublishMode::ExistingDirectory)
            && let Some(active) = old_generation.as_deref()
        {
            cleanup_bundle_generations(&bundle_dir, active);
        }
        let generation_dir = bundle_generation_dir(&bundle_dir, generation);
        fs::create_dir(&generation_dir)
            .map_err(|error| TransferError::Io(format!("{}: {error}", generation_dir.display())))?;
        let shards_dir = generation_dir.join(SHARDS_DIRNAME);
        fs::create_dir(&shards_dir)
            .map_err(|error| TransferError::Io(format!("{}: {error}", shards_dir.display())))?;
        Ok(Self {
            destination,
            bundle_dir,
            generation_dir,
            mode,
            published: false,
        })
    }

    fn shards_dir(&self) -> PathBuf {
        self.generation_dir.join(SHARDS_DIRNAME)
    }

    fn publish(
        &mut self,
        manifest: &BundleManifest,
        cancel: &AtomicBool,
    ) -> Result<(), TransferError> {
        write_bundle_manifest_atomic(&self.bundle_dir, manifest, cancel)?;
        match self.mode {
            BundlePublishMode::ExistingDirectory => {
                self.published = true;
            }
            BundlePublishMode::NewBundle => {
                let backup = self.destination.with_file_name(format!(
                    ".{SIDECAR_FILENAME}.{}.old.tmp",
                    uuid::Uuid::new_v4().simple()
                ));
                let had_previous = self.destination.exists();
                if had_previous {
                    fs::rename(&self.destination, &backup).map_err(|error| {
                        TransferError::Io(format!("{}: {error}", self.destination.display()))
                    })?;
                }
                if let Err(error) = fs::rename(&self.bundle_dir, &self.destination) {
                    if had_previous {
                        let _ = fs::rename(&backup, &self.destination);
                    }
                    return Err(TransferError::Io(format!(
                        "{}: {error}",
                        self.destination.display()
                    )));
                }
                self.published = true;
                if had_previous {
                    let _ = fs::remove_file(&backup);
                }
            }
        }
        crate::sidecar::clear_hidden_system_preserving_other_attributes(&self.destination);
        cleanup_bundle_generations(&self.destination, self.generation_name());
        Ok(())
    }

    fn generation_name(&self) -> &str {
        self.generation_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    }

    fn abort(&mut self) {
        if self.published {
            return;
        }
        let target = match self.mode {
            BundlePublishMode::ExistingDirectory => &self.generation_dir,
            BundlePublishMode::NewBundle => &self.bundle_dir,
        };
        let _ = fs::remove_dir_all(target);
        self.published = true;
    }
}

fn cleanup_bundle_generations(bundle_dir: &Path, keep: &str) {
    if validate_generation(keep).is_err() {
        return;
    }
    let generations_dir = bundle_dir.join(GENERATIONS_DIRNAME);
    let Ok(entries) = fs::read_dir(&generations_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name == keep || validate_generation(&name).is_err() {
            continue;
        }
        let path = entry.path();
        let removable = fs::symlink_metadata(&path)
            .ok()
            .is_some_and(|metadata| metadata.is_dir() && !metadata_is_reparse(&metadata));
        if removable && let Err(error) = fs::remove_dir_all(&path) {
            crate::logger::log(format!(
                "metadata export: unpublished generation cleanup failed {}: {error}",
                path.display()
            ));
        }
    }
}

impl Drop for BundleStaging {
    fn drop(&mut self) {
        self.abort();
    }
}

fn write_bundle_manifest_atomic(
    bundle_dir: &Path,
    manifest: &BundleManifest,
    cancel: &AtomicBool,
) -> Result<(), TransferError> {
    let destination = bundle_dir.join(BUNDLE_MANIFEST_FILENAME);
    let temp = bundle_dir.join(format!(
        ".{BUNDLE_MANIFEST_FILENAME}.{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| {
        let file = File::create(&temp)
            .map_err(|error| TransferError::Io(format!("{}: {error}", temp.display())))?;
        let mut writer = BufWriter::new(file);
        {
            let mut cancel_writer = CancelWriter {
                inner: &mut writer,
                cancel,
                written: 0,
                max_bytes: MAX_MANIFEST_BYTES,
            };
            serde_json::to_writer_pretty(&mut cancel_writer, manifest).map_err(|error| {
                if cancel.load(Ordering::Relaxed) {
                    TransferError::Cancelled
                } else {
                    TransferError::Io(error.to_string())
                }
            })?;
        }
        writer
            .flush()
            .map_err(|error| TransferError::Io(error.to_string()))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| TransferError::Io(error.to_string()))?;
        check_cancel(cancel)?;
        replace_file_atomic(&temp, &destination)
            .map_err(|error| TransferError::Io(format!("{}: {error}", destination.display())))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn write_json_line<T: Serialize, W: Write>(
    writer: &mut W,
    value: &T,
    cancel: &AtomicBool,
    max_bytes: u64,
) -> Result<(), TransferError> {
    let mut cancel_writer = CancelWriter {
        inner: writer,
        cancel,
        written: 0,
        max_bytes,
    };
    serde_json::to_writer(&mut cancel_writer, value).map_err(|error| {
        if cancel.load(Ordering::Relaxed) {
            TransferError::Cancelled
        } else {
            TransferError::Io(error.to_string())
        }
    })?;
    cancel_writer.write_all(b"\n").map_err(|error| {
        if cancel.load(Ordering::Relaxed) {
            TransferError::Cancelled
        } else {
            TransferError::Io(error.to_string())
        }
    })
}

fn bundle_generation_dir(bundle_dir: &Path, generation: &str) -> PathBuf {
    bundle_dir.join(GENERATIONS_DIRNAME).join(generation)
}

fn shard_filename(folder: &str) -> String {
    let identity = folder.to_lowercase();
    format!(
        "{:x}.{SHARD_EXTENSION}",
        Sha256::digest(identity.as_bytes())
    )
}

fn validate_generation(value: &str) -> Result<(), TransferError> {
    if value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(TransferError::Invalid(
            "generation識別子が不正です".to_string(),
        ))
    }
}

fn read_existing_generation(bundle_dir: &Path) -> Option<String> {
    let path = bundle_dir.join(BUNDLE_MANIFEST_FILENAME);
    ensure_plain_file(&path).ok()?;
    let metadata = fs::metadata(&path).ok()?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    let manifest: BundleManifest =
        serde_json::from_reader(BufReader::new(File::open(path).ok()?)).ok()?;
    validate_bundle_manifest(&manifest).ok()?;
    Some(manifest.generation)
}

fn ensure_plain_directory(path: &Path) -> Result<(), TransferError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| TransferError::Io(format!("{}: {error}", path.display())))?;
    if metadata.is_dir() && !metadata_is_reparse(&metadata) {
        Ok(())
    } else {
        Err(TransferError::Invalid(format!(
            "通常のフォルダではありません: {}",
            path.display()
        )))
    }
}

fn ensure_plain_file(path: &Path) -> Result<(), TransferError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| TransferError::Io(format!("{}: {error}", path.display())))?;
    if metadata.is_file() && !metadata_is_reparse(&metadata) {
        Ok(())
    } else {
        Err(TransferError::Invalid(format!(
            "通常のファイルではありません: {}",
            path.display()
        )))
    }
}

#[cfg(windows)]
fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
fn write_manifest_atomic(
    root: &Path,
    manifest: &Manifest,
    cancel: &AtomicBool,
) -> Result<(), TransferError> {
    validate_manifest(manifest)?;
    let generation = uuid::Uuid::new_v4().simple().to_string();
    let mut staging = BundleStaging::create(root, &generation)?;
    let shards_dir = staging.shards_dir();
    let mut by_folder: HashMap<String, Vec<&PortableEntry>> = HashMap::new();
    for entry in &manifest.entries {
        let folder = if entry.path == "." {
            ".".to_string()
        } else {
            entry
                .path
                .rsplit_once('/')
                .map_or_else(|| ".".to_string(), |(parent, _)| parent.to_string())
        };
        by_folder.entry(folder).or_default().push(entry);
    }
    by_folder.entry(".".to_string()).or_default();
    let mut folders = by_folder.into_iter().collect::<Vec<_>>();
    folders.sort_by(|a, b| a.0.cmp(&b.0));
    for (folder, mut entries) in folders {
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let path = shards_dir.join(shard_filename(&folder));
        let mut writer = BufWriter::new(
            File::options()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| TransferError::Io(format!("{}: {error}", path.display())))?,
        );
        write_json_line(
            &mut writer,
            &ShardHeader {
                format: SHARD_FORMAT_NAME.to_string(),
                version: FORMAT_VERSION,
                generation: generation.clone(),
                folder,
            },
            cancel,
            MAX_SHARD_HEADER_BYTES,
        )?;
        for entry in entries {
            write_json_line(&mut writer, entry, cancel, MAX_RECORD_BYTES)?;
        }
        writer
            .flush()
            .map_err(|error| TransferError::Io(error.to_string()))?;
    }
    let bundle_manifest = BundleManifest {
        format: FORMAT_NAME.to_string(),
        version: FORMAT_VERSION,
        generation,
        exported_at_ms: manifest.exported_at_ms,
        recursive: manifest.recursive,
        sections: manifest.sections,
        shard_count: folders_len_for_manifest(&manifest.entries),
        entry_count: manifest.entries.len() as u64,
    };
    staging.publish(&bundle_manifest, cancel)
}

#[cfg(test)]
fn folders_len_for_manifest(entries: &[PortableEntry]) -> u64 {
    let mut folders = HashSet::from([".".to_string()]);
    for entry in entries {
        if let Some((parent, _)) = entry.path.rsplit_once('/') {
            folders.insert(parent.to_lowercase());
        }
    }
    folders.len() as u64
}

#[cfg(test)]
fn read_manifest(root: &Path, cancel: &AtomicBool) -> Result<Manifest, TransferError> {
    let bundle = read_bundle_manifest(root, cancel)?;
    let mut entries = Vec::new();
    visit_bundle_entries(root, &bundle, cancel, |entry, _, _record_bytes| {
        entries.push(entry.clone());
        Ok(())
    })?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Manifest {
        format: bundle.format,
        version: bundle.version,
        exported_at_ms: bundle.exported_at_ms,
        recursive: bundle.recursive,
        sections: bundle.sections,
        entries,
    })
}

#[cfg(windows)]
fn replace_file_atomic(temp: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;

    let temp_wide: Vec<u16> = temp.as_os_str().encode_wide().chain([0]).collect();
    let destination_wide: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        MoveFileExW(
            PCWSTR(temp_wide.as_ptr()),
            PCWSTR(destination_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(not(windows))]
fn replace_file_atomic(temp: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(temp, destination)
}

fn relative_string(root: &Path, path: &Path) -> Result<String, TransferError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| TransferError::Invalid(format!("対象外のパスです: {}", path.display())))?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(value) = component else {
            return Err(TransferError::Invalid(format!(
                "相対パスへ変換できません: {}",
                path.display()
            )));
        };
        let Some(value) = value.to_str() else {
            return Err(TransferError::Invalid(format!(
                "Unicodeで表せないファイル名です: {}",
                path.display()
            )));
        };
        parts.push(value);
    }
    Ok(parts.join("/"))
}

fn is_temp_sidecar_name(name: &std::ffi::OsStr) -> bool {
    crate::fs_entry::is_internal_app_entry_name(name)
        && !name
            .to_string_lossy()
            .eq_ignore_ascii_case(SIDECAR_FILENAME)
}

fn is_sidecar_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.eq_ignore_ascii_case(SIDECAR_FILENAME))
}

fn is_automatic_sidecar_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.eq_ignore_ascii_case(crate::sidecar::SIDECAR_FILENAME))
}

fn rating_kind_name(value: i64) -> Option<&'static str> {
    match value {
        0 => Some("image"),
        1 => Some("video"),
        2 => Some("folder"),
        3 => Some("zip_file"),
        4 => Some("pdf_file"),
        5 => Some("convertible_archive"),
        6 => Some("zip_image"),
        7 => Some("pdf_page"),
        8 => Some("zip_dir"),
        9 => Some("audio"),
        _ => None,
    }
}

fn rating_kind_value(value: &str) -> Option<i64> {
    match value {
        "image" => Some(0),
        "video" => Some(1),
        "folder" => Some(2),
        "zip_file" => Some(3),
        "pdf_file" => Some(4),
        "convertible_archive" => Some(5),
        "zip_image" => Some(6),
        "pdf_page" => Some(7),
        "zip_dir" => Some(8),
        "audio" => Some(9),
        _ => None,
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), TransferError> {
    if cancel.load(Ordering::Relaxed) {
        Err(TransferError::Cancelled)
    } else {
        Ok(())
    }
}

fn system_time_ms(value: SystemTime) -> Option<i64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
}

fn now_ms() -> i64 {
    system_time_ms(SystemTime::now()).unwrap_or(0)
}

fn db_error(error: rusqlite::Error) -> TransferError {
    TransferError::Database(error.to_string())
}

fn invalid_db_value(
    column: usize,
    value_type: rusqlite::types::Type,
    message: String,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        value_type,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        )),
    )
}

struct CancelReader<'a, R> {
    inner: R,
    cancel: &'a AtomicBool,
}

impl<R: Read> Read for CancelReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "metadata transfer cancelled",
            ));
        }
        self.inner.read(buffer)
    }
}

struct CancelWriter<'a, W> {
    inner: W,
    cancel: &'a AtomicBool,
    written: u64,
    max_bytes: u64,
}

impl<W: Write> Write for CancelWriter<'_, W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "metadata transfer cancelled",
            ));
        }
        let requested = u64::try_from(buffer.len()).unwrap_or(u64::MAX);
        if self.written.saturating_add(requested) > self.max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "metadata sidecar exceeds {} MiB",
                    self.max_bytes / 1024 / 1024
                ),
            ));
        }
        let written = self.inner.write(buffer)?;
        self.written = self
            .written
            .saturating_add(u64::try_from(written).unwrap_or(u64::MAX));
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_progress(_: TransferProgress) {}

    #[cfg(windows)]
    fn bundle_file_attributes(root: &Path) -> u32 {
        use std::os::windows::fs::MetadataExt;
        fs::metadata(root.join(SIDECAR_FILENAME))
            .unwrap()
            .file_attributes()
    }

    #[cfg(windows)]
    fn hidden_system_attribute_mask() -> u32 {
        use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM};
        FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0
    }

    #[cfg(windows)]
    #[test]
    fn new_export_publishes_bundle_without_hidden_or_system_attributes() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        init_data_dir(&data);

        export_at(&data, &root, false, &AtomicBool::new(false), no_progress).unwrap();

        assert_eq!(
            bundle_file_attributes(&root) & hidden_system_attribute_mask(),
            0
        );
    }

    #[cfg(windows)]
    #[test]
    fn reexport_clears_hidden_and_system_from_existing_bundle() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        init_data_dir(&data);
        let cancel = AtomicBool::new(false);
        export_at(&data, &root, false, &cancel, no_progress).unwrap();
        crate::sidecar::mark_hidden_system(&root.join(SIDECAR_FILENAME));
        assert_eq!(
            bundle_file_attributes(&root) & hidden_system_attribute_mask(),
            hidden_system_attribute_mask()
        );

        export_at(&data, &root, false, &cancel, no_progress).unwrap();

        assert_eq!(
            bundle_file_attributes(&root) & hidden_system_attribute_mask(),
            0
        );
    }

    #[test]
    fn page_state_validation_accepts_source_pixel_crop_coordinates() {
        let state = PortablePageState {
            export_crop: Some(crate::export_crop::CropSettings {
                rect: crate::export_crop::CropRect {
                    min_x: 25.0,
                    min_y: 40.0,
                    max_x: 1_600.0,
                    max_y: 900.0,
                },
                aspect_mode: crate::export_crop::CropAspectMode::Ratio16x9,
            }),
            ..PortablePageState::default()
        };

        validate_page_state(&state, "image.png")
            .expect("crop coordinates are stored in source-image pixels, not normalized 0..=1");
    }

    #[test]
    fn page_state_validation_rejects_unsafe_crop_coordinates() {
        for rect in [
            crate::export_crop::CropRect {
                min_x: f32::INFINITY,
                min_y: 0.0,
                max_x: 100.0,
                max_y: 100.0,
            },
            crate::export_crop::CropRect {
                min_x: -1.0,
                min_y: 0.0,
                max_x: 100.0,
                max_y: 100.0,
            },
            crate::export_crop::CropRect {
                min_x: 100.0,
                min_y: 0.0,
                max_x: 100.0,
                max_y: 100.0,
            },
        ] {
            let state = PortablePageState {
                export_crop: Some(crate::export_crop::CropSettings {
                    rect,
                    aspect_mode: crate::export_crop::CropAspectMode::Free,
                }),
                ..PortablePageState::default()
            };
            assert!(validate_page_state(&state, "image.png").is_err());
        }
    }

    fn copy_sidecar_bundle(source_root: &Path, destination_root: &Path) {
        fn copy_directory(source: &Path, destination: &Path) {
            fs::create_dir(destination).unwrap();
            for entry in fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                let source_path = entry.path();
                let destination_path = destination.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy_directory(&source_path, &destination_path);
                } else {
                    fs::copy(source_path, destination_path).unwrap();
                }
            }
        }

        copy_directory(
            &source_root.join(SIDECAR_FILENAME),
            &destination_root.join(SIDECAR_FILENAME),
        );
    }

    fn rewrite_root_shard_entries(
        root: &Path,
        cancel: &AtomicBool,
        mut rewrite: impl FnMut(&mut PortableEntry),
    ) {
        let manifest = read_bundle_manifest(root, cancel).unwrap();
        let shard = bundle_generation_dir(&root.join(SIDECAR_FILENAME), &manifest.generation)
            .join(SHARDS_DIRNAME)
            .join(shard_filename("."));
        let contents = fs::read_to_string(&shard).unwrap();
        let mut lines = contents.lines();
        let mut rewritten = String::new();
        rewritten.push_str(lines.next().unwrap());
        rewritten.push('\n');
        for line in lines {
            let mut entry: PortableEntry = serde_json::from_str(line).unwrap();
            rewrite(&mut entry);
            rewritten.push_str(&serde_json::to_string(&entry).unwrap());
            rewritten.push('\n');
        }
        fs::write(shard, rewritten).unwrap();
    }

    fn init_data_dir(path: &Path) {
        ensure_database_schemas(path).unwrap();
    }

    #[test]
    fn open_import_connection_names_held_tags_db_instead_of_raw_locked_error() {
        let temp = tempfile::TempDir::new().unwrap();
        init_data_dir(temp.path());
        let _held_tags = crate::tags_db::TagsDb::open_at(&temp.path().join("tags.db")).unwrap();

        let error = match open_import_connection(temp.path()) {
            Ok(_) => panic!("別接続がtags.dbを保持中ならjournal mode切替を拒否する"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("tags.db"), "対象DB名を示す: {error}");
        assert!(
            error.contains("他の接続"),
            "保持接続が原因だと示す: {error}"
        );
        assert!(
            error.contains("rollback journal"),
            "失敗した切替段階を示す: {error}"
        );
        assert!(
            !error.contains("database is locked"),
            "SQLiteの生エラーだけを利用者へ返さない: {error}"
        );
    }

    #[test]
    fn open_import_connection_succeeds_without_held_tags_db_connection() {
        let temp = tempfile::TempDir::new().unwrap();
        init_data_dir(temp.path());
        let connection = open_import_connection(temp.path())
            .expect("他のtags.db接続が無い通常状態ならimport connectionを開ける");
        drop(connection);
    }

    fn plain_portable_entry(path: &str, media_kind: PortableMediaKind) -> PortableEntry {
        PortableEntry {
            path: path.to_string(),
            kind: PortableEntryKind::File,
            media_kind,
            virtual_key_base: PortableVirtualKeyBase::Source,
            container_key_base: PortableVirtualKeyBase::Source,
            fingerprint: Some(FileFingerprint {
                size: 1,
                modified_ms: None,
            }),
            rating: None,
            tags: Vec::new(),
            tags_decided: false,
            timed_bookmarks: Vec::new(),
            book_bookmarks: Vec::new(),
            page_state: PortablePageState::default(),
            container_state: PortableContainerState::default(),
            nested_containers: Vec::new(),
            video_pin: None,
            virtual_items: Vec::new(),
        }
    }

    fn set_tag_state(data_dir: &Path, item_key: &str) {
        Connection::open(data_dir.join("tags.db"))
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO tag_item_state (item_key, decided_at, source)
                 VALUES (?1, 123, 'test')",
                [item_key],
            )
            .unwrap();
    }

    fn tag_state_exists(data_dir: &Path, item_key: &str) -> bool {
        Connection::open(data_dir.join("tags.db"))
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM tag_item_state WHERE item_key = ?1)",
                [item_key],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn set_rating(data_dir: &Path, path: &Path, stars: i64) {
        let conn = Connection::open(data_dir.join("rating.db")).unwrap();
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let kind = if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension.as_str()) {
            1
        } else {
            0
        };
        conn.execute(
            "INSERT OR REPLACE INTO ratings (path, stars, rated_at_ms, source_path, kind)
             VALUES (?1, ?2, 1234, ?3, ?4)",
            params![
                crate::path_key::normalize_keep_drive(path),
                stars,
                path.to_string_lossy().as_ref(),
                kind,
            ],
        )
        .unwrap();
    }

    fn rating(data_dir: &Path, path: &Path) -> Option<i64> {
        let conn = Connection::open(data_dir.join("rating.db")).unwrap();
        conn.query_row(
            "SELECT stars FROM ratings WHERE path = ?1",
            [crate::path_key::normalize_keep_drive(path)],
            |row| row.get(0),
        )
        .ok()
    }

    fn set_tags(data_dir: &Path, path: &Path, tags: &[&str]) {
        let mut db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")).unwrap();
        db.set_item_tags(
            &crate::path_key::normalize_keep_drive(path),
            tags,
            crate::tags_db::source::EDIT,
        )
        .unwrap();
    }

    fn set_tags_with_applied_at(data_dir: &Path, item_key: &str, tags: &[(&str, i64)]) {
        let conn = Connection::open(data_dir.join("tags.db")).unwrap();
        for (name, applied_at) in tags {
            let display = crate::tags_db::normalize_tag_display_name(name);
            conn.execute(
                "INSERT INTO item_tags (item_key, tag, tag_key, applied_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    item_key,
                    display,
                    crate::tags_db::normalize_tag_key(name),
                    applied_at
                ],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO tag_item_state (item_key, decided_at, source)
             VALUES (?1, ?2, ?3)",
            params![
                item_key,
                tags.iter()
                    .map(|(_, applied_at)| *applied_at)
                    .max()
                    .unwrap_or(0),
                crate::tags_db::source::EDIT
            ],
        )
        .unwrap();
    }

    fn tags_with_applied_at(data_dir: &Path, item_key: &str) -> Vec<(String, i64)> {
        let conn = Connection::open(data_dir.join("tags.db")).unwrap();
        let mut statement = conn
            .prepare(
                "SELECT tag, applied_at
                   FROM item_tags
                  WHERE item_key = ?1
                  ORDER BY applied_at ASC, tag COLLATE NOCASE ASC",
            )
            .unwrap();
        statement
            .query_map([item_key], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn tags(data_dir: &Path, path: &Path) -> Vec<String> {
        crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db"))
            .unwrap()
            .get_item_tags(&crate::path_key::normalize_keep_drive(path))
            .into_iter()
            .map(|tag| tag.tag)
            .collect()
    }

    fn manifest_with_bookmark(
        entry_kind: PortableEntryKind,
        container_kind: &str,
        page_kind: &str,
        page_value: &str,
    ) -> Manifest {
        let entry_path = match (entry_kind, container_kind) {
            (PortableEntryKind::Directory, _) => "book",
            (_, "pdf") => "book.pdf",
            (_, "other_archive") => "book.7z",
            _ => "book.zip",
        };
        Manifest {
            format: FORMAT_NAME.to_string(),
            version: FORMAT_VERSION,
            exported_at_ms: 0,
            recursive: false,
            sections: ManifestSections::default(),
            entries: vec![PortableEntry {
                path: entry_path.to_string(),
                kind: entry_kind,
                media_kind: match (entry_kind, container_kind) {
                    (PortableEntryKind::Directory, _) => PortableMediaKind::Directory,
                    (_, "pdf") => PortableMediaKind::Pdf,
                    (_, "other_archive") => PortableMediaKind::ConvertibleArchive,
                    _ => PortableMediaKind::Zip,
                },
                virtual_key_base: PortableVirtualKeyBase::Source,
                container_key_base: PortableVirtualKeyBase::Source,
                fingerprint: (entry_kind == PortableEntryKind::File).then_some(FileFingerprint {
                    size: 1,
                    modified_ms: None,
                }),
                rating: None,
                tags: Vec::new(),
                tags_decided: false,
                timed_bookmarks: Vec::new(),
                book_bookmarks: vec![PortableBookBookmark {
                    container_kind: container_kind.to_string(),
                    page_kind: page_kind.to_string(),
                    page_value: page_value.to_string(),
                    page_index_hint: 0,
                    created_at_ms: 0,
                    title: None,
                }],
                page_state: PortablePageState::default(),
                container_state: PortableContainerState::default(),
                nested_containers: Vec::new(),
                video_pin: None,
                virtual_items: Vec::new(),
            }],
        }
    }

    #[test]
    fn round_trip_maps_physical_and_virtual_metadata_to_new_root() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::create_dir_all(destination.join("nested")).unwrap();
        fs::write(source.join("a.jpg"), b"abc").unwrap();
        fs::write(destination.join("a.jpg"), b"abc").unwrap();
        fs::write(source.join("clip.mp4"), b"video").unwrap();
        fs::write(destination.join("clip.mp4"), b"video").unwrap();
        fs::write(source.join("nested/book.zip"), b"zip").unwrap();
        fs::write(destination.join("nested/book.zip"), b"zip").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);

        set_rating(&source_data, &source.join("a.jpg"), 4);
        let physical_source_key = crate::path_key::normalize_keep_drive(&source.join("a.jpg"));
        // 辞書順とは逆の古い→新しい順を作り、表示順と候補時刻の双方を検証する。
        set_tags_with_applied_at(
            &source_data,
            &physical_source_key,
            &[("Zulu", 100), ("Alpha", 300)],
        );
        let virtual_source_key = format!(
            "{}::page/001.jpg",
            crate::path_key::normalize_keep_drive(&source.join("nested/book.zip"))
        );
        let conn = Connection::open(source_data.join("rating.db")).unwrap();
        conn.execute(
            "INSERT INTO ratings (path, stars, rated_at_ms, source_path, kind, entry_name)
             VALUES (?1, 5, 5678, ?2, 6, 'page/001.jpg')",
            params![
                virtual_source_key,
                source.join("nested/book.zip").to_string_lossy().as_ref()
            ],
        )
        .unwrap();
        set_tags_with_applied_at(
            &source_data,
            &virtual_source_key,
            &[("VirtualZulu", 200), ("VirtualAlpha", 500)],
        );
        let video = Connection::open(source_data.join("video_bookmarks.db")).unwrap();
        video
            .execute(
                "INSERT INTO video_bookmarks (path, pts_secs, title, created_at)
                 VALUES (?1, 12.5, '場面', 99)",
                [crate::path_key::normalize_keep_drive(
                    &source.join("clip.mp4"),
                )],
            )
            .unwrap();
        let book = Connection::open(source_data.join("book_bookmarks.db")).unwrap();
        let book_path = source.join("nested/book.zip");
        book.execute(
            "INSERT INTO book_bookmarks
                (container_key, container_path, container_kind, page_kind, page_value,
                 page_key, page_index_hint, created_at_ms, title)
             VALUES (?1, ?2, 'zip', 'archive_entry', 'page/001.jpg',
                     'page/001.jpg', 0, 777, '表紙')",
            params![
                crate::path_key::normalize_keep_drive(&book_path),
                book_path.to_string_lossy().as_ref()
            ],
        )
        .unwrap();

        let cancel = AtomicBool::new(false);
        let exported = export_at(&source_data, &source, true, &cancel, no_progress).unwrap();
        assert_eq!(exported.ratings, 2);
        copy_sidecar_bundle(&source, &destination);
        // 実アプリと同様、rollback-journal DB のアイドル接続が開いたままでも別 worker
        // 接続から attached transaction を実行できることを確認する。WAL の tags.db
        // connection は UI から worker へ移して閉じた後に import_at を呼ぶ。
        let _open_rating =
            crate::rating_db::RatingDb::open_at(destination_data.join("rating.db")).unwrap();
        let _open_video = crate::video_bookmarks::VideoBookmarkDb::open_at(
            &destination_data.join("video_bookmarks.db"),
        )
        .unwrap();
        let _open_book = Connection::open(destination_data.join("book_bookmarks.db")).unwrap();
        let imported = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(imported.failed_entries, 0);
        assert_eq!(
            rating(&destination_data, &destination.join("a.jpg")),
            Some(4)
        );
        assert_eq!(
            tags(&destination_data, &destination.join("a.jpg")),
            vec!["Zulu", "Alpha"]
        );
        let physical_destination_key =
            crate::path_key::normalize_keep_drive(&destination.join("a.jpg"));
        assert_eq!(
            tags_with_applied_at(&destination_data, &physical_destination_key),
            vec![("Zulu".to_string(), 100), ("Alpha".to_string(), 300)]
        );

        let virtual_destination_key = format!(
            "{}::page/001.jpg",
            crate::path_key::normalize_keep_drive(&destination.join("nested/book.zip"))
        );
        let conn = Connection::open(destination_data.join("rating.db")).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT stars FROM ratings WHERE path = ?1",
                [&virtual_destination_key],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            5
        );
        assert_eq!(
            tags_with_applied_at(&destination_data, &virtual_destination_key),
            vec![
                ("VirtualZulu".to_string(), 200),
                ("VirtualAlpha".to_string(), 500),
            ]
        );
        let destination_tags =
            crate::tags_db::TagsDb::open_at(&destination_data.join("tags.db")).unwrap();
        assert_eq!(
            destination_tags
                .find_exact("Alpha")
                .expect("physical tag summary")
                .last_applied_at,
            300
        );
        assert_eq!(
            destination_tags
                .find_exact("VirtualAlpha")
                .expect("virtual tag summary")
                .last_applied_at,
            500
        );
        assert_eq!(
            destination_tags
                .find_by_prefix("", 10)
                .into_iter()
                .map(|summary| (summary.tag, summary.last_applied_at))
                .collect::<Vec<_>>(),
            vec![
                ("VirtualAlpha".to_string(), 500),
                ("Alpha".to_string(), 300),
                ("VirtualZulu".to_string(), 200),
                ("Zulu".to_string(), 100),
            ],
            "recent-tag candidates must retain their pre-transfer ordering"
        );
        let video = Connection::open(destination_data.join("video_bookmarks.db")).unwrap();
        let video_row = video
            .query_row(
                "SELECT path, pts_secs, title, created_at FROM video_bookmarks",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, f64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            video_row,
            (
                crate::path_key::normalize_keep_drive(&destination.join("clip.mp4")),
                12.5,
                "場面".to_string(),
                99,
            )
        );
        let book = Connection::open(destination_data.join("book_bookmarks.db")).unwrap();
        let book_row = book
            .query_row(
                "SELECT container_key, container_path, page_value, title FROM book_bookmarks",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            book_row,
            (
                crate::path_key::normalize_keep_drive(&destination.join("nested/book.zip")),
                destination
                    .join("nested/book.zip")
                    .to_string_lossy()
                    .into_owned(),
                "page/001.jpg".to_string(),
                "表紙".to_string(),
            )
        );
    }

    #[test]
    fn timed_bookmark_thumbnails_round_trip_with_null_preserved() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        let source_video = source.join("clip.mp4");
        let destination_video = destination.join("clip.mp4");
        fs::write(&source_video, b"same-video").unwrap();
        fs::write(&destination_video, b"same-video").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);

        let source_key = crate::path_key::normalize_keep_drive(&source_video);
        Connection::open(source_data.join("video_bookmarks.db"))
            .unwrap()
            .execute(
                "INSERT INTO video_bookmarks
                    (path, pts_secs, title, thumb_webp, created_at)
                 VALUES (?1, 1.5, 'thumb', ?2, 10),
                        (?1, 3.0, 'null', NULL, 20)",
                params![source_key, b"webp-bookmark-thumb".as_slice()],
            )
            .unwrap();

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        let manifest = read_manifest(&source, &cancel).unwrap();
        let bookmarks = &manifest
            .entries
            .iter()
            .find(|entry| entry.path == "clip.mp4")
            .unwrap()
            .timed_bookmarks;
        assert_eq!(bookmarks.len(), 2);
        assert!(bookmarks[0].thumb_webp_base64.is_some());
        assert!(bookmarks[1].thumb_webp_base64.is_none());

        copy_sidecar_bundle(&source, &destination);
        let summary = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(summary.failed_entries, 0);
        let destination_key = crate::path_key::normalize_keep_drive(&destination_video);
        let conn = Connection::open(destination_data.join("video_bookmarks.db")).unwrap();
        let mut statement = conn
            .prepare(
                "SELECT thumb_webp FROM video_bookmarks
                  WHERE path = ?1 ORDER BY pts_secs",
            )
            .unwrap();
        let thumbs = statement
            .query_map([destination_key], |row| row.get::<_, Option<Vec<u8>>>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(thumbs, vec![Some(b"webp-bookmark-thumb".to_vec()), None]);
    }

    #[test]
    fn physical_tag_decision_state_round_trips_without_suppressing_undecided_seed() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        for name in ["decided-empty.jpg", "undecided.jpg", "tagged.jpg"] {
            fs::write(source.join(name), name.as_bytes()).unwrap();
            fs::write(destination.join(name), name.as_bytes()).unwrap();
        }
        init_data_dir(&source_data);
        init_data_dir(&destination_data);

        let source_decided =
            crate::path_key::normalize_keep_drive(&source.join("decided-empty.jpg"));
        let source_tagged = crate::path_key::normalize_keep_drive(&source.join("tagged.jpg"));
        set_tag_state(&source_data, &source_decided);
        set_tags_with_applied_at(&source_data, &source_tagged, &[("new-tag", 456)]);
        for name in ["decided-empty.jpg", "undecided.jpg", "tagged.jpg"] {
            let key = crate::path_key::normalize_keep_drive(&destination.join(name));
            set_tags_with_applied_at(&destination_data, &key, &[("stale", 1)]);
        }

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        let manifest = read_manifest(&source, &cancel).unwrap();
        for (path, decided) in [
            ("decided-empty.jpg", true),
            ("undecided.jpg", false),
            ("tagged.jpg", true),
        ] {
            assert_eq!(
                manifest
                    .entries
                    .iter()
                    .find(|entry| entry.path == path)
                    .unwrap()
                    .tags_decided,
                decided,
                "unexpected tag decision state for {path}"
            );
        }

        copy_sidecar_bundle(&source, &destination);
        let summary = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(summary.failed_entries, 0);
        let destination_decided =
            crate::path_key::normalize_keep_drive(&destination.join("decided-empty.jpg"));
        let destination_undecided =
            crate::path_key::normalize_keep_drive(&destination.join("undecided.jpg"));
        let destination_tagged =
            crate::path_key::normalize_keep_drive(&destination.join("tagged.jpg"));
        assert!(tags(&destination_data, &destination.join("decided-empty.jpg")).is_empty());
        assert!(tag_state_exists(&destination_data, &destination_decided));
        assert!(tags(&destination_data, &destination.join("undecided.jpg")).is_empty());
        assert!(!tag_state_exists(&destination_data, &destination_undecided));
        assert_eq!(
            tags(&destination_data, &destination.join("tagged.jpg")),
            vec!["new-tag"]
        );
        assert!(tag_state_exists(&destination_data, &destination_tagged));
    }

    #[test]
    fn zip_and_pdf_virtual_tag_decision_state_round_trips() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        for name in ["book.zip", "book.pdf"] {
            fs::write(source.join(name), name.as_bytes()).unwrap();
            fs::write(destination.join(name), name.as_bytes()).unwrap();
        }
        init_data_dir(&source_data);
        init_data_dir(&destination_data);

        let zip_members = ["decided.jpg", "undecided.jpg", "tagged.jpg"];
        let pdf_members = ["page_0", "page_1", "page_2"];
        let source_zip_base = crate::path_key::normalize_keep_drive(&source.join("book.zip"));
        let source_pdf_base = crate::path_key::normalize_keep_drive(&source.join("book.pdf"));
        let source_zip_keys =
            zip_members.map(|member| canonical_virtual_item_key(&source_zip_base, member));
        let source_pdf_keys =
            pdf_members.map(|member| canonical_virtual_item_key(&source_pdf_base, member));
        let ratings = Connection::open(source_data.join("rating.db")).unwrap();
        for (key, member) in source_zip_keys.iter().zip(zip_members) {
            ratings
                .execute(
                    "INSERT INTO ratings (path, stars, kind, entry_name)
                     VALUES (?1, 4, 6, ?2)",
                    params![key, member],
                )
                .unwrap();
        }
        for (page_num, key) in source_pdf_keys.iter().enumerate() {
            ratings
                .execute(
                    "INSERT INTO ratings (path, stars, kind, page_num)
                     VALUES (?1, 4, 7, ?2)",
                    params![key, page_num as i64],
                )
                .unwrap();
        }
        set_tag_state(&source_data, &source_zip_keys[0]);
        set_tag_state(&source_data, &source_pdf_keys[0]);
        set_tags_with_applied_at(&source_data, &source_zip_keys[2], &[("zip-tag", 300)]);
        set_tags_with_applied_at(&source_data, &source_pdf_keys[2], &[("pdf-tag", 400)]);

        let destination_zip_base =
            crate::path_key::normalize_keep_drive(&destination.join("book.zip"));
        let destination_pdf_base =
            crate::path_key::normalize_keep_drive(&destination.join("book.pdf"));
        let destination_zip_keys =
            zip_members.map(|member| canonical_virtual_item_key(&destination_zip_base, member));
        let destination_pdf_keys =
            pdf_members.map(|member| canonical_virtual_item_key(&destination_pdf_base, member));
        for key in destination_zip_keys
            .iter()
            .chain(destination_pdf_keys.iter())
        {
            set_tags_with_applied_at(&destination_data, key, &[("stale", 1)]);
        }

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        let manifest = read_manifest(&source, &cancel).unwrap();
        for (path, members) in [("book.zip", &zip_members), ("book.pdf", &pdf_members)] {
            let entry = manifest
                .entries
                .iter()
                .find(|entry| entry.path == path)
                .unwrap();
            for (member, decided) in members.iter().zip([true, false, true]) {
                assert_eq!(
                    entry
                        .virtual_items
                        .iter()
                        .find(|item| item.member_key == *member)
                        .unwrap()
                        .tags_decided,
                    decided,
                    "unexpected tag decision state for {path}::{member}"
                );
            }
        }

        copy_sidecar_bundle(&source, &destination);
        let summary = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(summary.failed_entries, 0);
        for (keys, expected_tag) in [
            (&destination_zip_keys, "zip-tag"),
            (&destination_pdf_keys, "pdf-tag"),
        ] {
            assert!(tags_with_applied_at(&destination_data, &keys[0]).is_empty());
            assert!(tag_state_exists(&destination_data, &keys[0]));
            assert!(tags_with_applied_at(&destination_data, &keys[1]).is_empty());
            assert!(!tag_state_exists(&destination_data, &keys[1]));
            assert_eq!(
                tags_with_applied_at(&destination_data, &keys[2]),
                vec![(
                    expected_tag.to_string(),
                    if expected_tag == "zip-tag" { 300 } else { 400 }
                )]
            );
            assert!(tag_state_exists(&destination_data, &keys[2]));
        }
    }

    #[test]
    fn converted_archive_pages_round_trip_via_environment_local_cache_key() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        let source_archive = source.join("book.7z");
        let destination_archive = destination.join("book.7z");
        fs::write(&source_archive, b"same-archive").unwrap();
        fs::write(&destination_archive, b"same-archive").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);

        let source_cache =
            crate::archive_cache::cache_zip_path_for_data_dir(&source_data, &source_archive);
        fs::create_dir_all(source_cache.parent().unwrap()).unwrap();
        fs::write(&source_cache, b"converted-zip").unwrap();
        let source_metadata = fs::metadata(&source_archive).unwrap();
        let source_mtime = source_metadata
            .modified()
            .ok()
            .and_then(system_time_ms)
            .unwrap()
            .div_euclid(1000);
        let archive_cache = Connection::open(source_data.join("archive_cache.db")).unwrap();
        archive_cache
            .execute_batch(
                "CREATE TABLE converted_archives (
                    src_path_key TEXT PRIMARY KEY,
                    src_mtime INTEGER NOT NULL,
                    src_size INTEGER NOT NULL,
                    cached_zip_path TEXT NOT NULL
                 );",
            )
            .unwrap();
        archive_cache
            .execute(
                "INSERT INTO converted_archives
                    (src_path_key, src_mtime, src_size, cached_zip_path)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    crate::path_key::normalize(&source_archive),
                    source_mtime,
                    source_metadata.len() as i64,
                    source_cache.to_string_lossy().as_ref(),
                ],
            )
            .unwrap();

        let source_page = canonical_virtual_item_key(
            &crate::path_key::normalize_keep_drive(&source_cache),
            "Pages/Cover.JPG",
        );
        Connection::open(source_data.join("rating.db"))
            .unwrap()
            .execute(
                "INSERT INTO ratings
                    (path, stars, rated_at_ms, source_path, kind, entry_name)
                 VALUES (?1, 5, 123, ?2, 6, 'Pages/Cover.JPG')",
                params![source_page, source_archive.to_string_lossy().as_ref()],
            )
            .unwrap();
        Connection::open(source_data.join("rotation.db"))
            .unwrap()
            .execute(
                "INSERT INTO rotations (path, angle) VALUES (?1, 270)",
                [&source_page],
            )
            .unwrap();

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        let exported = read_manifest(&source, &cancel).unwrap();
        let archive_entry = exported
            .entries
            .iter()
            .find(|entry| entry.path == "book.7z")
            .unwrap();
        assert_eq!(
            archive_entry.virtual_key_base,
            PortableVirtualKeyBase::ConvertedCache
        );
        assert_eq!(archive_entry.virtual_items.len(), 1);

        copy_sidecar_bundle(&source, &destination);
        let summary = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(summary.failed_entries, 0);
        let destination_cache = crate::archive_cache::cache_zip_path_for_data_dir(
            &destination_data,
            &destination_archive,
        );
        let destination_page = canonical_virtual_item_key(
            &crate::path_key::normalize_keep_drive(&destination_cache),
            "Pages/Cover.JPG",
        );
        assert_eq!(
            Connection::open(destination_data.join("rating.db"))
                .unwrap()
                .query_row(
                    "SELECT stars FROM ratings WHERE path = ?1",
                    [&destination_page],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            5
        );
        assert_eq!(
            Connection::open(destination_data.join("rotation.db"))
                .unwrap()
                .query_row(
                    "SELECT angle FROM rotations WHERE path = ?1",
                    [&destination_page],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            270
        );
    }

    #[test]
    fn converted_archive_container_states_round_trip_via_environment_local_cache_key() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        let source_archive = source.join("book.rar");
        let destination_archive = destination.join("book.rar");
        fs::write(&source_archive, b"same-rar").unwrap();
        fs::write(&destination_archive, b"same-rar").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);

        let source_cache_key = crate::path_key::normalize(
            &crate::archive_cache::cache_zip_path_for_data_dir(&source_data, &source_archive),
        );
        let source_nested_key = join_container_key(&source_cache_key, "vol01");
        let trim = crate::view_trim::ViewTrimBookState {
            apply_mode: crate::view_trim::ViewTrimApplyMode::Book,
            book_settings: crate::view_trim::ViewTrimBookSettings {
                enabled: true,
                single: crate::view_trim::ViewTrimMargins {
                    left: 0.01,
                    top: 0.02,
                    right: 0.03,
                    bottom: 0.04,
                },
                ..Default::default()
            },
        };
        let trim_json = serde_json::to_string(&trim).unwrap();
        let spread = Connection::open(source_data.join("spread.db")).unwrap();
        spread
            .execute(
                "INSERT INTO spreads (path, mode, flow, direction) VALUES (?1, 2, 1, 1), (?2, 3, 2, 0)",
                params![source_cache_key, source_nested_key],
            )
            .unwrap();
        let view_trim = Connection::open(source_data.join("view_trim.db")).unwrap();
        view_trim
            .execute(
                "INSERT INTO view_trim_books (book_key, state_json) VALUES (?1, ?3), (?2, ?3)",
                params![source_cache_key, source_nested_key, trim_json],
            )
            .unwrap();

        let cancel = AtomicBool::new(false);
        let exported = export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        assert_eq!(exported.container_states, 2);
        let manifest = read_manifest(&source, &cancel).unwrap();
        let archive_entry = manifest
            .entries
            .iter()
            .find(|entry| entry.path == "book.rar")
            .unwrap();
        assert_eq!(
            archive_entry.container_key_base,
            PortableVirtualKeyBase::ConvertedCache
        );
        assert!(archive_entry.container_state.spread.is_some());
        assert!(archive_entry.container_state.view_trim.is_some());
        assert_eq!(archive_entry.nested_containers.len(), 1);

        let destination_source_key = crate::path_key::normalize(&destination_archive);
        let destination_cache_key =
            crate::path_key::normalize(&crate::archive_cache::cache_zip_path_for_data_dir(
                &destination_data,
                &destination_archive,
            ));
        let old_trim_json =
            serde_json::to_string(&crate::view_trim::ViewTrimBookState::default()).unwrap();
        let destination_spread = Connection::open(destination_data.join("spread.db")).unwrap();
        for base in [&destination_source_key, &destination_cache_key] {
            destination_spread
                .execute(
                    "INSERT INTO spreads (path, mode, flow, direction) VALUES (?1, 1, 1, 0), (?2, 1, 1, 0)",
                    params![base, join_container_key(base, "vol01")],
                )
                .unwrap();
        }
        let destination_trim = Connection::open(destination_data.join("view_trim.db")).unwrap();
        for base in [&destination_source_key, &destination_cache_key] {
            destination_trim
                .execute(
                    "INSERT INTO view_trim_books (book_key, state_json) VALUES (?1, ?3), (?2, ?3)",
                    params![base, join_container_key(base, "vol01"), old_trim_json],
                )
                .unwrap();
        }

        copy_sidecar_bundle(&source, &destination);
        let imported = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(imported.failed_entries, 0);
        assert_eq!(imported.skipped_kind_mismatch, 0);
        let destination_nested_key = join_container_key(&destination_cache_key, "vol01");
        assert_eq!(
            destination_spread
                .query_row(
                    "SELECT mode, flow, direction FROM spreads WHERE path = ?1",
                    [&destination_cache_key],
                    |row| Ok((
                        row.get::<_, i32>(0)?,
                        row.get::<_, i32>(1)?,
                        row.get::<_, i32>(2)?
                    ))
                )
                .unwrap(),
            (2, 1, 1)
        );
        assert_eq!(
            destination_spread
                .query_row(
                    "SELECT mode, flow, direction FROM spreads WHERE path = ?1",
                    [&destination_nested_key],
                    |row| Ok((
                        row.get::<_, i32>(0)?,
                        row.get::<_, i32>(1)?,
                        row.get::<_, i32>(2)?
                    ))
                )
                .unwrap(),
            (3, 2, 0)
        );
        assert_eq!(
            destination_spread
                .query_row(
                    "SELECT COUNT(*) FROM spreads WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')",
                    [&destination_source_key],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            destination_trim
                .query_row(
                    "SELECT COUNT(*) FROM view_trim_books WHERE book_key = ?1 OR (book_key >= ?1 || '/' AND book_key < ?1 || '0')",
                    [&destination_source_key],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        for key in [&destination_cache_key, &destination_nested_key] {
            assert_eq!(
                destination_trim
                    .query_row(
                        "SELECT state_json FROM view_trim_books WHERE book_key = ?1",
                        [key],
                        |row| row.get::<_, String>(0)
                    )
                    .unwrap(),
                trim_json
            );
        }
    }

    #[test]
    fn container_state_export_rejects_source_and_cache_origin_conflict() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("book.rar");
        fs::write(&archive, b"rar").unwrap();
        init_data_dir(&data);
        let source_key = crate::path_key::normalize(&archive);
        let cache_key = crate::path_key::normalize(
            &crate::archive_cache::cache_zip_path_for_data_dir(&data, &archive),
        );
        Connection::open(data.join("spread.db"))
            .unwrap()
            .execute(
                "INSERT INTO spreads (path, mode, flow, direction) VALUES (?1, 2, 1, 1)",
                [&source_key],
            )
            .unwrap();
        Connection::open(data.join("view_trim.db"))
            .unwrap()
            .execute(
                "INSERT INTO view_trim_books (book_key, state_json) VALUES (?1, ?2)",
                params![
                    cache_key,
                    serde_json::to_string(&crate::view_trim::ViewTrimBookState::default()).unwrap()
                ],
            )
            .unwrap();

        let error =
            export_at(&data, &root, false, &AtomicBool::new(false), no_progress).unwrap_err();
        assert!(matches!(
            error,
            TransferError::Database(message)
                if message.contains("source key")
                    && message.contains("cache key")
                    && message.contains("book.rar")
                    && message.contains("直接閲覧と変換キャッシュの両方で編集")
        ));
    }

    #[test]
    fn zip_and_directory_container_states_round_trip_via_source_keys() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(source.join("folder-book")).unwrap();
        fs::create_dir_all(destination.join("folder-book")).unwrap();
        fs::write(source.join("folder-book/001.jpg"), b"page").unwrap();
        fs::write(destination.join("folder-book/001.jpg"), b"page").unwrap();
        fs::write(source.join("book.zip"), b"zip").unwrap();
        fs::write(destination.join("book.zip"), b"zip").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);

        let source_folder_key = crate::path_key::normalize(&source.join("folder-book"));
        let source_zip_key = crate::path_key::normalize(&source.join("book.zip"));
        Connection::open(source_data.join("spread.db"))
            .unwrap()
            .execute(
                "INSERT INTO spreads (path, mode, flow, direction) VALUES (?1, 4, 2, 0), (?2, 2, 1, 1)",
                params![source_folder_key, source_zip_key],
            )
            .unwrap();
        let trim_json =
            serde_json::to_string(&crate::view_trim::ViewTrimBookState::default()).unwrap();
        Connection::open(source_data.join("view_trim.db"))
            .unwrap()
            .execute(
                "INSERT INTO view_trim_books (book_key, state_json) VALUES (?1, ?2)",
                params![source_zip_key, trim_json],
            )
            .unwrap();

        let cancel = AtomicBool::new(false);
        let exported = export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        assert_eq!(exported.container_states, 2);
        let manifest = read_manifest(&source, &cancel).unwrap();
        for path in ["folder-book", "book.zip"] {
            assert_eq!(
                manifest
                    .entries
                    .iter()
                    .find(|entry| entry.path == path)
                    .unwrap()
                    .container_key_base,
                PortableVirtualKeyBase::Source
            );
        }

        copy_sidecar_bundle(&source, &destination);
        let imported = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(imported.failed_entries, 0);
        let destination_folder_key = crate::path_key::normalize(&destination.join("folder-book"));
        let destination_zip_key = crate::path_key::normalize(&destination.join("book.zip"));
        let spread = Connection::open(destination_data.join("spread.db")).unwrap();
        assert_eq!(
            spread
                .query_row(
                    "SELECT mode, flow, direction FROM spreads WHERE path = ?1",
                    [&destination_folder_key],
                    |row| Ok((
                        row.get::<_, i32>(0)?,
                        row.get::<_, i32>(1)?,
                        row.get::<_, i32>(2)?
                    ))
                )
                .unwrap(),
            (4, 2, 0)
        );
        assert_eq!(
            spread
                .query_row(
                    "SELECT mode, flow, direction FROM spreads WHERE path = ?1",
                    [&destination_zip_key],
                    |row| Ok((
                        row.get::<_, i32>(0)?,
                        row.get::<_, i32>(1)?,
                        row.get::<_, i32>(2)?
                    ))
                )
                .unwrap(),
            (2, 1, 1)
        );
        let destination_cache_key =
            crate::path_key::normalize(&crate::archive_cache::cache_zip_path_for_data_dir(
                &destination_data,
                &destination.join("book.zip"),
            ));
        assert_eq!(
            spread
                .query_row(
                    "SELECT COUNT(*) FROM spreads WHERE path = ?1",
                    [&destination_cache_key],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            Connection::open(destination_data.join("view_trim.db"))
                .unwrap()
                .query_row(
                    "SELECT state_json FROM view_trim_books WHERE book_key = ?1",
                    [&destination_zip_key],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            trim_json
        );
    }

    #[test]
    fn converted_archive_pages_survive_cache_prune_before_export() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        let source_archive = source.join("book.7z");
        let destination_archive = destination.join("book.7z");
        fs::write(&source_archive, b"same-archive").unwrap();
        fs::write(&destination_archive, b"same-archive").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);

        let source_cache =
            crate::archive_cache::cache_zip_path_for_data_dir(&source_data, &source_archive);
        fs::create_dir_all(source_cache.parent().unwrap()).unwrap();
        fs::write(&source_cache, b"converted-zip").unwrap();
        let source_metadata = fs::metadata(&source_archive).unwrap();
        let source_mtime = source_metadata
            .modified()
            .ok()
            .and_then(system_time_ms)
            .unwrap()
            .div_euclid(1000);
        let archive_cache = Connection::open(source_data.join("archive_cache.db")).unwrap();
        archive_cache
            .execute_batch(
                "CREATE TABLE converted_archives (
                    src_path_key TEXT PRIMARY KEY,
                    src_mtime INTEGER NOT NULL,
                    src_size INTEGER NOT NULL,
                    cached_zip_path TEXT NOT NULL
                 );",
            )
            .unwrap();
        archive_cache
            .execute(
                "INSERT INTO converted_archives
                    (src_path_key, src_mtime, src_size, cached_zip_path)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    crate::path_key::normalize(&source_archive),
                    source_mtime,
                    source_metadata.len() as i64,
                    source_cache.to_string_lossy().as_ref(),
                ],
            )
            .unwrap();

        let source_page = canonical_virtual_item_key(
            &crate::path_key::normalize_keep_drive(&source_cache),
            "Pages/Cover.JPG",
        );
        Connection::open(source_data.join("rating.db"))
            .unwrap()
            .execute(
                "INSERT INTO ratings
                    (path, stars, rated_at_ms, source_path, kind, entry_name)
                 VALUES (?1, 5, 123, ?2, 6, 'Pages/Cover.JPG')",
                params![source_page, source_archive.to_string_lossy().as_ref()],
            )
            .unwrap();
        Connection::open(source_data.join("rotation.db"))
            .unwrap()
            .execute(
                "INSERT INTO rotations (path, angle) VALUES (?1, 270)",
                [&source_page],
            )
            .unwrap();

        archive_cache
            .execute(
                "DELETE FROM converted_archives WHERE src_path_key = ?1",
                [crate::path_key::normalize(&source_archive)],
            )
            .unwrap();
        drop(archive_cache);
        fs::remove_file(&source_cache).unwrap();

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        let exported = read_manifest(&source, &cancel).unwrap();
        let archive_entry = exported
            .entries
            .iter()
            .find(|entry| entry.path == "book.7z")
            .unwrap();
        assert_eq!(
            archive_entry.virtual_key_base,
            PortableVirtualKeyBase::ConvertedCache
        );
        assert_eq!(archive_entry.virtual_items.len(), 1);

        copy_sidecar_bundle(&source, &destination);
        let summary = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(summary.failed_entries, 0);
        let destination_cache = crate::archive_cache::cache_zip_path_for_data_dir(
            &destination_data,
            &destination_archive,
        );
        let destination_page = canonical_virtual_item_key(
            &crate::path_key::normalize_keep_drive(&destination_cache),
            "Pages/Cover.JPG",
        );
        assert_eq!(
            Connection::open(destination_data.join("rating.db"))
                .unwrap()
                .query_row(
                    "SELECT stars FROM ratings WHERE path = ?1",
                    [&destination_page],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            5
        );
        assert_eq!(
            Connection::open(destination_data.join("rotation.db"))
                .unwrap()
                .query_row(
                    "SELECT angle FROM rotations WHERE path = ?1",
                    [&destination_page],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            270
        );
    }

    #[test]
    fn direct_read_rar_source_page_metadata_round_trips_without_cache_remap() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        let source_archive = source.join("book.rar");
        let destination_archive = destination.join("book.rar");
        fs::write(&source_archive, b"same-rar").unwrap();
        fs::write(&destination_archive, b"same-rar").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);

        // 有効な変換cache行が残っていても、実際にsource keyで観測したページ情報を優先する。
        let source_cache =
            crate::archive_cache::cache_zip_path_for_data_dir(&source_data, &source_archive);
        fs::create_dir_all(source_cache.parent().unwrap()).unwrap();
        fs::write(&source_cache, b"converted-zip").unwrap();
        let source_metadata = fs::metadata(&source_archive).unwrap();
        let source_mtime = source_metadata
            .modified()
            .ok()
            .and_then(system_time_ms)
            .unwrap()
            .div_euclid(1000);
        let archive_cache = Connection::open(source_data.join("archive_cache.db")).unwrap();
        archive_cache
            .execute_batch(
                "CREATE TABLE converted_archives (
                    src_path_key TEXT PRIMARY KEY,
                    src_mtime INTEGER NOT NULL,
                    src_size INTEGER NOT NULL,
                    cached_zip_path TEXT NOT NULL
                 );",
            )
            .unwrap();
        archive_cache
            .execute(
                "INSERT INTO converted_archives
                    (src_path_key, src_mtime, src_size, cached_zip_path)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    crate::path_key::normalize(&source_archive),
                    source_mtime,
                    source_metadata.len() as i64,
                    source_cache.to_string_lossy().as_ref(),
                ],
            )
            .unwrap();

        let source_page = canonical_virtual_item_key(
            &crate::path_key::normalize_keep_drive(&source_archive),
            "Pages/Cover.JPG",
        );
        Connection::open(source_data.join("rating.db"))
            .unwrap()
            .execute(
                "INSERT INTO ratings
                    (path, stars, rated_at_ms, source_path, kind, entry_name)
                 VALUES (?1, 4, 123, ?2, 6, 'Pages/Cover.JPG')",
                params![source_page, source_archive.to_string_lossy().as_ref()],
            )
            .unwrap();
        set_tags_with_applied_at(&source_data, &source_page, &[("直読み", 456)]);
        Connection::open(source_data.join("rotation.db"))
            .unwrap()
            .execute(
                "INSERT INTO rotations (path, angle) VALUES (?1, 90)",
                [&source_page],
            )
            .unwrap();

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        let exported = read_manifest(&source, &cancel).unwrap();
        let archive_entry = exported
            .entries
            .iter()
            .find(|entry| entry.path == "book.rar")
            .unwrap();
        assert_eq!(
            archive_entry.virtual_key_base,
            PortableVirtualKeyBase::Source
        );
        assert_eq!(archive_entry.virtual_items.len(), 1);

        let destination_cache = crate::archive_cache::cache_zip_path_for_data_dir(
            &destination_data,
            &destination_archive,
        );
        let destination_cache_page = canonical_virtual_item_key(
            &crate::path_key::normalize_keep_drive(&destination_cache),
            "Pages/Cover.JPG",
        );
        Connection::open(destination_data.join("rating.db"))
            .unwrap()
            .execute(
                "INSERT INTO ratings
                    (path, stars, rated_at_ms, source_path, kind, entry_name)
                 VALUES (?1, 1, 1, ?2, 6, 'Pages/Cover.JPG')",
                params![
                    destination_cache_page,
                    destination_archive.to_string_lossy().as_ref()
                ],
            )
            .unwrap();
        set_tags_with_applied_at(
            &destination_data,
            &destination_cache_page,
            &[("旧cache", 1)],
        );
        Connection::open(destination_data.join("rotation.db"))
            .unwrap()
            .execute(
                "INSERT INTO rotations (path, angle) VALUES (?1, 270)",
                [&destination_cache_page],
            )
            .unwrap();

        copy_sidecar_bundle(&source, &destination);
        let summary = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(summary.failed_entries, 0);
        let destination_page = canonical_virtual_item_key(
            &crate::path_key::normalize_keep_drive(&destination_archive),
            "Pages/Cover.JPG",
        );
        assert_eq!(
            Connection::open(destination_data.join("rating.db"))
                .unwrap()
                .query_row(
                    "SELECT stars FROM ratings WHERE path = ?1",
                    [&destination_page],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            4
        );
        assert_eq!(
            tags_with_applied_at(&destination_data, &destination_page),
            vec![("直読み".to_string(), 456)]
        );
        assert_eq!(
            Connection::open(destination_data.join("rotation.db"))
                .unwrap()
                .query_row(
                    "SELECT angle FROM rotations WHERE path = ?1",
                    [&destination_page],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            90
        );

        assert_eq!(
            Connection::open(destination_data.join("rating.db"))
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM ratings WHERE path = ?1",
                    [&destination_cache_page],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert!(tags_with_applied_at(&destination_data, &destination_cache_page).is_empty());
        assert_eq!(
            Connection::open(destination_data.join("tags.db"))
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM tag_item_state WHERE item_key = ?1",
                    [&destination_cache_page],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            Connection::open(destination_data.join("rotation.db"))
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM rotations WHERE path = ?1",
                    [&destination_cache_page],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn converted_archive_source_and_cache_page_metadata_are_a_conflict() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("book.7z");
        fs::write(&archive, b"archive").unwrap();
        init_data_dir(&data);

        let source_page = canonical_virtual_item_key(
            &crate::path_key::normalize_keep_drive(&archive),
            "page.jpg",
        );
        let cache = crate::archive_cache::cache_zip_path_for_data_dir(&data, &archive);
        let cache_page =
            canonical_virtual_item_key(&crate::path_key::normalize_keep_drive(&cache), "page.jpg");
        Connection::open(data.join("rating.db"))
            .unwrap()
            .execute(
                "INSERT INTO ratings (path, stars, kind, entry_name)
                 VALUES (?1, 5, 6, 'page.jpg')",
                [&source_page],
            )
            .unwrap();
        Connection::open(data.join("rotation.db"))
            .unwrap()
            .execute(
                "INSERT INTO rotations (path, angle) VALUES (?1, 90)",
                [&cache_page],
            )
            .unwrap();

        let cancel = AtomicBool::new(false);
        assert!(matches!(
            export_at(&data, &root, false, &cancel, no_progress),
            Err(TransferError::Database(message))
                if message.contains("source key")
                    && message.contains("cache key")
                    && message.contains("book.7z")
                    && message.contains("直接閲覧と変換キャッシュの両方で編集")
        ));
    }

    #[test]
    fn converted_archive_export_reports_archive_cache_open_and_query_failures() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("book.7z"), b"archive").unwrap();
        init_data_dir(&data);
        let cache_db = data.join("archive_cache.db");
        let cancel = AtomicBool::new(false);

        fs::create_dir(&cache_db).unwrap();
        assert!(matches!(
            export_at(&data, &root, false, &cancel, no_progress),
            Err(TransferError::Database(message))
                if message.contains("archive_cache.db")
        ));
        fs::remove_dir(&cache_db).unwrap();

        Connection::open(&cache_db)
            .unwrap()
            .execute_batch("CREATE TABLE unrelated (id INTEGER PRIMARY KEY);")
            .unwrap();
        assert!(matches!(
            export_at(&data, &root, false, &cancel, no_progress),
            Err(TransferError::Database(message))
                if message.contains("archive_cache.db") && message.contains("book.7z")
        ));
    }

    #[test]
    fn mixed_case_virtual_member_uses_one_canonical_key_for_all_databases() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("library");
        let data_dir = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("book.zip"), b"zip").unwrap();
        init_data_dir(&data_dir);

        let mixed_member = r"Pages\Cover.JPG";
        let mixed_container = r"Chapters\Volume.ONE";
        let manifest = Manifest {
            format: FORMAT_NAME.to_string(),
            version: FORMAT_VERSION,
            exported_at_ms: 123_000,
            recursive: false,
            sections: ManifestSections::default(),
            entries: vec![PortableEntry {
                path: "book.zip".to_string(),
                kind: PortableEntryKind::File,
                media_kind: PortableMediaKind::Zip,
                virtual_key_base: PortableVirtualKeyBase::Source,
                container_key_base: PortableVirtualKeyBase::Source,
                fingerprint: Some(FileFingerprint {
                    size: 3,
                    modified_ms: None,
                }),
                rating: None,
                tags: Vec::new(),
                tags_decided: false,
                timed_bookmarks: Vec::new(),
                book_bookmarks: Vec::new(),
                page_state: PortablePageState::default(),
                container_state: PortableContainerState::default(),
                nested_containers: vec![PortableNestedContainer {
                    member_key: mixed_container.to_string(),
                    state: PortableContainerState {
                        spread: Some(PortableSpreadState {
                            mode: 2,
                            flow: 0,
                            direction: 1,
                        }),
                        ..PortableContainerState::default()
                    },
                }],
                video_pin: None,
                virtual_items: vec![PortableVirtualItem {
                    member_key: mixed_member.to_string(),
                    rating: Some(PortableRating {
                        stars: 4,
                        rated_at_ms: Some(456),
                        kind: Some("zip_image".to_string()),
                        entry_name: Some(mixed_member.to_string()),
                        page_num: None,
                        dir_prefix: None,
                        archive_format: None,
                        zipdir_is_archive: None,
                        zipdir_representative: None,
                    }),
                    tags: vec![PortableTag {
                        name: "MixedCase".to_string(),
                        applied_at: 456,
                    }],
                    tags_decided: true,
                    page_state: PortablePageState {
                        rotation_degrees: Some(90),
                        ..PortablePageState::default()
                    },
                }],
            }],
        };
        let cancel = AtomicBool::new(false);
        write_manifest_atomic(&root, &manifest, &cancel).unwrap();

        let base_key = crate::path_key::normalize_keep_drive(&root.join("book.zip"));
        let canonical_key = canonical_virtual_item_key(&base_key, mixed_member);
        let raw_key = format!("{base_key}::{mixed_member}");
        let summary = import_at(&data_dir, &root, &cancel, no_progress).unwrap();
        assert_eq!(summary.failed_entries, 0);

        let rating_db = Connection::open(data_dir.join("rating.db")).unwrap();
        assert_eq!(
            rating_db
                .query_row(
                    "SELECT stars FROM ratings WHERE path = ?1",
                    [&canonical_key],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            4
        );
        assert_eq!(
            rating_db
                .query_row(
                    "SELECT COUNT(*) FROM ratings WHERE path = ?1",
                    [&raw_key],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );

        let tags_db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")).unwrap();
        assert_eq!(
            tags_db
                .get_item_tags(&canonical_key)
                .into_iter()
                .map(|tag| tag.tag)
                .collect::<Vec<_>>(),
            vec!["MixedCase"]
        );
        assert!(tags_db.get_item_tags(&raw_key).is_empty());

        let rotation_db = Connection::open(data_dir.join("rotation.db")).unwrap();
        assert_eq!(
            rotation_db
                .query_row(
                    "SELECT angle FROM rotations WHERE path = ?1",
                    [&canonical_key],
                    |row| row.get::<_, i32>(0)
                )
                .unwrap(),
            90
        );
        let snapshot = load_import_page_state_snapshot(&data_dir).unwrap();
        assert!(snapshot.rotated.contains(&canonical_key));
        let stripped_base = crate::path_key::normalize(&root.join("book.zip"));
        let canonical_container_key = join_container_key(&stripped_base, mixed_container);
        let raw_container_key = format!("{stripped_base}/{mixed_container}");
        let spread_db = Connection::open(data_dir.join("spread.db")).unwrap();
        assert_eq!(
            spread_db
                .query_row(
                    "SELECT mode FROM spreads WHERE path = ?1",
                    [&canonical_container_key],
                    |row| row.get::<_, i32>(0)
                )
                .unwrap(),
            2
        );
        assert_eq!(
            spread_db
                .query_row(
                    "SELECT COUNT(*) FROM spreads WHERE path = ?1",
                    [&raw_container_key],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn slash_variants_are_duplicate_virtual_and_nested_member_keys() {
        let virtual_item = |member_key: &str| PortableVirtualItem {
            member_key: member_key.to_string(),
            rating: None,
            tags: Vec::new(),
            tags_decided: false,
            page_state: PortablePageState::default(),
        };
        let nested_container = |member_key: &str| PortableNestedContainer {
            member_key: member_key.to_string(),
            state: PortableContainerState::default(),
        };
        let mut manifest = Manifest {
            format: FORMAT_NAME.to_string(),
            version: FORMAT_VERSION,
            exported_at_ms: 0,
            recursive: false,
            sections: ManifestSections::default(),
            entries: vec![PortableEntry {
                path: "book.zip".to_string(),
                kind: PortableEntryKind::File,
                media_kind: PortableMediaKind::Zip,
                virtual_key_base: PortableVirtualKeyBase::Source,
                container_key_base: PortableVirtualKeyBase::Source,
                fingerprint: Some(FileFingerprint {
                    size: 1,
                    modified_ms: None,
                }),
                rating: None,
                tags: Vec::new(),
                tags_decided: false,
                timed_bookmarks: Vec::new(),
                book_bookmarks: Vec::new(),
                page_state: PortablePageState::default(),
                container_state: PortableContainerState::default(),
                nested_containers: Vec::new(),
                video_pin: None,
                virtual_items: vec![virtual_item("A/B"), virtual_item(r"A\B")],
            }],
        };
        assert!(matches!(
            validate_manifest(&manifest),
            Err(TransferError::Invalid(message))
                if message.contains("仮想項目キーが不正または重複")
        ));

        manifest.entries[0].virtual_items.clear();
        manifest.entries[0].nested_containers =
            vec![nested_container("A/B"), nested_container(r"A\B")];
        assert!(matches!(
            validate_manifest(&manifest),
            Err(TransferError::Invalid(message))
                if message.contains("仮想コンテナキーが不正または重複")
        ));
    }

    #[test]
    fn item_import_rolls_back_all_stores_on_late_write_failure() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("clip.mp4"), b"same-video").unwrap();
        fs::write(destination.join("clip.mp4"), b"same-video").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);

        let source_path = source.join("clip.mp4");
        set_rating(&source_data, &source_path, 5);
        Connection::open(source_data.join("video_pins.db"))
            .unwrap()
            .execute(
                "INSERT INTO video_pins (path, pin_pts_secs, thumb_webp, thumb_pts_secs)
                 VALUES (?1, 9.0, ?2, 9.0)",
                params![
                    crate::path_key::normalize_keep_drive(&source_path),
                    b"new-thumb".as_slice()
                ],
            )
            .unwrap();

        let destination_path = destination.join("clip.mp4");
        set_rating(&destination_data, &destination_path, 2);
        let video_pins = Connection::open(destination_data.join("video_pins.db")).unwrap();
        video_pins
            .execute(
                "INSERT INTO video_pins (path, pin_pts_secs, thumb_webp, thumb_pts_secs)
                 VALUES (?1, 1.0, ?2, 1.0)",
                params![
                    crate::path_key::normalize_keep_drive(&destination_path),
                    b"old-thumb".as_slice()
                ],
            )
            .unwrap();
        // video_pin is written after rating and all page/container stores.  A late
        // failure here must roll the main rating DB back through the same attached
        // transaction, not leave a partially imported physical item.
        video_pins
            .execute_batch(
                "CREATE TRIGGER fail_metadata_import_video_pin
                 BEFORE INSERT ON video_pins
                 BEGIN
                     SELECT RAISE(FAIL, 'injected late import failure');
                 END;",
            )
            .unwrap();
        drop(video_pins);

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        copy_sidecar_bundle(&source, &destination);
        let imported = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        // The manifest also contains the root directory entry; only clip.mp4 is
        // fault-injected and must remain wholly at its previous state.
        assert_eq!(imported.applied_entries, 1);
        assert_eq!(imported.failed_entries, 1);
        assert_eq!(imported.failed_items.len(), 1);
        assert_eq!(imported.failed_items[0].path, "clip.mp4");
        assert!(
            imported.failed_items[0]
                .reason
                .contains("injected late import failure")
        );
        assert_eq!(rating(&destination_data, &destination_path), Some(2));
        let video_pins = Connection::open(destination_data.join("video_pins.db")).unwrap();
        let old_pin = video_pins
            .query_row(
                "SELECT pin_pts_secs, thumb_webp FROM video_pins WHERE path = ?1",
                [crate::path_key::normalize_keep_drive(&destination_path)],
                |row| Ok((row.get::<_, f64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .unwrap();
        assert_eq!(old_pin, (1.0, b"old-thumb".to_vec()));
    }

    #[test]
    fn v7_round_trip_is_self_contained_without_automatic_sidecar() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        let edited_image = "ChatGPT Image 2026年6月7日 15_49_45 (1).png";
        let image_book = Path::new("原神").join("ヨォーヨ");
        let image_book_page = "001.png";
        fs::create_dir_all(source.join(&image_book)).unwrap();
        fs::create_dir_all(destination.join(&image_book)).unwrap();
        for name in [edited_image, "book.zip", "clip.mp4"] {
            fs::write(source.join(name), name.as_bytes()).unwrap();
            fs::write(destination.join(name), name.as_bytes()).unwrap();
        }
        fs::write(
            source.join(&image_book).join(image_book_page),
            image_book_page.as_bytes(),
        )
        .unwrap();
        fs::write(
            destination.join(&image_book).join(image_book_page),
            image_book_page.as_bytes(),
        )
        .unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);
        assert!(!source.join(crate::sidecar::SIDECAR_FILENAME).exists());
        set_rating(&source_data, &source.join(edited_image), 3);
        set_tags(
            &source_data,
            &source.join(edited_image),
            &["AI", "テキスト注釈"],
        );

        let source_root = crate::path_key::normalize(&source);
        let source_root_keep = crate::path_key::normalize_keep_drive(&source);
        let source_image_book = crate::path_key::normalize(&source.join(&image_book));
        let source_image_book_keep =
            crate::path_key::normalize_keep_drive(&source.join(&image_book));
        let source_nested_book = join_container_key(
            &crate::path_key::normalize(&source.join("book.zip")),
            "特典用",
        );
        let source_nested_book_keep = join_container_key(
            &crate::path_key::normalize_keep_drive(&source.join("book.zip")),
            "特典用",
        );
        let image_key = crate::path_key::normalize_keep_drive(&source.join(edited_image));
        let virtual_key = format!(
            "{}::特典用/001.jpg",
            crate::path_key::normalize_keep_drive(&source.join("book.zip"))
        );
        Connection::open(source_data.join("video_bookmarks.db"))
            .unwrap()
            .execute(
                "INSERT INTO video_bookmarks (path, pts_secs, title, created_at)
                 VALUES (?1, 8.25, '虹色の場面', 99)",
                [crate::path_key::normalize_keep_drive(
                    &source.join("clip.mp4"),
                )],
            )
            .unwrap();
        Connection::open(source_data.join("book_bookmarks.db"))
            .unwrap()
            .execute(
                "INSERT INTO book_bookmarks
                    (container_key, container_path, container_kind, page_kind, page_value,
                     page_key, page_index_hint, created_at_ms, title)
                 VALUES (?1, ?2, 'zip', 'archive_entry', '特典用/001.jpg',
                         '特典用/001.jpg', 0, 777, '表紙')",
                params![
                    crate::path_key::normalize_keep_drive(&source.join("book.zip")),
                    source.join("book.zip").to_string_lossy().as_ref()
                ],
            )
            .unwrap();
        let spread = Connection::open(source_data.join("spread.db")).unwrap();
        for (path, mode, flow, direction) in [
            (&source_root, 2, 1, 1),
            (&source_image_book, 4, 2, 0),
            (&source_nested_book, 3, 2, 0),
        ] {
            spread
                .execute(
                    "INSERT INTO spreads (path, mode, flow, direction)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![path, mode, flow, direction],
                )
                .unwrap();
        }
        let book_trim = crate::view_trim::ViewTrimBookState {
            apply_mode: crate::view_trim::ViewTrimApplyMode::Book,
            book_settings: crate::view_trim::ViewTrimBookSettings {
                enabled: true,
                single: crate::view_trim::ViewTrimMargins {
                    left: 0.01,
                    top: 0.02,
                    right: 0.03,
                    bottom: 0.04,
                },
                ..Default::default()
            },
        };
        let page_trim = crate::view_trim::ViewTrimPageOverride::from_margins(
            crate::view_trim::ViewTrimMargins {
                left: 0.05,
                top: 0.06,
                right: 0.07,
                bottom: 0.08,
            },
        );
        let view_trim = Connection::open(source_data.join("view_trim.db")).unwrap();
        view_trim
            .execute(
                "INSERT INTO view_trim_books (book_key, state_json) VALUES (?1, ?2)",
                params![source_root, serde_json::to_string(&book_trim).unwrap()],
            )
            .unwrap();
        view_trim
            .execute(
                "INSERT INTO view_trim_pages (page_path, override_json) VALUES (?1, ?2)",
                params![image_key, serde_json::to_string(&page_trim).unwrap()],
            )
            .unwrap();
        Connection::open(source_data.join("rotation.db"))
            .unwrap()
            .execute(
                "INSERT INTO rotations (path, angle) VALUES (?1, 90), (?2, 270)",
                params![image_key, virtual_key],
            )
            .unwrap();
        let mut adjustment = crate::adjustment::AdjustParams::default();
        adjustment.brightness = 12.0;
        adjustment.contrast = -8.0;
        adjustment.gamma = 1.2;
        adjustment.saturation = 15.0;
        adjustment.smart_sharpen = 24;
        let adjustment_json = serde_json::to_string(&adjustment).unwrap();
        Connection::open(source_data.join("adjustment.db"))
            .unwrap()
            .execute(
                "INSERT INTO page_params (page_path, params_json) VALUES (?1, ?2)",
                params![image_key, adjustment_json],
            )
            .unwrap();
        let erase_mask = crate::mask_db::compress_mask(&[true, false, false, true]);
        Connection::open(source_data.join("mask.db"))
            .unwrap()
            .execute(
                "INSERT INTO masks (path, mask_data, width, height, vectors)
                 VALUES (?1, ?2, 2, 2, '[]')",
                params![image_key, erase_mask],
            )
            .unwrap();
        let conceal_mask = crate::mask_db::compress_mask(&[false, true, true, false]);
        Connection::open(source_data.join("conceal.db"))
            .unwrap()
            .execute(
                "INSERT INTO conceal_entries
                    (page_path, bitmap_w, bitmap_h, bitmap_data, shapes)
                 VALUES (?1, 2, 2, ?2, '[]')",
                params![image_key, conceal_mask],
            )
            .unwrap();
        let local_layers = vec![local_adjust_core::LocalAdjustmentLayer::new(
            "修復／塗り",
            local_adjust_core::LocalMask::Full,
            local_adjust_core::LocalEffect::None,
        )];
        let local_layers_json = serde_json::to_string(&local_layers).unwrap();
        Connection::open(source_data.join("local_adjust.db"))
            .unwrap()
            .execute(
                "INSERT INTO local_adjust_pages (page_path, layers_json) VALUES (?1, ?2)",
                params![image_key, local_layers_json],
            )
            .unwrap();
        Connection::open(source_data.join("export_crop.db"))
            .unwrap()
            .execute(
                "INSERT INTO export_crop_pages
                    (page_path, min_x, min_y, max_x, max_y, aspect_mode)
                 VALUES (?1, 63.3854256, 33.5276108, 1024.0, 1024.0, 'free')",
                [&image_key],
            )
            .unwrap();
        let mut text = comic_core::TextBlock {
            text: "ガーン".to_string(),
            size_px: 157.0,
            ..Default::default()
        };
        text.bold = true;
        let annotations = vec![comic_core::AnnotationObject::new_text(
            1,
            (701.0, 344.0),
            text,
        )];
        let annotations_json = serde_json::to_string(&annotations).unwrap();
        Connection::open(source_data.join("comic.db"))
            .unwrap()
            .execute(
                "INSERT INTO comic_entries (page_path, doc_version, doc_json)
                 VALUES (?1, 1, ?2)",
                params![image_key, annotations_json],
            )
            .unwrap();
        Connection::open(source_data.join("folder_thumb_pins.db"))
            .unwrap()
            .execute(
                "INSERT INTO folder_thumb_pins
                    (container_key, source_kind, source_rel, source_entry, source_page)
                 VALUES (?1, 'image', ?2, NULL, NULL)",
                params![source_root_keep, edited_image],
            )
            .unwrap();
        Connection::open(source_data.join("folder_thumb_pins.db"))
            .unwrap()
            .execute(
                "INSERT INTO folder_thumb_pins
                    (container_key, source_kind, source_rel, source_entry, source_page)
                 VALUES (?1, 'image', ?2, NULL, NULL)",
                params![source_image_book_keep, image_book_page],
            )
            .unwrap();
        Connection::open(source_data.join("folder_thumb_pins.db"))
            .unwrap()
            .execute(
                "INSERT INTO folder_thumb_pins
                    (container_key, source_kind, source_rel, source_entry, source_page)
                 VALUES (?1, 'zipentry', '', '特典用/001.jpg', NULL)",
                [&source_nested_book_keep],
            )
            .unwrap();
        Connection::open(source_data.join("video_pins.db"))
            .unwrap()
            .execute(
                "INSERT INTO video_pins (path, pin_pts_secs, thumb_webp, thumb_pts_secs)
                 VALUES (?1, 12.5, ?2, 12.5)",
                params![
                    crate::path_key::normalize_keep_drive(&source.join("clip.mp4")),
                    b"webp".as_slice()
                ],
            )
            .unwrap();

        let cancel = AtomicBool::new(false);
        let exported = export_at(&source_data, &source, true, &cancel, no_progress).unwrap();
        assert!(exported.page_states >= 2);
        assert!(exported.container_states >= 3);
        assert_eq!(exported.thumbnail_pins, 4);
        copy_sidecar_bundle(&source, &destination);
        let imported = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(imported.failed_entries, 0);

        let destination_root = crate::path_key::normalize(&destination);
        let destination_root_keep = crate::path_key::normalize_keep_drive(&destination);
        let destination_image =
            crate::path_key::normalize_keep_drive(&destination.join(edited_image));
        let destination_virtual = format!(
            "{}::特典用/001.jpg",
            crate::path_key::normalize_keep_drive(&destination.join("book.zip"))
        );
        assert_eq!(
            rating(&destination_data, &destination.join(edited_image)),
            Some(3)
        );
        assert_eq!(
            tags(&destination_data, &destination.join(edited_image)),
            vec!["AI", "テキスト注釈"]
        );
        assert_eq!(
            Connection::open(destination_data.join("video_bookmarks.db"))
                .unwrap()
                .query_row(
                    "SELECT path, pts_secs, title FROM video_bookmarks",
                    [],
                    |row| Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, f64>(1)?,
                        row.get::<_, String>(2)?
                    ))
                )
                .unwrap(),
            (
                crate::path_key::normalize_keep_drive(&destination.join("clip.mp4")),
                8.25,
                "虹色の場面".to_string()
            )
        );
        assert_eq!(
            Connection::open(destination_data.join("book_bookmarks.db"))
                .unwrap()
                .query_row(
                    "SELECT container_key, page_value, title FROM book_bookmarks",
                    [],
                    |row| Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?
                    ))
                )
                .unwrap(),
            (
                crate::path_key::normalize_keep_drive(&destination.join("book.zip")),
                "特典用/001.jpg".to_string(),
                "表紙".to_string()
            )
        );
        assert_eq!(
            Connection::open(destination_data.join("spread.db"))
                .unwrap()
                .query_row(
                    "SELECT mode, flow, direction FROM spreads WHERE path = ?1",
                    [&destination_root],
                    |row| Ok((
                        row.get::<_, i32>(0)?,
                        row.get::<_, i32>(1)?,
                        row.get::<_, i32>(2)?
                    ))
                )
                .unwrap(),
            (2, 1, 1)
        );
        let destination_image_book = crate::path_key::normalize(&destination.join(&image_book));
        assert_eq!(
            Connection::open(destination_data.join("spread.db"))
                .unwrap()
                .query_row(
                    "SELECT mode, flow, direction FROM spreads WHERE path = ?1",
                    [&destination_image_book],
                    |row| Ok((
                        row.get::<_, i32>(0)?,
                        row.get::<_, i32>(1)?,
                        row.get::<_, i32>(2)?
                    ))
                )
                .unwrap(),
            (4, 2, 0)
        );
        let destination_nested_book = join_container_key(
            &crate::path_key::normalize(&destination.join("book.zip")),
            "特典用",
        );
        assert_eq!(
            Connection::open(destination_data.join("spread.db"))
                .unwrap()
                .query_row(
                    "SELECT mode, flow, direction FROM spreads WHERE path = ?1",
                    [&destination_nested_book],
                    |row| Ok((
                        row.get::<_, i32>(0)?,
                        row.get::<_, i32>(1)?,
                        row.get::<_, i32>(2)?
                    ))
                )
                .unwrap(),
            (3, 2, 0)
        );
        let destination_view_trim =
            Connection::open(destination_data.join("view_trim.db")).unwrap();
        assert_eq!(
            destination_view_trim
                .query_row(
                    "SELECT state_json FROM view_trim_books WHERE book_key = ?1",
                    [&destination_root],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            serde_json::to_string(&book_trim).unwrap()
        );
        assert_eq!(
            destination_view_trim
                .query_row(
                    "SELECT override_json FROM view_trim_pages WHERE page_path = ?1",
                    [&destination_image],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            serde_json::to_string(&page_trim).unwrap()
        );
        let rotations = Connection::open(destination_data.join("rotation.db")).unwrap();
        assert_eq!(
            rotations
                .query_row(
                    "SELECT angle FROM rotations WHERE path = ?1",
                    [&destination_image],
                    |row| row.get::<_, i32>(0)
                )
                .unwrap(),
            90
        );
        assert_eq!(
            rotations
                .query_row(
                    "SELECT angle FROM rotations WHERE path = ?1",
                    [&destination_virtual],
                    |row| row.get::<_, i32>(0)
                )
                .unwrap(),
            270
        );
        for (db, table, column) in [
            ("adjustment.db", "page_params", "page_path"),
            ("mask.db", "masks", "path"),
            ("conceal.db", "conceal_entries", "page_path"),
            ("local_adjust.db", "local_adjust_pages", "page_path"),
            ("export_crop.db", "export_crop_pages", "page_path"),
            ("comic.db", "comic_entries", "page_path"),
        ] {
            let count = Connection::open(destination_data.join(db))
                .unwrap()
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
                    [&destination_image],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap();
            assert_eq!(count, 1, "missing imported state in {db}");
        }
        assert_eq!(
            Connection::open(destination_data.join("adjustment.db"))
                .unwrap()
                .query_row(
                    "SELECT params_json FROM page_params WHERE page_path = ?1",
                    [&destination_image],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            adjustment_json
        );
        assert_eq!(
            Connection::open(destination_data.join("mask.db"))
                .unwrap()
                .query_row(
                    "SELECT mask_data, width, height, vectors FROM masks WHERE path = ?1",
                    [&destination_image],
                    |row| Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                )
                .unwrap(),
            (erase_mask, 2, 2, None)
        );
        assert_eq!(
            Connection::open(destination_data.join("conceal.db"))
                .unwrap()
                .query_row(
                    "SELECT bitmap_data, bitmap_w, bitmap_h, shapes
                       FROM conceal_entries WHERE page_path = ?1",
                    [&destination_image],
                    |row| Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                )
                .unwrap(),
            (conceal_mask, 2, 2, None)
        );
        assert_eq!(
            Connection::open(destination_data.join("local_adjust.db"))
                .unwrap()
                .query_row(
                    "SELECT layers_json FROM local_adjust_pages WHERE page_path = ?1",
                    [&destination_image],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            local_layers_json
        );
        assert_eq!(
            Connection::open(destination_data.join("comic.db"))
                .unwrap()
                .query_row(
                    "SELECT doc_version, doc_json FROM comic_entries WHERE page_path = ?1",
                    [&destination_image],
                    |row| Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?))
                )
                .unwrap(),
            (1, annotations_json)
        );
        assert_eq!(
            Connection::open(destination_data.join("export_crop.db"))
                .unwrap()
                .query_row(
                    "SELECT min_x, min_y, max_x, max_y, aspect_mode
                       FROM export_crop_pages WHERE page_path = ?1",
                    [&destination_image],
                    |row| Ok((
                        row.get::<_, f32>(0)?,
                        row.get::<_, f32>(1)?,
                        row.get::<_, f32>(2)?,
                        row.get::<_, f32>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                )
                .unwrap(),
            (63.3854256, 33.5276108, 1024.0, 1024.0, "free".to_string()),
            "source-image pixel crop coordinates must round-trip without normalization"
        );
        assert_eq!(
            Connection::open(destination_data.join("folder_thumb_pins.db"))
                .unwrap()
                .query_row(
                    "SELECT source_kind, source_rel FROM folder_thumb_pins WHERE container_key = ?1",
                    [&destination_root_keep],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                )
                .unwrap(),
            ("image".to_string(), edited_image.to_string())
        );
        assert_eq!(
            Connection::open(destination_data.join("folder_thumb_pins.db"))
                .unwrap()
                .query_row(
                    "SELECT source_kind, source_rel FROM folder_thumb_pins WHERE container_key = ?1",
                    [crate::path_key::normalize_keep_drive(
                        &destination.join(&image_book)
                    )],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                )
                .unwrap(),
            ("image".to_string(), image_book_page.to_string())
        );
        assert_eq!(
            Connection::open(destination_data.join("folder_thumb_pins.db"))
                .unwrap()
                .query_row(
                    "SELECT source_kind, source_entry FROM folder_thumb_pins WHERE container_key = ?1",
                    [join_container_key(
                        &crate::path_key::normalize_keep_drive(&destination.join("book.zip")),
                        "特典用"
                    )],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                )
                .unwrap(),
            ("zipentry".to_string(), "特典用/001.jpg".to_string())
        );
        assert_eq!(
            Connection::open(destination_data.join("video_pins.db"))
                .unwrap()
                .query_row(
                    "SELECT pin_pts_secs, thumb_webp FROM video_pins WHERE path = ?1",
                    [crate::path_key::normalize_keep_drive(
                        &destination.join("clip.mp4")
                    )],
                    |row| Ok((row.get::<_, f64>(0)?, row.get::<_, Vec<u8>>(1)?))
                )
                .unwrap(),
            (12.5, b"webp".to_vec())
        );
        assert!(!destination.join(crate::sidecar::SIDECAR_FILENAME).exists());
    }

    #[test]
    fn import_clears_listed_empty_metadata_and_preserves_unlisted_items() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("listed.jpg"), b"x").unwrap();
        fs::write(destination.join("listed.jpg"), b"x").unwrap();
        fs::write(destination.join("unlisted.jpg"), b"y").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);
        set_rating(&destination_data, &destination.join("listed.jpg"), 5);
        set_tags(
            &destination_data,
            &destination.join("listed.jpg"),
            &["古い"],
        );
        let stale_virtual_key = format!(
            "{}::stale-page",
            crate::path_key::normalize_keep_drive(&destination.join("listed.jpg"))
        );
        Connection::open(destination_data.join("rating.db"))
            .unwrap()
            .execute(
                "INSERT INTO ratings (path, stars) VALUES (?1, 4)",
                [&stale_virtual_key],
            )
            .unwrap();
        crate::tags_db::TagsDb::open_at(&destination_data.join("tags.db"))
            .unwrap()
            .set_item_tags(
                &stale_virtual_key,
                ["古いページ"],
                crate::tags_db::source::EDIT,
            )
            .unwrap();
        set_rating(&destination_data, &destination.join("unlisted.jpg"), 3);

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        copy_sidecar_bundle(&source, &destination);
        let _summary = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();

        assert_eq!(
            rating(&destination_data, &destination.join("listed.jpg")),
            None
        );
        assert!(tags(&destination_data, &destination.join("listed.jpg")).is_empty());
        assert_eq!(
            Connection::open(destination_data.join("rating.db"))
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM ratings WHERE path = ?1",
                    [&stale_virtual_key],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert!(
            crate::tags_db::TagsDb::open_at(&destination_data.join("tags.db"))
                .unwrap()
                .get_item_tags(&stale_virtual_key)
                .is_empty()
        );
        assert_eq!(
            rating(&destination_data, &destination.join("unlisted.jpg")),
            Some(3)
        );
    }

    #[test]
    fn v6_and_older_bundles_are_rejected_with_v7_only_message() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.jpg"), b"x").unwrap();
        init_data_dir(&data);

        let cancel = AtomicBool::new(false);
        export_at(&data, &root, false, &cancel, no_progress).unwrap();
        let path = root.join(SIDECAR_FILENAME).join(BUNDLE_MANIFEST_FILENAME);
        let mut json: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for version in [6, 5] {
            json["version"] = serde_json::Value::from(version);
            fs::write(&path, serde_json::to_vec_pretty(&json).unwrap()).unwrap();
            assert!(matches!(
                inspect_import_at(&root, &cancel, no_progress),
                Err(TransferError::Invalid(message))
                    if message.contains("未対応のバージョン")
                        && message.contains(&version.to_string())
                        && message.contains("v7だけを受け入れます")
            ));
        }
    }

    #[test]
    fn v7_import_marks_existing_automatic_sidecar_as_consumed() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("a.jpg"), b"x").unwrap();
        fs::write(destination.join("a.jpg"), b"x").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        copy_sidecar_bundle(&source, &destination);
        let automatic_sidecar = destination.join(crate::sidecar::SIDECAR_FILENAME);
        fs::write(&automatic_sidecar, br#"{"version":1,"items":{}}"#).unwrap();
        let modified_secs = fs::metadata(&automatic_sidecar)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        let folder_key = crate::adjustment_db::normalize_path(&destination);
        assert_eq!(
            Connection::open(destination_data.join("adjustment.db"))
                .unwrap()
                .query_row(
                    "SELECT sidecar_mtime FROM sidecar_sync WHERE folder_key = ?1",
                    [&folder_key],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            modified_secs
        );
        assert_eq!(
            Connection::open(destination_data.join("tags.db"))
                .unwrap()
                .query_row(
                    "SELECT sidecar_mtime FROM tag_sidecar_sync WHERE folder_key = ?1",
                    [&folder_key],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            modified_secs
        );
    }

    #[test]
    fn nonrecursive_export_does_not_include_nested_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("top.jpg"), b"x").unwrap();
        fs::write(root.join("nested/deep.jpg"), b"y").unwrap();
        init_data_dir(&data);
        let cancel = AtomicBool::new(false);
        let summary = export_at(&data, &root, false, &cancel, no_progress).unwrap();
        assert_eq!(summary.entries, 3); // root + immediate file + immediate directory
        let manifest = read_manifest(&root, &cancel).unwrap();
        assert!(
            !manifest
                .entries
                .iter()
                .any(|entry| entry.path == "nested/deep.jpg")
        );
    }

    #[test]
    fn recursive_export_writes_one_streaming_shard_per_folder() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::create_dir_all(root.join("empty")).unwrap();
        fs::write(root.join("root.jpg"), b"r").unwrap();
        fs::write(root.join("a/a.jpg"), b"a").unwrap();
        fs::write(root.join("a/b/b.jpg"), b"b").unwrap();
        init_data_dir(&data);

        let cancel = AtomicBool::new(false);
        let summary = export_at(&data, &root, true, &cancel, no_progress).unwrap();
        let manifest = read_bundle_manifest(&root, &cancel).unwrap();
        assert_eq!(summary.entries, 7);
        assert_eq!(manifest.entry_count, 7);
        assert_eq!(manifest.shard_count, 4);

        let shards = bundle_generation_dir(&root.join(SIDECAR_FILENAME), &manifest.generation)
            .join(SHARDS_DIRNAME);
        for (folder, records) in [(".", 4), ("a", 2), ("a/b", 1), ("empty", 0)] {
            let shard = shards.join(shard_filename(folder));
            assert!(shard.is_file(), "missing shard for {folder}");
            assert_eq!(
                fs::read_to_string(shard).unwrap().lines().count(),
                records + 1,
                "header + direct entries for {folder}"
            );
        }
    }

    #[test]
    fn export_metadata_batch_spans_many_small_folders() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        for index in 0..100 {
            let folder = root.join(format!("d{index:03}"));
            fs::create_dir(&folder).unwrap();
            fs::write(folder.join("page.jpg"), b"x").unwrap();
        }
        init_data_dir(&data);

        let summary = export_at(&data, &root, true, &AtomicBool::new(false), no_progress).unwrap();
        assert_eq!(summary.entries, 201);
        assert_eq!(
            summary.metadata_batches, 1,
            "small folders must share one bounded metadata DB scope"
        );
    }

    #[test]
    fn second_pass_parse_failure_returns_committed_summary() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        for name in ["a.jpg", "b.jpg"] {
            fs::write(source.join(name), b"x").unwrap();
            fs::write(destination.join(name), b"x").unwrap();
        }
        init_data_dir(&source_data);
        init_data_dir(&destination_data);
        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        copy_sidecar_bundle(&source, &destination);

        let manifest = read_bundle_manifest(&destination, &cancel).unwrap();
        let shard =
            bundle_generation_dir(&destination.join(SIDECAR_FILENAME), &manifest.generation)
                .join(SHARDS_DIRNAME)
                .join(shard_filename("."));
        let lines = fs::read_to_string(&shard).unwrap();
        let mut padded = String::new();
        for (index, line) in lines.lines().enumerate() {
            if index == 0 {
                padded.push_str(line);
            } else {
                let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
                value
                    .as_object_mut()
                    .unwrap()
                    .insert("test_padding".to_string(), "x".repeat(64 * 1024).into());
                padded.push_str(&serde_json::to_string(&value).unwrap());
            }
            padded.push('\n');
        }
        fs::write(&shard, padded).unwrap();

        let mut truncated = false;
        let summary = import_at(&destination_data, &destination, &cancel, |progress| {
            if !truncated && progress.phase == TransferPhase::Importing && progress.processed == 1 {
                File::options()
                    .write(true)
                    .truncate(true)
                    .open(&shard)
                    .unwrap();
                truncated = true;
            }
        })
        .unwrap();

        assert!(truncated);
        assert!(summary.incomplete_error.is_some());
        assert!(summary.applied_entries >= 1);
    }

    #[test]
    fn export_crosses_streaming_batch_boundary_without_a_total_entry_cap() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        for index in 0..=EXPORT_BATCH_ENTRIES {
            fs::write(root.join(format!("{index:05}.jpg")), b"x").unwrap();
        }
        init_data_dir(&data);

        let cancel = AtomicBool::new(false);
        let summary = export_at(&data, &root, false, &cancel, no_progress).unwrap();
        let manifest = read_bundle_manifest(&root, &cancel).unwrap();
        let expected = EXPORT_BATCH_ENTRIES + 2; // root + batch boundaryを越えるfile群
        assert_eq!(summary.entries, expected);
        assert_eq!(manifest.entry_count, expected as u64);
        assert_eq!(manifest.shard_count, 1);
        assert_eq!(
            read_manifest(&root, &cancel).unwrap().entries.len(),
            expected
        );
    }

    #[test]
    fn import_uses_bounded_outer_transactions_instead_of_per_item_commits() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        for index in 0..600 {
            let name = format!("{index:04}.jpg");
            fs::write(source.join(&name), b"x").unwrap();
            fs::write(destination.join(&name), b"x").unwrap();
        }
        init_data_dir(&source_data);
        init_data_dir(&destination_data);
        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        copy_sidecar_bundle(&source, &destination);

        let summary = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(summary.applied_entries, 601);
        assert_eq!(summary.transaction_batches, 3);
    }

    #[test]
    fn manifest_count_mismatch_is_rejected_before_any_database_write() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("a.jpg"), b"x").unwrap();
        fs::write(destination.join("a.jpg"), b"x").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);
        set_rating(&source_data, &source.join("a.jpg"), 1);
        set_rating(&destination_data, &destination.join("a.jpg"), 5);

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        copy_sidecar_bundle(&source, &destination);
        let manifest_path = destination
            .join(SIDECAR_FILENAME)
            .join(BUNDLE_MANIFEST_FILENAME);
        let mut manifest: BundleManifest =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest.entry_count += 1;
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

        assert!(matches!(
            import_at(
                &destination_data,
                &destination,
                &cancel,
                no_progress
            ),
            Err(TransferError::Invalid(message)) if message.contains("manifest件数")
        ));
        assert_eq!(
            rating(&destination_data, &destination.join("a.jpg")),
            Some(5)
        );
    }

    #[test]
    fn export_reads_only_metadata_for_enumerated_items() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        let data = temp.path().join("data");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(root.join("inside.jpg"), b"x").unwrap();
        fs::write(root.join("nested/deep.jpg"), b"x").unwrap();
        fs::write(outside.join("other.jpg"), b"x").unwrap();
        init_data_dir(&data);
        set_rating(&data, &root.join("inside.jpg"), 1);
        set_rating(&data, &root.join("nested/deep.jpg"), 2);
        set_rating(&data, &outside.join("other.jpg"), 3);

        let cancel = AtomicBool::new(false);
        let mut read_rows = 0;
        let summary = export_at(&data, &root, false, &cancel, |progress| {
            if progress.phase == TransferPhase::ReadingMetadata {
                read_rows = read_rows.max(progress.processed);
            }
        })
        .unwrap();
        assert_eq!(summary.ratings, 1);
        assert_eq!(read_rows, 1);
    }

    #[test]
    fn metadata_scope_query_seeks_rating_path_index() {
        let temp = tempfile::TempDir::new().unwrap();
        let data = temp.path().join("data");
        init_data_dir(&data);
        let mut scope = HashMap::new();
        scope.insert("c:/photos/a.jpg".to_string(), 0);
        let cancel = AtomicBool::new(false);
        let mut conn = open_readonly(&data.join("rating.db")).unwrap();
        prepare_metadata_scope(&mut conn, &scope, &scope, &cancel).unwrap();
        let mut stmt = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT r.path
                   FROM metadata_transfer_scope AS s
                  CROSS JOIN ratings AS r
                  WHERE r.path = s.item_key
                 UNION ALL
                 SELECT r.path
                   FROM metadata_transfer_scope AS s
                  CROSS JOIN ratings AS r
                  WHERE r.path >= s.virtual_lower AND r.path < s.virtual_upper",
            )
            .unwrap();
        let details = stmt
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            details.iter().any(|detail| detail.contains("SEARCH r")),
            "query plan should seek the ratings path index: {details:?}"
        );
        assert!(
            details.iter().all(|detail| !detail.contains("SCAN r")),
            "query plan must not scan the global ratings table: {details:?}"
        );
    }

    #[test]
    fn family_delete_uses_indexed_range_and_keeps_neighbor_keys() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE values_by_key (item_key TEXT PRIMARY KEY);
             INSERT INTO values_by_key VALUES
                ('c:/book.zip'),
                ('c:/book.zip::page/1.jpg'),
                ('c:/book.zip::page/2.jpg'),
                ('c:/book.zip:;neighbor'),
                ('c:/book.zipx::page/1.jpg');",
        )
        .unwrap();
        let mut plan = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 DELETE FROM values_by_key
                  WHERE item_key = ?1
                     OR (item_key >= ?1 || '::' AND item_key < ?1 || ':;')",
            )
            .unwrap();
        let details = plan
            .query_map(["c:/book.zip"], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            details.iter().any(|detail| detail.contains("SEARCH")),
            "family delete should seek the primary-key index: {details:?}"
        );
        assert!(
            details.iter().all(|detail| !detail.contains("SCAN")),
            "family delete must not scan the global table: {details:?}"
        );

        delete_key_family(&conn, "values_by_key", "item_key", "c:/book.zip").unwrap();
        let remaining = conn
            .prepare("SELECT item_key FROM values_by_key ORDER BY item_key")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            remaining,
            vec![
                "c:/book.zip:;neighbor".to_string(),
                "c:/book.zipx::page/1.jpg".to_string()
            ]
        );
    }

    #[test]
    fn export_scan_observes_cancel_between_directory_entries() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        for index in 0..16 {
            fs::write(root.join(format!("{index:02}.jpg")), b"x").unwrap();
        }
        init_data_dir(&data);
        let cancel = AtomicBool::new(false);
        assert!(matches!(
            export_at(&data, &root, false, &cancel, |progress| {
                if progress.phase == TransferPhase::Scanning {
                    cancel.store(true, Ordering::Relaxed);
                }
            }),
            Err(TransferError::Cancelled)
        ));
        assert!(!root.join(SIDECAR_FILENAME).exists());
    }

    #[test]
    fn recursive_export_rejects_excessive_depth_instead_of_truncating() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        let mut nested = root.clone();
        for index in 0..=MAX_RECURSION_DEPTH {
            nested.push(format!("d{index}"));
            fs::create_dir(&nested).unwrap();
        }
        init_data_dir(&data);
        let cancel = AtomicBool::new(false);
        assert!(matches!(
            export_at(&data, &root, true, &cancel, no_progress),
            Err(TransferError::Invalid(message)) if message.contains("深すぎます")
        ));
        assert!(!root.join(SIDECAR_FILENAME).exists());
    }

    #[test]
    fn recursive_export_reports_skipped_reparse_directories() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("hidden.jpg"), b"x").unwrap();
        let link = root.join("linked");
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(&outside, &link).is_err() {
            return;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        init_data_dir(&data);

        let summary = export_at(&data, &root, true, &AtomicBool::new(false), no_progress).unwrap();
        assert_eq!(summary.skipped_reparse_directories, 1);
        assert_eq!(summary.skipped_reparse_paths, vec!["linked".to_string()]);
        let manifest = read_manifest(&root, &AtomicBool::new(false)).unwrap();
        assert!(manifest.entries.iter().any(|entry| entry.path == "linked"));
        assert!(
            manifest
                .entries
                .iter()
                .all(|entry| entry.path != "linked/hidden.jpg")
        );
    }

    #[test]
    fn sidecar_writer_rejects_data_beyond_its_limit() {
        let cancel = AtomicBool::new(false);
        let mut output = Vec::new();
        let mut writer = CancelWriter {
            inner: &mut output,
            cancel: &cancel,
            written: 0,
            max_bytes: 3,
        };
        assert!(writer.write_all(b"1234").is_err());
        assert!(output.is_empty());
    }

    #[test]
    fn export_cancel_preserves_existing_sidecar() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.jpg"), b"x").unwrap();
        fs::write(root.join(SIDECAR_FILENAME), b"old").unwrap();
        init_data_dir(&data);
        let cancel = AtomicBool::new(true);
        assert!(matches!(
            export_at(&data, &root, false, &cancel, no_progress),
            Err(TransferError::Cancelled)
        ));
        assert_eq!(fs::read(root.join(SIDECAR_FILENAME)).unwrap(), b"old");
    }

    #[test]
    fn export_cancel_preserves_published_bundle_generation() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.jpg"), b"x").unwrap();
        init_data_dir(&data);
        let cancel = AtomicBool::new(false);
        export_at(&data, &root, false, &cancel, no_progress).unwrap();
        let manifest_path = root.join(SIDECAR_FILENAME).join(BUNDLE_MANIFEST_FILENAME);
        let before = fs::read(&manifest_path).unwrap();
        let before_manifest: BundleManifest = serde_json::from_slice(&before).unwrap();

        assert!(matches!(
            export_at(&data, &root, false, &cancel, |progress| {
                if progress.phase == TransferPhase::Scanning {
                    cancel.store(true, Ordering::Relaxed);
                }
            }),
            Err(TransferError::Cancelled)
        ));
        assert_eq!(fs::read(&manifest_path).unwrap(), before);
        assert!(inspect_import_at(&root, &AtomicBool::new(false), no_progress).is_ok());
        let generations = fs::read_dir(root.join(SIDECAR_FILENAME).join(GENERATIONS_DIRNAME))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(generations.len(), 1);
        assert_eq!(generations[0].to_string_lossy(), before_manifest.generation);
    }

    #[test]
    fn successful_reexport_atomically_switches_to_the_new_generation() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.jpg"), b"a").unwrap();
        init_data_dir(&data);
        let cancel = AtomicBool::new(false);
        export_at(&data, &root, false, &cancel, no_progress).unwrap();
        let first = read_bundle_manifest(&root, &cancel).unwrap();

        fs::write(root.join("b.jpg"), b"b").unwrap();
        export_at(&data, &root, false, &cancel, no_progress).unwrap();
        let second = read_bundle_manifest(&root, &cancel).unwrap();
        assert_ne!(first.generation, second.generation);
        assert_eq!(second.entry_count, 3);
        let bundle = root.join(SIDECAR_FILENAME);
        assert!(!bundle_generation_dir(&bundle, &first.generation).exists());
        assert!(bundle_generation_dir(&bundle, &second.generation).is_dir());
    }

    #[test]
    fn successful_reexport_collects_unpublished_generations() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.jpg"), b"a").unwrap();
        init_data_dir(&data);
        let cancel = AtomicBool::new(false);
        export_at(&data, &root, false, &cancel, no_progress).unwrap();

        let bundle = root.join(SIDECAR_FILENAME);
        let stale = uuid::Uuid::new_v4().simple().to_string();
        let stale_dir = bundle_generation_dir(&bundle, &stale);
        fs::create_dir_all(stale_dir.join(SHARDS_DIRNAME)).unwrap();
        fs::write(stale_dir.join("orphan.bin"), vec![1_u8; 1024]).unwrap();

        export_at(&data, &root, false, &cancel, no_progress).unwrap();
        assert!(!stale_dir.exists());
        let active = read_bundle_manifest(&root, &cancel).unwrap().generation;
        let generations = fs::read_dir(bundle.join(GENERATIONS_DIRNAME))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(generations, vec![active]);
    }

    #[test]
    fn import_cancel_keeps_completed_entries_without_touching_remaining_entries() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        for name in ["a.jpg", "b.jpg"] {
            fs::write(source.join(name), b"x").unwrap();
            fs::write(destination.join(name), b"x").unwrap();
        }
        init_data_dir(&source_data);
        init_data_dir(&destination_data);
        set_rating(&source_data, &source.join("a.jpg"), 1);
        set_rating(&source_data, &source.join("b.jpg"), 2);
        set_rating(&destination_data, &destination.join("a.jpg"), 4);
        set_rating(&destination_data, &destination.join("b.jpg"), 5);

        let export_cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &export_cancel, no_progress).unwrap();
        copy_sidecar_bundle(&source, &destination);

        let import_cancel = AtomicBool::new(false);
        let mut completed_file = None;
        let summary = import_at(
            &destination_data,
            &destination,
            &import_cancel,
            |progress| {
                // rootに続く最初のfile transaction完了後に止める。read_dir順には依存しない。
                if progress.phase == TransferPhase::Importing && progress.processed == 2 {
                    completed_file = progress.current_path.clone();
                    import_cancel.store(true, Ordering::Relaxed);
                }
            },
        )
        .unwrap();
        assert!(summary.cancelled);
        assert_eq!(summary.applied_entries, 2);
        let completed_file = completed_file.unwrap();
        let (completed_rating, untouched_rating) = if completed_file == "a.jpg" {
            (1, 5)
        } else {
            assert_eq!(completed_file, "b.jpg");
            (2, 4)
        };
        assert_eq!(
            rating(&destination_data, &destination.join(&completed_file)),
            Some(completed_rating)
        );
        let untouched = if completed_file == "a.jpg" {
            "b.jpg"
        } else {
            "a.jpg"
        };
        assert_eq!(
            rating(&destination_data, &destination.join(untouched)),
            Some(untouched_rating)
        );
    }

    #[test]
    fn changed_file_is_reported_and_skipped() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("a.jpg"), b"short").unwrap();
        fs::write(destination.join("a.jpg"), b"different-size").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);
        set_rating(&source_data, &source.join("a.jpg"), 1);
        set_rating(&destination_data, &destination.join("a.jpg"), 5);

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        copy_sidecar_bundle(&source, &destination);
        let preview = inspect_import_at(&destination, &cancel, no_progress).unwrap();
        assert_eq!(preview.changed_files, 1);
        let summary = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(summary.skipped_changed, 1);
        assert_eq!(
            rating(&destination_data, &destination.join("a.jpg")),
            Some(5)
        );
    }

    #[test]
    fn image_other_file_environment_mismatch_is_applied() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        for name in ["plugin.codex_unknown_image", "known.jpg"] {
            fs::write(source.join(name), b"same").unwrap();
            fs::write(destination.join(name), b"same").unwrap();
        }
        init_data_dir(&source_data);
        init_data_dir(&destination_data);
        set_tags(
            &source_data,
            &source.join("plugin.codex_unknown_image"),
            &["plugin-image"],
        );
        set_tags(&source_data, &source.join("known.jpg"), &["built-in-image"]);

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        rewrite_root_shard_entries(&source, &cancel, |entry| match entry.path.as_str() {
            "plugin.codex_unknown_image" => entry.media_kind = PortableMediaKind::Image,
            "known.jpg" => entry.media_kind = PortableMediaKind::OtherFile,
            _ => {}
        });
        copy_sidecar_bundle(&source, &destination);

        let preview = inspect_import_at(&destination, &cancel, no_progress).unwrap();
        assert_eq!(preview.existing_entries, 3);
        assert_eq!(preview.kind_mismatch_entries, 0);
        let summary = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(summary.applied_entries, 3);
        assert_eq!(summary.skipped_kind_mismatch, 0);
        assert_eq!(
            tags(
                &destination_data,
                &destination.join("plugin.codex_unknown_image")
            ),
            vec!["plugin-image"]
        );
        assert_eq!(
            tags(&destination_data, &destination.join("known.jpg")),
            vec!["built-in-image"]
        );
    }

    #[test]
    fn incompatible_media_kind_skips_only_that_entry() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        for name in ["good.png", "mismatch.jpg"] {
            fs::write(source.join(name), b"same").unwrap();
            fs::write(destination.join(name), b"same").unwrap();
        }
        init_data_dir(&source_data);
        init_data_dir(&destination_data);
        set_tags(&source_data, &source.join("good.png"), &["new-good"]);
        set_tags(
            &source_data,
            &source.join("mismatch.jpg"),
            &["must-not-apply"],
        );
        set_tags(
            &destination_data,
            &destination.join("mismatch.jpg"),
            &["old-value"],
        );

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        rewrite_root_shard_entries(&source, &cancel, |entry| {
            if entry.path == "mismatch.jpg" {
                entry.media_kind = PortableMediaKind::Video;
            }
        });
        copy_sidecar_bundle(&source, &destination);

        let preview = inspect_import_at(&destination, &cancel, no_progress).unwrap();
        assert_eq!(preview.existing_entries, 2);
        assert_eq!(preview.kind_mismatch_entries, 1);
        assert_eq!(preview.changed_files, 0);
        let summary = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(summary.applied_entries, 2);
        assert_eq!(summary.skipped_kind_mismatch, 1);
        assert_eq!(summary.skipped_changed, 0);
        assert_eq!(
            tags(&destination_data, &destination.join("good.png")),
            vec!["new-good"]
        );
        assert_eq!(
            tags(&destination_data, &destination.join("mismatch.jpg")),
            vec!["old-value"]
        );
    }

    #[test]
    fn same_path_kind_and_size_are_applied_without_reading_file_content() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let source_data = temp.path().join("source-data");
        let destination_data = temp.path().join("destination-data");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("a.jpg"), b"source").unwrap();
        fs::write(destination.join("a.jpg"), b"target").unwrap();
        init_data_dir(&source_data);
        init_data_dir(&destination_data);
        set_rating(&source_data, &source.join("a.jpg"), 1);
        set_rating(&destination_data, &destination.join("a.jpg"), 5);

        let cancel = AtomicBool::new(false);
        export_at(&source_data, &source, false, &cancel, no_progress).unwrap();
        copy_sidecar_bundle(&source, &destination);
        let preview = inspect_import_at(&destination, &cancel, no_progress).unwrap();
        assert_eq!(preview.existing_entries, 2);
        assert_eq!(preview.changed_files, 0);
        let summary = import_at(&destination_data, &destination, &cancel, no_progress).unwrap();
        assert_eq!(summary.applied_entries, 2);
        assert_eq!(summary.skipped_changed, 0);
        assert_eq!(
            rating(&destination_data, &destination.join("a.jpg")),
            Some(1)
        );
    }

    #[test]
    fn timed_bookmark_thumbnail_validation_rejects_invalid_and_oversized_base64() {
        let mut entry = plain_portable_entry("movie.mp4", PortableMediaKind::Video);
        entry.timed_bookmarks.push(PortableTimedBookmark {
            pts_secs: 1.0,
            title: None,
            thumb_webp_base64: Some("not-base64".to_string()),
            created_at_ms: 0,
        });
        assert!(matches!(
            validate_portable_entry(&entry),
            Err(TransferError::Invalid(message))
                if message.contains("時刻ブックマークの動画サムネの base64 が不正")
        ));

        entry.timed_bookmarks[0].thumb_webp_base64 = Some(
            base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_VIDEO_THUMB_BYTES + 1]),
        );
        assert!(matches!(
            validate_portable_entry(&entry),
            Err(TransferError::Invalid(message))
                if message.contains("時刻ブックマークの動画サムネが大きすぎます")
        ));
    }

    #[test]
    fn undecided_tag_state_rejects_nonempty_physical_and_virtual_tags() {
        let tag = PortableTag {
            name: "invalid".to_string(),
            applied_at: 1,
        };
        let mut physical = plain_portable_entry("image.jpg", PortableMediaKind::Image);
        physical.tags.push(tag.clone());
        assert!(matches!(
            validate_portable_entry(&physical),
            Err(TransferError::Invalid(message))
                if message.contains("タグ未決定の項目にタグがあります")
        ));

        let mut archive = plain_portable_entry("book.zip", PortableMediaKind::Zip);
        archive.virtual_items.push(PortableVirtualItem {
            member_key: "page.jpg".to_string(),
            rating: None,
            tags: vec![tag],
            tags_decided: false,
            page_state: PortablePageState::default(),
        });
        assert!(matches!(
            validate_portable_entry(&archive),
            Err(TransferError::Invalid(message))
                if message.contains("タグ未決定の仮想項目にタグがあります")
        ));
    }

    #[test]
    fn media_kind_rejects_incompatible_metadata_sections() {
        let mut image = plain_portable_entry("image.jpg", PortableMediaKind::Image);
        image.timed_bookmarks.push(PortableTimedBookmark {
            pts_secs: 1.0,
            title: None,
            thumb_webp_base64: None,
            created_at_ms: 0,
        });
        assert!(validate_portable_entry(&image).is_err());

        let mut audio = plain_portable_entry("song.mp3", PortableMediaKind::Audio);
        audio.video_pin = Some(PortableVideoPin {
            pin_pts_secs: 1.0,
            thumb_webp_base64: None,
            thumb_pts_secs: None,
        });
        assert!(validate_portable_entry(&audio).is_err());

        let mut video = plain_portable_entry("movie.mp4", PortableMediaKind::Video);
        video.page_state.rotation_degrees = Some(90);
        assert!(validate_portable_entry(&video).is_err());

        let mut other = plain_portable_entry("notes.txt", PortableMediaKind::OtherFile);
        other.virtual_items.push(PortableVirtualItem {
            member_key: "page.jpg".to_string(),
            rating: None,
            tags: Vec::new(),
            tags_decided: false,
            page_state: PortablePageState::default(),
        });
        assert!(validate_portable_entry(&other).is_err());

        let pdf_rating = |page_num| PortableRating {
            stars: 4,
            rated_at_ms: Some(123),
            kind: Some("pdf_page".to_string()),
            entry_name: None,
            page_num: Some(page_num),
            dir_prefix: None,
            archive_format: None,
            zipdir_is_archive: None,
            zipdir_representative: None,
        };
        let mut pdf = plain_portable_entry("book.pdf", PortableMediaKind::Pdf);
        pdf.virtual_items.push(PortableVirtualItem {
            member_key: "cover.jpg".to_string(),
            rating: None,
            tags: Vec::new(),
            tags_decided: false,
            page_state: PortablePageState::default(),
        });
        assert!(validate_portable_entry(&pdf).is_err());

        pdf.virtual_items[0].member_key = "page_02".to_string();
        assert!(validate_portable_entry(&pdf).is_err());

        pdf.virtual_items[0].member_key = "page_2".to_string();
        pdf.virtual_items[0].rating = Some(pdf_rating(3));
        assert!(validate_portable_entry(&pdf).is_err());

        pdf.virtual_items[0].rating = Some(pdf_rating(2));
        assert!(validate_portable_entry(&pdf).is_ok());

        let mut invalid_container_origin =
            plain_portable_entry("image.jpg", PortableMediaKind::Image);
        invalid_container_origin.container_key_base = PortableVirtualKeyBase::ConvertedCache;
        assert!(validate_portable_entry(&invalid_container_origin).is_err());

        let mut valid_container_origin = plain_portable_entry("book.zip", PortableMediaKind::Zip);
        valid_container_origin.container_key_base = PortableVirtualKeyBase::ConvertedCache;
        assert!(validate_portable_entry(&valid_container_origin).is_ok());
    }

    #[test]
    fn export_rejects_invalid_source_database_values_instead_of_dropping_them() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        let image = root.join("image.jpg");
        fs::write(&image, b"image").unwrap();
        init_data_dir(&data);
        let key = crate::path_key::normalize_keep_drive(&image);
        Connection::open(data.join("rating.db"))
            .unwrap()
            .execute(
                "INSERT INTO ratings (path, stars, source_path)
                 VALUES (?1, 9, ?2)",
                params![key, image.to_string_lossy().as_ref()],
            )
            .unwrap();
        let cancel = AtomicBool::new(false);
        assert!(matches!(
            export_at(&data, &root, false, &cancel, no_progress),
            Err(TransferError::Database(message))
                if message.contains("rating.db") && message.contains("stars=9")
        ));

        Connection::open(data.join("rating.db"))
            .unwrap()
            .execute("DELETE FROM ratings", [])
            .unwrap();
        Connection::open(data.join("rotation.db"))
            .unwrap()
            .execute(
                "INSERT INTO rotations (path, angle) VALUES (?1, 45)",
                [&key],
            )
            .unwrap();
        assert!(matches!(
            export_at(&data, &root, false, &cancel, no_progress),
            Err(TransferError::Database(message))
                if message.contains("rotation.db") && message.contains("angle=45")
        ));

        Connection::open(data.join("rotation.db"))
            .unwrap()
            .execute("DELETE FROM rotations", [])
            .unwrap();
        Connection::open(data.join("mask.db"))
            .unwrap()
            .execute(
                "INSERT INTO masks (path, mask_data, width, height, vectors)
                 VALUES (?1, ?2, 1, 1, 'not json')",
                params![key, vec![0_u8]],
            )
            .unwrap();
        assert!(matches!(
            export_at(&data, &root, false, &cancel, no_progress),
            Err(TransferError::Database(message))
                if message.contains("mask.db") && message.contains("vectors JSON")
        ));

        Connection::open(data.join("mask.db"))
            .unwrap()
            .execute("DELETE FROM masks", [])
            .unwrap();
        Connection::open(data.join("conceal.db"))
            .unwrap()
            .execute(
                "INSERT INTO conceal_entries
                    (page_path, bitmap_w, bitmap_h, bitmap_data, shapes)
                 VALUES (?1, 1, 1, ?2, 'not json')",
                params![key, vec![0_u8]],
            )
            .unwrap();
        assert!(matches!(
            export_at(&data, &root, false, &cancel, no_progress),
            Err(TransferError::Database(message))
                if message.contains("conceal.db") && message.contains("shapes JSON")
        ));
    }

    #[test]
    fn rejects_parent_path_and_newer_version() {
        let base = Manifest {
            format: FORMAT_NAME.to_string(),
            version: FORMAT_VERSION,
            exported_at_ms: 0,
            recursive: false,
            sections: ManifestSections::default(),
            entries: vec![PortableEntry {
                path: "../escape.jpg".to_string(),
                kind: PortableEntryKind::File,
                media_kind: PortableMediaKind::Image,
                virtual_key_base: PortableVirtualKeyBase::Source,
                container_key_base: PortableVirtualKeyBase::Source,
                fingerprint: Some(FileFingerprint {
                    size: 1,
                    modified_ms: None,
                }),
                rating: None,
                tags: Vec::new(),
                tags_decided: false,
                timed_bookmarks: Vec::new(),
                book_bookmarks: Vec::new(),
                page_state: PortablePageState::default(),
                container_state: PortableContainerState::default(),
                nested_containers: Vec::new(),
                video_pin: None,
                virtual_items: Vec::new(),
            }],
        };
        assert!(matches!(
            validate_manifest(&base),
            Err(TransferError::Invalid(_))
        ));
        let mut newer = base;
        newer.entries.clear();
        newer.version += 1;
        assert!(matches!(
            validate_manifest(&newer),
            Err(TransferError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_unsafe_bookmark_page_paths_and_mismatched_kinds() {
        for value in [
            "../../outside.jpg",
            "../outside.jpg",
            "/outside.jpg",
            r"C:\outside.jpg",
            r"chapter\..\outside.jpg",
        ] {
            assert!(matches!(
                validate_manifest(&manifest_with_bookmark(
                    PortableEntryKind::Directory,
                    "image_folder",
                    "relative_path",
                    value,
                )),
                Err(TransferError::Invalid(_))
            ));
        }
        assert!(matches!(
            validate_manifest(&manifest_with_bookmark(
                PortableEntryKind::File,
                "zip",
                "archive_entry",
                "../outside.jpg",
            )),
            Err(TransferError::Invalid(_))
        ));

        for (entry_kind, container_kind, page_kind, value) in [
            (PortableEntryKind::File, "pdf", "relative_path", "001.jpg"),
            (
                PortableEntryKind::Directory,
                "image_folder",
                "pdf_page",
                "0",
            ),
            (PortableEntryKind::File, "zip", "pdf_page", "0"),
            (
                PortableEntryKind::Directory,
                "image_folder",
                "relative_path",
                "001.jpg",
            ),
        ] {
            let manifest = manifest_with_bookmark(entry_kind, container_kind, page_kind, value);
            let should_be_valid = container_kind == "image_folder" && page_kind == "relative_path";
            assert_eq!(validate_manifest(&manifest).is_ok(), should_be_valid);
        }
        assert!(
            validate_manifest(&manifest_with_bookmark(
                PortableEntryKind::File,
                "zip",
                "archive_entry",
                r"chapter\001.jpg",
            ))
            .is_ok()
        );
        assert!(
            validate_manifest(&manifest_with_bookmark(
                PortableEntryKind::File,
                "pdf",
                "pdf_page",
                "42",
            ))
            .is_ok()
        );
    }

    #[test]
    fn rejects_unsafe_or_malformed_v7_state_before_import() {
        let mut base =
            manifest_with_bookmark(PortableEntryKind::File, "zip", "archive_entry", "page.jpg");
        base.entries[0].book_bookmarks.clear();
        assert!(validate_manifest(&base).is_ok());

        let mut invalid_nested = base.clone();
        invalid_nested.entries[0]
            .nested_containers
            .push(PortableNestedContainer {
                member_key: "../outside".to_string(),
                state: PortableContainerState::default(),
            });
        assert!(matches!(
            validate_manifest(&invalid_nested),
            Err(TransferError::Invalid(_))
        ));

        let mut invalid_pin = base.clone();
        invalid_pin.entries[0].container_state.folder_thumb_pin = Some(PortableFolderThumbPin {
            source_kind: "image".to_string(),
            source_rel: "../outside.jpg".to_string(),
            source_entry: None,
            source_page: None,
        });
        assert!(matches!(
            validate_manifest(&invalid_pin),
            Err(TransferError::Invalid(_))
        ));

        let mut invalid_mask = base.clone();
        invalid_mask.entries[0].page_state.mask = Some(crate::sidecar::SidecarMask {
            w: 2,
            h: 2,
            data: base64::engine::general_purpose::STANDARD.encode([1, 2, 3]),
            vectors: Vec::new(),
        });
        assert!(matches!(
            validate_manifest(&invalid_mask),
            Err(TransferError::Invalid(_))
        ));

        let mut invalid_spread = base;
        invalid_spread.entries[0].container_state.spread = Some(PortableSpreadState {
            mode: 99,
            flow: 0,
            direction: 0,
        });
        assert!(matches!(
            validate_manifest(&invalid_spread),
            Err(TransferError::Invalid(_))
        ));
    }

    #[test]
    fn import_rejects_existing_reparse_path_outside_root() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.jpg"), b"x").unwrap();
        let link = root.join("link");
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(&outside, &link).is_err() {
            // Developer Mode / symlink privilege がない環境では作成不能。
            return;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        let manifest = Manifest {
            format: FORMAT_NAME.to_string(),
            version: FORMAT_VERSION,
            exported_at_ms: 0,
            recursive: true,
            sections: ManifestSections::default(),
            entries: vec![PortableEntry {
                path: "link/secret.jpg".to_string(),
                kind: PortableEntryKind::File,
                media_kind: PortableMediaKind::Image,
                virtual_key_base: PortableVirtualKeyBase::Source,
                container_key_base: PortableVirtualKeyBase::Source,
                fingerprint: Some(FileFingerprint {
                    size: 1,
                    modified_ms: None,
                }),
                rating: None,
                tags: Vec::new(),
                tags_decided: false,
                timed_bookmarks: Vec::new(),
                book_bookmarks: Vec::new(),
                page_state: PortablePageState::default(),
                container_state: PortableContainerState::default(),
                nested_containers: Vec::new(),
                video_pin: None,
                virtual_items: Vec::new(),
            }],
        };
        let cancel = AtomicBool::new(false);
        write_manifest_atomic(&root, &manifest, &cancel).unwrap();
        assert!(matches!(
            inspect_import_at(&root, &cancel, no_progress),
            Err(TransferError::Invalid(message)) if message.contains("フォルダ外")
        ));
    }

    #[test]
    fn import_rejects_bookmark_page_reparse_path_outside_container() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let album = root.join("album");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&album).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.jpg"), b"x").unwrap();
        let link = album.join("link");
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(&outside, &link).is_err() {
            return;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        let mut manifest = manifest_with_bookmark(
            PortableEntryKind::Directory,
            "image_folder",
            "relative_path",
            "link/secret.jpg",
        );
        manifest.entries[0].path = "album".to_string();
        let cancel = AtomicBool::new(false);
        write_manifest_atomic(&root, &manifest, &cancel).unwrap();
        assert!(matches!(
            inspect_import_at(&root, &cancel, no_progress),
            Err(TransferError::Invalid(message)) if message.contains("コンテナ外")
        ));
    }

    #[test]
    fn import_rejects_missing_bookmark_page_below_external_reparse_path() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let album = root.join("album");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&album).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let link = album.join("link");
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(&outside, &link).is_err() {
            return;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        let mut manifest = manifest_with_bookmark(
            PortableEntryKind::Directory,
            "image_folder",
            "relative_path",
            "link/future.jpg",
        );
        manifest.entries[0].path = "album".to_string();
        let cancel = AtomicBool::new(false);
        write_manifest_atomic(&root, &manifest, &cancel).unwrap();
        assert!(matches!(
            inspect_import_at(&root, &cancel, no_progress),
            Err(TransferError::Invalid(message)) if message.contains("コンテナ外")
        ));
    }

    #[test]
    fn import_allows_safe_missing_bookmark_page() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let album = root.join("album");
        let data = temp.path().join("data");
        fs::create_dir_all(album.join("chapter")).unwrap();
        init_data_dir(&data);

        let mut manifest = manifest_with_bookmark(
            PortableEntryKind::Directory,
            "image_folder",
            "relative_path",
            "chapter/future.jpg",
        );
        manifest.entries[0].path = "album".to_string();
        let cancel = AtomicBool::new(false);
        write_manifest_atomic(&root, &manifest, &cancel).unwrap();
        let preview = inspect_import_at(&root, &cancel, no_progress).unwrap();
        assert_eq!(preview.existing_entries, 1);
        let summary = import_at(&data, &root, &cancel, no_progress).unwrap();
        assert_eq!(summary.applied_entries, 1);
        assert!(matches!(
            crate::book_bookmarks::resolve_relative_page_path(&album, "chapter/future.jpg"),
            crate::book_bookmarks::RelativePagePathResolution::Missing(_)
        ));
    }
}
