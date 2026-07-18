//! スマートフォルダ: 複数実フォルダから本コンテナを収集する flat snapshot view。

use super::*;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SMART_FOLDER_MAX_DEPTH: u32 = 40;
const SMART_FOLDER_CONFIRM_THRESHOLD: usize = 100_000;
const SMART_FOLDER_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const METADATA_CHUNK_SIZE: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SmartFolderEntryKind {
    ImageFolder,
    Zip,
    Pdf,
    Archive,
}

impl SmartFolderEntryKind {
    fn setting_kind(self) -> crate::settings::SmartFolderContainerKind {
        match self {
            Self::ImageFolder => crate::settings::SmartFolderContainerKind::ImageFolder,
            Self::Zip => crate::settings::SmartFolderContainerKind::Zip,
            Self::Pdf => crate::settings::SmartFolderContainerKind::Pdf,
            Self::Archive => crate::settings::SmartFolderContainerKind::Archive,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SmartFolderEntry {
    /// 表示上の所属 source。場所表示や将来の source facet で使う snapshot 情報。
    #[allow(dead_code)]
    pub(crate) source_id: uuid::Uuid,
    #[allow(dead_code)]
    pub(crate) source_root: PathBuf,
    pub(crate) source_order: usize,
    pub(crate) relative_parent: PathBuf,
    pub(crate) path: PathBuf,
    pub(crate) kind: SmartFolderEntryKind,
    pub(crate) mtime: i64,
    pub(crate) file_size: i64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SmartFolderDiag {
    pub(crate) dirs_scanned: usize,
    pub(crate) containers_found: usize,
    pub(crate) source_failures: usize,
    pub(crate) read_dir_errors: usize,
    pub(crate) entry_errors: usize,
    pub(crate) file_type_errors: usize,
    pub(crate) metadata_errors: usize,
    pub(crate) depth_limit_hits: usize,
    pub(crate) visited_skips: usize,
    pub(crate) duplicates_removed: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SmartFolderProgress {
    pub(crate) phase: SmartFolderPhase,
    pub(crate) completed: usize,
    pub(crate) total: usize,
    pub(crate) dirs_scanned: usize,
    pub(crate) containers_found: usize,
    pub(crate) current_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SmartFolderPhase {
    #[default]
    Scanning,
    Ratings,
    Tags,
    Adjustments,
    Filtering,
    Sorting,
    Building,
}

impl SmartFolderPhase {
    fn label(self) -> &'static str {
        match self {
            Self::Scanning => "検索元を走査中",
            Self::Ratings => "レーティングを読み込み中",
            Self::Tags => "タグを読み込み中",
            Self::Adjustments => "編集状態を確認中",
            Self::Filtering => "条件を適用中",
            Self::Sorting => "並び順を計算中",
            Self::Building => "一覧を構築中",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SmartFolderSnapshot {
    pub(crate) definition: crate::settings::SmartFolderDefinition,
    pub(crate) entries: Arc<Vec<SmartFolderEntry>>,
    pub(crate) diag: SmartFolderDiag,
}

struct SmartFolderScanResult {
    snapshot: SmartFolderSnapshot,
}

enum SmartFolderScanEvent {
    Progress(SmartFolderProgress),
    Done(SmartFolderScanResult),
    Cancelled,
}

pub(crate) struct SmartFolderPending {
    definition_id: uuid::Uuid,
    generation: u64,
    refresh: bool,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<SmartFolderScanEvent>,
}

impl SmartFolderPending {
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(crate) struct SmartFolderConfirmPending {
    snapshot: SmartFolderSnapshot,
    generation: u64,
    refresh: bool,
}

struct PreparedSmartFolder {
    snapshot: SmartFolderSnapshot,
    items: Vec<GridItem>,
    image_metas: Vec<Option<(i64, i64)>>,
    metadata: super::subfolder_expansion::PreparedSubfolderMetadata,
    refresh: bool,
}

enum SmartFolderPrepareEvent {
    Progress(SmartFolderProgress),
    Done(PreparedSmartFolder),
    Cancelled,
    Error(String),
}

pub(crate) struct SmartFolderPreparePending {
    definition_id: uuid::Uuid,
    generation: u64,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<SmartFolderPrepareEvent>,
}

impl SmartFolderPreparePending {
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone)]
struct ActiveSource {
    id: uuid::Uuid,
    root: PathBuf,
    registration_order: usize,
}

fn smart_folder_root() -> PathBuf {
    crate::data_dir::get().join("__smart_folder__")
}

pub(crate) fn smart_folder_synthetic_path(id: uuid::Uuid) -> PathBuf {
    smart_folder_root().join(id.to_string())
}

pub(crate) fn smart_folder_id_from_synthetic_path(path: &Path) -> Option<uuid::Uuid> {
    let relative = path.strip_prefix(smart_folder_root()).ok()?;
    if relative.components().count() != 1 {
        return None;
    }
    uuid::Uuid::parse_str(relative.file_name()?.to_str()?).ok()
}

pub(crate) fn is_smart_folder_synthetic_path(path: &Path) -> bool {
    smart_folder_id_from_synthetic_path(path).is_some()
}

fn path_depth(path: &Path) -> usize {
    path.components().count()
}

fn active_sources(definition: &crate::settings::SmartFolderDefinition) -> Vec<ActiveSource> {
    let mut sources: Vec<_> = definition
        .sources
        .iter()
        .enumerate()
        .filter(|(_, source)| source.enabled && !source.path.as_os_str().is_empty())
        .map(|(registration_order, source)| ActiveSource {
            id: source.id,
            root: source.path.clone(),
            registration_order,
        })
        .collect();
    // 親子 root が重なるとき、具体的な root を先に訪問して共通 walker の visited set に
    // 登録する。親 root から同じ subtree に再侵入しないため、所属も具体的 root で安定する。
    sources.sort_by(|a, b| {
        path_depth(&b.root)
            .cmp(&path_depth(&a.root))
            .then_with(|| a.registration_order.cmp(&b.registration_order))
    });
    sources
}

fn entry_relative_parent(path: &Path, source_root: &Path) -> PathBuf {
    path.parent()
        .and_then(|parent| parent.strip_prefix(source_root).ok())
        .unwrap_or_else(|| Path::new(""))
        .to_path_buf()
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

fn passes_cheap_filter(
    entry: &SmartFolderEntry,
    filter: &crate::settings::SmartFolderFilter,
    now: i64,
) -> bool {
    if !filter.kinds.is_empty() && !filter.kinds.contains(&entry.kind.setting_kind()) {
        return false;
    }
    let name = entry
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !filter.name_contains.is_empty()
        && !name
            .to_lowercase()
            .contains(&filter.name_contains.to_lowercase())
    {
        return false;
    }
    if !filter.extensions.is_empty() {
        let extension = entry
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        // 画像のみフォルダには拡張子が無いので、拡張子条件があれば対象外。
        if extension.is_empty() || !filter.extensions.contains(&extension) {
            return false;
        }
    }
    if let Some(preset) = filter.date_preset {
        let earliest = now.saturating_sub(preset.seconds());
        if entry.mtime < earliest {
            return false;
        }
    }
    if let Some(preset) = filter.size_preset {
        let (min, max) = preset.range_bytes();
        let size = entry.file_size.max(0) as u64;
        if size < min || max.is_some_and(|max| size >= max) {
            return false;
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn scan_one_directory(
    source: &ActiveSource,
    dir: &Path,
    entries: std::fs::ReadDir,
    show_hidden_files: bool,
    include_convertible_archives: bool,
    cancel: &AtomicBool,
    result: &mut Vec<SmartFolderEntry>,
    diag: &mut SmartFolderDiag,
) -> Vec<PathBuf> {
    let mut subdirs = Vec::new();
    let mut image_count = 0usize;
    let mut only_images = true;
    let mut image_total_size = 0i64;
    let mut image_latest_mtime = 0i64;

    for entry_result in entries {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(_) => {
                diag.entry_errors += 1;
                continue;
            }
        };
        if crate::fs_entry::should_hide_fs_entry(&entry, show_hidden_files) {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                diag.file_type_errors += 1;
                continue;
            }
        };
        let entry_kind = crate::fs_entry::classify_dir_entry(&entry, &file_type);
        let path = entry.path();
        if entry_kind.is_directory() {
            if !crate::video::upscale::paths::has_work_dir_suffix(&path) {
                subdirs.push(path);
                only_images = false;
            }
            continue;
        }
        if !entry_kind.is_file() || crate::folder_tree::is_apple_double(&path) {
            continue;
        }
        let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
            continue;
        };
        let extension = extension.to_ascii_lowercase();
        let metadata = entry.metadata().ok();
        let mtime = metadata
            .as_ref()
            .map(crate::ui_helpers::mtime_secs)
            .unwrap_or(0);
        let file_size = metadata
            .as_ref()
            .map(|metadata| metadata.len() as i64)
            .unwrap_or(0);
        if metadata.is_none() {
            diag.metadata_errors += 1;
        }

        if crate::folder_tree::is_recognized_image_ext(&extension) {
            image_count += 1;
            image_total_size = image_total_size.saturating_add(file_size);
            image_latest_mtime = image_latest_mtime.max(mtime);
            continue;
        }
        if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension.as_str())
            || crate::folder_tree::is_audio_ext(&extension)
        {
            only_images = false;
            continue;
        }
        let kind = if crate::folder_tree::is_zip_extension(&extension) {
            Some(SmartFolderEntryKind::Zip)
        } else if extension == "pdf" {
            Some(SmartFolderEntryKind::Pdf)
        } else if include_convertible_archives
            && crate::archive_converter::ArchiveFormat::from_extension(&extension).is_some()
        {
            Some(SmartFolderEntryKind::Archive)
        } else {
            None
        };
        if let Some(kind) = kind {
            only_images = false;
            result.push(SmartFolderEntry {
                source_id: source.id,
                source_root: source.root.clone(),
                source_order: source.registration_order,
                relative_parent: entry_relative_parent(&path, &source.root),
                path,
                kind,
                mtime,
                file_size,
            });
        }
    }

    if only_images && image_count > 0 {
        let dir_metadata = std::fs::metadata(dir).ok();
        let dir_mtime = dir_metadata
            .as_ref()
            .map(crate::ui_helpers::mtime_secs)
            .unwrap_or(image_latest_mtime);
        result.push(SmartFolderEntry {
            source_id: source.id,
            source_root: source.root.clone(),
            source_order: source.registration_order,
            relative_parent: entry_relative_parent(dir, &source.root),
            path: dir.to_path_buf(),
            kind: SmartFolderEntryKind::ImageFolder,
            mtime: dir_mtime,
            file_size: image_total_size,
        });
    }
    subdirs
}

fn scan_smart_folder(
    definition: crate::settings::SmartFolderDefinition,
    show_hidden_files: bool,
    include_convertible_archives: bool,
    cancel: &AtomicBool,
    io_sem: &crate::io_semaphore::GlobalIoSemaphore,
    activity_gate: &crate::activity_gate::ActivityGate,
    tx: &mpsc::Sender<SmartFolderScanEvent>,
) -> Option<SmartFolderScanResult> {
    let sources = active_sources(&definition);
    let roots: Vec<_> = sources.iter().map(|source| source.root.clone()).collect();
    let mut entries = Vec::new();
    let mut diag = SmartFolderDiag::default();
    let root_scanned = std::cell::RefCell::new(vec![false; sources.len()]);
    let containers_found = std::cell::Cell::new(0usize);
    let last_progress = std::cell::Cell::new(Instant::now());
    let walk_diag = super::recursive_snapshot_scan::walk_snapshot_roots(
        &roots,
        SMART_FOLDER_MAX_DEPTH,
        cancel,
        Some(io_sem),
        Some(activity_gate),
        |root_index, dir, read_dir, cancel| {
            if crate::folder_tree::path_eq(dir, &sources[root_index].root) {
                root_scanned.borrow_mut()[root_index] = true;
            }
            let subdirs = scan_one_directory(
                &sources[root_index],
                dir,
                read_dir,
                show_hidden_files,
                include_convertible_archives,
                cancel,
                &mut entries,
                &mut diag,
            );
            containers_found.set(entries.len());
            subdirs
        },
        |walk_diag, current_dir| {
            if current_dir.is_none()
                || last_progress.get().elapsed() >= SMART_FOLDER_PROGRESS_INTERVAL
            {
                let _ = tx.send(SmartFolderScanEvent::Progress(SmartFolderProgress {
                    phase: SmartFolderPhase::Scanning,
                    dirs_scanned: walk_diag.dirs_scanned,
                    containers_found: containers_found.get(),
                    current_dir: current_dir.map(Path::to_path_buf),
                    ..SmartFolderProgress::default()
                }));
                last_progress.set(Instant::now());
            }
        },
    );
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    diag.dirs_scanned = walk_diag.dirs_scanned;
    diag.read_dir_errors = walk_diag.read_dir_errors;
    diag.depth_limit_hits = walk_diag.depth_limit_hits;
    diag.visited_skips = walk_diag.visited_skips;
    diag.source_failures = root_scanned.borrow().iter().filter(|seen| !**seen).count();

    let now = now_unix_secs();
    entries.retain(|entry| passes_cheap_filter(entry, &definition.filter, now));
    let before_dedupe = entries.len();
    let mut seen = HashSet::new();
    entries.retain(|entry| seen.insert(crate::path_key::normalize_keep_drive(&entry.path)));
    diag.duplicates_removed = before_dedupe.saturating_sub(entries.len());
    diag.containers_found = entries.len();
    Some(SmartFolderScanResult {
        snapshot: SmartFolderSnapshot {
            definition,
            entries: Arc::new(entries),
            diag,
        },
    })
}

#[allow(clippy::too_many_arguments)]
fn spawn_smart_folder_scan(
    definition: crate::settings::SmartFolderDefinition,
    generation: u64,
    refresh: bool,
    show_hidden_files: bool,
    include_convertible_archives: bool,
    io_sem: Arc<crate::io_semaphore::GlobalIoSemaphore>,
    activity_gate: Arc<crate::activity_gate::ActivityGate>,
) -> Result<SmartFolderPending, String> {
    let definition_id = definition.id;
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("smart-folder-scan".into())
        .spawn(move || {
            let event = match scan_smart_folder(
                definition,
                show_hidden_files,
                include_convertible_archives,
                &cancel_worker,
                &io_sem,
                &activity_gate,
                &tx,
            ) {
                Some(result) if !cancel_worker.load(Ordering::Relaxed) => {
                    SmartFolderScanEvent::Done(result)
                }
                _ => SmartFolderScanEvent::Cancelled,
            };
            let _ = tx.send(event);
        })
        .map_err(|error| format!("スマートフォルダ走査を開始できませんでした: {error}"))?;
    Ok(SmartFolderPending {
        definition_id,
        generation,
        refresh,
        cancel,
        rx,
    })
}

fn metadata_filter_passes(
    filter: &crate::settings::SmartFolderFilter,
    key: &str,
    ratings: &HashMap<String, u8>,
    tags: &HashMap<String, Vec<String>>,
) -> bool {
    let rating = ratings.get(key).copied().unwrap_or(0).min(5) as usize;
    if !filter.ratings[rating] {
        return false;
    }
    let item_tags = tags.get(key).map(Vec::as_slice).unwrap_or(&[]);
    if !filter.tags.is_empty() || filter.include_untagged {
        let tag_match = filter.tags.iter().any(|tag| item_tags.contains(tag));
        if !tag_match && !(filter.include_untagged && item_tags.is_empty()) {
            return false;
        }
    }
    true
}

struct SmartEntrySortKey {
    name: crate::filename_sort::SortNameKey,
    relative_parent: crate::filename_sort::SortNameKey,
}

fn prepare_smart_folder(
    snapshot: SmartFolderSnapshot,
    refresh: bool,
    load_ratings: bool,
    load_tags: bool,
    load_local_adjust: bool,
    cancel: &AtomicBool,
    tx: &mpsc::Sender<SmartFolderPrepareEvent>,
) -> Result<Option<PreparedSmartFolder>, String> {
    let total = snapshot.entries.len();
    let report = |phase, completed| {
        let _ = tx.send(SmartFolderPrepareEvent::Progress(SmartFolderProgress {
            phase,
            completed,
            total,
            containers_found: total,
            ..SmartFolderProgress::default()
        }));
    };
    let keys: Vec<String> = snapshot
        .entries
        .iter()
        .map(|entry| crate::adjustment_db::normalize_path(&entry.path))
        .collect();

    let mut ratings = HashMap::new();
    if load_ratings {
        report(SmartFolderPhase::Ratings, 0);
        let db = crate::rating_db::RatingDb::open_readonly(crate::rating_db::RatingDb::db_path())
            .map_err(|error| format!("レーティング DB を読み込めませんでした: {error}"))?;
        for (chunk_index, chunk) in keys.chunks(METADATA_CHUNK_SIZE).enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            ratings.extend(db.get_many(chunk));
            report(
                SmartFolderPhase::Ratings,
                ((chunk_index + 1) * METADATA_CHUNK_SIZE).min(total),
            );
        }
    }

    let mut tags = HashMap::new();
    if load_tags {
        report(SmartFolderPhase::Tags, 0);
        let db = crate::tags_db::TagsDb::open_readonly(&crate::tags_db::TagsDb::db_path())
            .map_err(|error| format!("タグ DB を読み込めませんでした: {error}"))?;
        for (chunk_index, chunk) in keys.chunks(METADATA_CHUNK_SIZE).enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            tags.extend(db.get_many_display_tags(chunk));
            report(
                SmartFolderPhase::Tags,
                ((chunk_index + 1) * METADATA_CHUNK_SIZE).min(total),
            );
        }
    }

    let mut local_adjust = HashSet::new();
    if load_local_adjust {
        report(SmartFolderPhase::Adjustments, 0);
        let db = crate::local_adjust_db::LocalAdjustDb::open_readonly(
            &crate::local_adjust_db::LocalAdjustDb::db_path(),
        )
        .map_err(|error| format!("補正レイヤー DB を読み込めませんでした: {error}"))?;
        for (chunk_index, chunk) in keys.chunks(METADATA_CHUNK_SIZE).enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            local_adjust.extend(db.load_existing_layer_keys(chunk));
            report(
                SmartFolderPhase::Adjustments,
                ((chunk_index + 1) * METADATA_CHUNK_SIZE).min(total),
            );
        }
    }

    report(SmartFolderPhase::Filtering, 0);
    let mut included = Vec::with_capacity(total);
    for (index, key) in keys.iter().enumerate() {
        if index.is_multiple_of(METADATA_CHUNK_SIZE) {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            report(SmartFolderPhase::Filtering, index);
        }
        if metadata_filter_passes(&snapshot.definition.filter, key, &ratings, &tags) {
            included.push(index);
        }
    }

    report(SmartFolderPhase::Sorting, 0);
    let sort = snapshot.definition.sort;
    let grouping = snapshot.definition.grouping;
    let sort_keys: Vec<_> = snapshot
        .entries
        .iter()
        .map(|entry| {
            let name = entry
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            SmartEntrySortKey {
                name: sort.name_key(name),
                relative_parent: crate::filename_sort::SortNameKey::file_name(
                    &entry.relative_parent.to_string_lossy(),
                ),
            }
        })
        .collect();
    let sorted_positions = super::recursive_snapshot_scan::cancelable_sorted_indices(
        included.len(),
        cancel,
        |a_position, b_position| {
            let a_index = included[a_position];
            let b_index = included[b_position];
            let a = &snapshot.entries[a_index];
            let b = &snapshot.entries[b_index];
            let ak = &sort_keys[a_index];
            let bk = &sort_keys[b_index];
            let within = || {
                sort.compare_name_keys(&ak.name, a.mtime, &bk.name, b.mtime)
                    .then_with(|| a.path.cmp(&b.path))
            };
            match grouping {
                crate::settings::SubfolderExpansionOrder::Flat => within()
                    .then_with(|| a.source_order.cmp(&b.source_order))
                    .then_with(|| ak.relative_parent.compare_file_name(&bk.relative_parent)),
                crate::settings::SubfolderExpansionOrder::FolderGrouped => a
                    .source_order
                    .cmp(&b.source_order)
                    .then_with(|| ak.relative_parent.compare_file_name(&bk.relative_parent))
                    .then_with(within),
            }
        },
        |completed| report(SmartFolderPhase::Sorting, completed),
    );
    let Some(sorted_positions) = sorted_positions else {
        return Ok(None);
    };

    report(SmartFolderPhase::Building, 0);
    let mut items = Vec::with_capacity(sorted_positions.len());
    let mut image_metas = Vec::with_capacity(sorted_positions.len());
    let mut rating_cache = HashMap::new();
    let mut tags_cache = HashMap::new();
    let mut local_adjust_pages = HashSet::new();
    for (display_index, included_position) in sorted_positions.into_iter().enumerate() {
        if display_index.is_multiple_of(METADATA_CHUNK_SIZE) {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            report(SmartFolderPhase::Building, display_index);
        }
        let entry_index = included[included_position];
        let entry = &snapshot.entries[entry_index];
        let item = match entry.kind {
            SmartFolderEntryKind::ImageFolder => GridItem::Folder(entry.path.clone()),
            SmartFolderEntryKind::Zip => GridItem::ZipFile(entry.path.clone()),
            SmartFolderEntryKind::Pdf => GridItem::PdfFile(entry.path.clone()),
            SmartFolderEntryKind::Archive => {
                let Some(format) = entry
                    .path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .and_then(crate::archive_converter::ArchiveFormat::from_extension)
                else {
                    continue;
                };
                GridItem::ConvertibleArchive {
                    path: entry.path.clone(),
                    format,
                }
            }
        };
        let key = &keys[entry_index];
        items.push(item);
        image_metas.push(Some((entry.mtime, entry.file_size)));
        if let Some(rating) = ratings.get(key).copied().filter(|rating| *rating > 0) {
            rating_cache.insert(display_index, rating);
        }
        if let Some(item_tags) = tags.get(key).filter(|tags| !tags.is_empty()) {
            tags_cache.insert(key.clone(), item_tags.clone());
        }
        if local_adjust.contains(key) {
            local_adjust_pages.insert(display_index);
        }
    }
    Ok(Some(PreparedSmartFolder {
        snapshot,
        items,
        image_metas,
        metadata: super::subfolder_expansion::PreparedSubfolderMetadata {
            rating_cache,
            tags_cache,
            local_adjust_pages,
            video_pin_blobs: HashMap::new(),
            legacy_paths: Vec::new(),
        },
        refresh,
    }))
}

fn spawn_smart_folder_prepare(
    snapshot: SmartFolderSnapshot,
    generation: u64,
    refresh: bool,
    load_ratings: bool,
    load_tags: bool,
    load_local_adjust: bool,
) -> Result<SmartFolderPreparePending, String> {
    let definition_id = snapshot.definition.id;
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("smart-folder-prepare".into())
        .spawn(move || {
            let event = match prepare_smart_folder(
                snapshot,
                refresh,
                load_ratings,
                load_tags,
                load_local_adjust,
                &cancel_worker,
                &tx,
            ) {
                Ok(Some(prepared)) if !cancel_worker.load(Ordering::Relaxed) => {
                    SmartFolderPrepareEvent::Done(prepared)
                }
                Ok(_) => SmartFolderPrepareEvent::Cancelled,
                Err(message) => SmartFolderPrepareEvent::Error(message),
            };
            let _ = tx.send(event);
        })
        .map_err(|error| format!("スマートフォルダの表示準備を開始できませんでした: {error}"))?;
    Ok(SmartFolderPreparePending {
        definition_id,
        generation,
        cancel,
        rx,
    })
}

impl App {
    pub(crate) fn open_smart_folder(&mut self, definition_id: uuid::Uuid, refresh: bool) {
        let Some(definition) = self
            .settings
            .smart_folders
            .iter()
            .find(|definition| definition.id == definition_id)
            .cloned()
        else {
            self.show_feedback_toast("スマートフォルダが見つかりません".into());
            return;
        };
        if active_sources(&definition).is_empty() {
            self.show_feedback_toast("有効な検索元フォルダを追加してください".into());
            self.open_smart_folder_manager(Some(definition_id));
            return;
        }
        self.cancel_smart_folder_pending();
        self.smart_folder_generation = self.smart_folder_generation.wrapping_add(1);
        let generation = self.smart_folder_generation;
        if !self.items_are_smart_folder_view {
            self.smart_folder_saved_folder = self
                .effective_folder()
                .filter(|path| !is_synthetic_view_path(path) && path.is_dir());
        }
        let io_sem = self
            .indexer_manager
            .as_ref()
            .map(|manager| manager.io_sem())
            .unwrap_or_else(|| {
                Arc::new(crate::io_semaphore::GlobalIoSemaphore::new(
                    self.settings.indexer_speed_profile.io_permits().max(1),
                ))
            });
        match spawn_smart_folder_scan(
            definition.clone(),
            generation,
            refresh,
            self.settings.show_hidden_files,
            !self.settings.archive_file_handling_ignores_convertible(),
            io_sem,
            Arc::clone(&self.activity_gate),
        ) {
            Ok(pending) => {
                self.smart_folder_progress = Some(SmartFolderProgress::default());
                self.smart_folder_pending = Some(pending);
                self.show_feedback_toast(format!(
                    "スマートフォルダ「{}」を走査中",
                    definition.name
                ));
            }
            Err(message) => self.show_feedback_toast(message),
        }
    }

    pub(crate) fn start_smart_folder_prepare(
        &mut self,
        snapshot: SmartFolderSnapshot,
        refresh: bool,
    ) {
        if let Some(pending) = self.smart_folder_prepare_pending.take() {
            pending.cancel();
        }
        let generation = self.smart_folder_generation;
        match spawn_smart_folder_prepare(
            snapshot,
            generation,
            refresh,
            self.rating_db.is_some(),
            self.tags_db.is_some(),
            self.local_adjust_db.is_some(),
        ) {
            Ok(pending) => {
                self.smart_folder_progress = Some(SmartFolderProgress {
                    phase: SmartFolderPhase::Filtering,
                    ..SmartFolderProgress::default()
                });
                self.smart_folder_prepare_pending = Some(pending);
            }
            Err(message) => self.show_feedback_toast(message),
        }
    }

    pub(crate) fn cancel_smart_folder_pending(&mut self) {
        if let Some(pending) = self.smart_folder_pending.take() {
            pending.cancel();
        }
        if let Some(pending) = self.smart_folder_prepare_pending.take() {
            pending.cancel();
        }
        self.smart_folder_confirm_pending = None;
        self.smart_folder_progress = None;
    }

    /// 定義変更時に古い snapshot / 進行中 worker を無効化する。
    pub(crate) fn invalidate_smart_folder_definition(&mut self, definition_id: uuid::Uuid) {
        self.smart_folder_snapshots.remove(&definition_id);
        let pending_matches = self
            .smart_folder_pending
            .as_ref()
            .is_some_and(|pending| pending.definition_id == definition_id)
            || self
                .smart_folder_prepare_pending
                .as_ref()
                .is_some_and(|pending| pending.definition_id == definition_id)
            || self
                .smart_folder_confirm_pending
                .as_ref()
                .is_some_and(|pending| pending.snapshot.definition.id == definition_id);
        if pending_matches {
            self.cancel_smart_folder_pending();
        }
    }

    /// 表示中の定義が削除された場合は orphan synthetic view を残さず元の場所へ戻す。
    pub(crate) fn forget_smart_folder_definition(&mut self, definition_id: uuid::Uuid) {
        self.invalidate_smart_folder_definition(definition_id);
        if self.current_smart_folder_id != Some(definition_id) {
            return;
        }
        let return_to = self.smart_folder_saved_folder.clone();
        self.clear_smart_folder_view_state();
        if let Some(path) = return_to {
            self.load_folder(path);
        } else {
            self.enter_drive_list(None);
        }
    }

    pub(crate) fn clear_smart_folder_view_state(&mut self) {
        self.items_are_smart_folder_view = false;
        self.current_smart_folder_id = None;
        self.smart_folder_saved_folder = None;
    }

    pub(crate) fn smart_folder_back_nav(&self) -> Option<crate::ui_main::AddressBarNav> {
        if !self.items_are_smart_folder_view {
            return None;
        }
        Some(
            self.smart_folder_saved_folder
                .clone()
                .map(crate::ui_main::AddressBarNav::Direct)
                .unwrap_or(crate::ui_main::AddressBarNav::DriveList(None)),
        )
    }

    pub(crate) fn restore_smart_folder_for_synthetic_path(&mut self, path: &Path) -> bool {
        let Some(definition_id) = smart_folder_id_from_synthetic_path(path) else {
            return false;
        };
        let Some(definition) = self
            .settings
            .smart_folders
            .iter()
            .find(|definition| definition.id == definition_id)
            .cloned()
        else {
            return false;
        };
        if let Some(snapshot) = self
            .smart_folder_snapshots
            .get(&definition_id)
            .filter(|snapshot| snapshot.definition == definition)
            .cloned()
        {
            self.current_smart_folder_id = Some(definition_id);
            self.start_smart_folder_prepare(snapshot, false);
        } else {
            self.open_smart_folder(definition_id, false);
        }
        true
    }

    pub(crate) fn poll_smart_folder(&mut self, ctx: &egui::Context) {
        if self.smart_folder_pending.is_some() || self.smart_folder_prepare_pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        loop {
            let event = match self.smart_folder_pending.as_ref() {
                Some(pending) => pending.rx.try_recv(),
                None => break,
            };
            match event {
                Ok(SmartFolderScanEvent::Progress(progress)) => {
                    self.smart_folder_progress = Some(progress);
                }
                Ok(SmartFolderScanEvent::Done(result)) => {
                    let Some(pending) = self.smart_folder_pending.take() else {
                        break;
                    };
                    let current_matches = pending.generation == self.smart_folder_generation
                        && self.settings.smart_folders.iter().any(|definition| {
                            definition.id == pending.definition_id
                                && definition == &result.snapshot.definition
                        });
                    if !current_matches {
                        self.smart_folder_progress = None;
                        break;
                    }
                    if result.snapshot.diag.source_failures
                        == active_sources(&result.snapshot.definition).len()
                    {
                        self.smart_folder_progress = None;
                        self.show_feedback_toast(
                            "スマートフォルダの検索元を1件も読み込めませんでした".into(),
                        );
                    } else if result.snapshot.entries.len() >= SMART_FOLDER_CONFIRM_THRESHOLD {
                        self.smart_folder_confirm_pending = Some(SmartFolderConfirmPending {
                            snapshot: result.snapshot,
                            generation: pending.generation,
                            refresh: pending.refresh,
                        });
                    } else {
                        self.start_smart_folder_prepare(result.snapshot, pending.refresh);
                    }
                    break;
                }
                Ok(SmartFolderScanEvent::Cancelled) => {
                    self.smart_folder_pending = None;
                    self.smart_folder_progress = None;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.smart_folder_pending = None;
                    self.smart_folder_progress = None;
                    self.show_feedback_toast("スマートフォルダ走査が中断されました".into());
                    break;
                }
            }
        }

        loop {
            let event = match self.smart_folder_prepare_pending.as_ref() {
                Some(pending) => pending.rx.try_recv(),
                None => break,
            };
            match event {
                Ok(SmartFolderPrepareEvent::Progress(progress)) => {
                    self.smart_folder_progress = Some(progress);
                }
                Ok(SmartFolderPrepareEvent::Done(prepared)) => {
                    let Some(pending) = self.smart_folder_prepare_pending.take() else {
                        break;
                    };
                    if pending.generation == self.smart_folder_generation
                        && pending.definition_id == prepared.snapshot.definition.id
                        && self.settings.smart_folders.iter().any(|definition| {
                            definition.id == pending.definition_id
                                && definition == &prepared.snapshot.definition
                        })
                    {
                        self.install_prepared_smart_folder(prepared);
                    }
                    break;
                }
                Ok(SmartFolderPrepareEvent::Cancelled) => {
                    self.smart_folder_prepare_pending = None;
                    self.smart_folder_progress = None;
                    break;
                }
                Ok(SmartFolderPrepareEvent::Error(message)) => {
                    self.smart_folder_prepare_pending = None;
                    self.smart_folder_progress = None;
                    self.show_feedback_toast(message);
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.smart_folder_prepare_pending = None;
                    self.smart_folder_progress = None;
                    self.show_feedback_toast("スマートフォルダの表示準備が中断されました".into());
                    break;
                }
            }
        }
    }

    fn install_prepared_smart_folder(&mut self, prepared: PreparedSmartFolder) {
        let PreparedSmartFolder {
            snapshot,
            items,
            image_metas,
            metadata,
            refresh,
        } = prepared;
        let definition_id = snapshot.definition.id;
        let definition_name = snapshot.definition.name.clone();
        let diag = snapshot.diag.clone();
        let item_count = items.len();
        self.settings.sort_order = snapshot.definition.sort;
        self.settings.grid_view_mode = snapshot.definition.view_mode;
        self.start_loading_subfolder_items(
            smart_folder_synthetic_path(definition_id),
            items,
            image_metas,
            Vec::new(),
            metadata,
        );
        self.items_are_subfolder_expansion_view = false;
        self.items_are_smart_folder_view = true;
        self.current_smart_folder_id = Some(definition_id);
        self.smart_folder_snapshots.insert(definition_id, snapshot);
        self.smart_folder_progress = None;
        self.address = format!("スマートフォルダ: {definition_name}");
        if let Some(&index) = self.visible_indices.first() {
            self.selected = Some(index);
            self.scroll_to_selected = true;
        }
        let skipped = diag.source_failures
            + diag.read_dir_errors
            + diag.entry_errors
            + diag.file_type_errors
            + diag.metadata_errors
            + diag.depth_limit_hits;
        let prefix = if refresh { "更新" } else { "表示" };
        if skipped > 0 {
            self.show_feedback_toast(format!(
                "スマートフォルダを{prefix}: {item_count}件 (読めなかった項目 {skipped}件)"
            ));
        } else {
            self.show_feedback_toast(format!("スマートフォルダを{prefix}: {item_count}件"));
        }
    }

    pub(crate) fn update_current_smart_folder_sort(&mut self) -> bool {
        if !self.items_are_smart_folder_view {
            return false;
        }
        let Some(id) = self.current_smart_folder_id else {
            return true;
        };
        if let Some(definition) = self
            .settings
            .smart_folders
            .iter_mut()
            .find(|definition| definition.id == id)
        {
            definition.sort = self.settings.sort_order;
            definition.view_mode = self.settings.grid_view_mode;
        }
        self.settings.save();
        if let Some(snapshot) = self.smart_folder_snapshots.get_mut(&id) {
            snapshot.definition.sort = self.settings.sort_order;
            snapshot.definition.view_mode = self.settings.grid_view_mode;
        }
        if let Some(snapshot) = self.smart_folder_snapshots.get(&id).cloned() {
            self.start_smart_folder_prepare(snapshot, false);
        }
        true
    }

    pub(crate) fn render_smart_folder_overlay(&mut self, ctx: &egui::Context) {
        if let Some(confirm) = self.smart_folder_confirm_pending.as_ref() {
            let count = confirm.snapshot.entries.len();
            let name = confirm.snapshot.definition.name.clone();
            let mut proceed = false;
            let mut cancel = false;
            egui::Modal::new(egui::Id::new("smart_folder_large_confirm")).show(ctx, |ui| {
                ui.heading("スマートフォルダを表示");
                ui.label(format!("「{name}」の結果は {count} 件です。"));
                ui.label("一覧の準備に時間とメモリを使用する可能性があります。");
                ui.horizontal(|ui| {
                    if ui.button("続行").clicked() {
                        proceed = true;
                    }
                    if ui.button("中止").clicked() {
                        cancel = true;
                    }
                });
            });
            if proceed {
                if let Some(confirm) = self.smart_folder_confirm_pending.take() {
                    if confirm.generation == self.smart_folder_generation {
                        self.start_smart_folder_prepare(confirm.snapshot, confirm.refresh);
                    }
                }
            } else if cancel {
                self.smart_folder_confirm_pending = None;
                self.smart_folder_progress = None;
            }
            return;
        }
        let Some(progress) = self.smart_folder_progress.clone() else {
            return;
        };
        let mut cancel = false;
        egui::Area::new(egui::Id::new("smart_folder_progress_overlay"))
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(12.0, -42.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(360.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(progress.phase.label());
                        if ui.small_button("中止").clicked() {
                            cancel = true;
                        }
                    });
                    if progress.phase == SmartFolderPhase::Scanning {
                        ui.label(format!(
                            "{}フォルダ / {}件",
                            progress.dirs_scanned, progress.containers_found
                        ));
                        if let Some(path) = progress.current_dir {
                            ui.label(egui::RichText::new(path.to_string_lossy()).small().weak());
                        }
                    } else if progress.total > 0 {
                        ui.add(
                            egui::ProgressBar::new(
                                progress.completed as f32 / progress.total as f32,
                            )
                            .show_percentage(),
                        );
                    }
                });
            });
        if cancel {
            self.cancel_smart_folder_pending();
            self.show_feedback_toast("スマートフォルダ処理を中止しました".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: uuid::Uuid, path: &str, enabled: bool) -> crate::settings::SmartFolderSource {
        crate::settings::SmartFolderSource {
            id,
            path: PathBuf::from(path),
            enabled,
        }
    }

    fn smart_entry(path: &str, source_order: usize, relative_parent: &str) -> SmartFolderEntry {
        SmartFolderEntry {
            source_id: uuid::Uuid::new_v4(),
            source_root: PathBuf::from(r"C:\Books"),
            source_order,
            relative_parent: PathBuf::from(relative_parent),
            path: PathBuf::from(path),
            kind: SmartFolderEntryKind::Zip,
            mtime: 0,
            file_size: 1,
        }
    }

    #[test]
    fn active_sources_put_specific_roots_first_but_keep_registration_order() {
        let mut definition = crate::settings::SmartFolderDefinition::new("books");
        definition.sources = vec![
            source(uuid::Uuid::new_v4(), r"C:\Books", true),
            source(uuid::Uuid::new_v4(), r"C:\Books\Done", true),
            source(uuid::Uuid::new_v4(), r"D:\Download", false),
        ];
        let sources = active_sources(&definition);
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].root, PathBuf::from(r"C:\Books\Done"));
        assert_eq!(sources[0].registration_order, 1);
        assert_eq!(sources[1].registration_order, 0);
    }

    #[test]
    fn cheap_filter_matches_kind_name_extension_date_and_size() {
        let now = now_unix_secs();
        let entry = SmartFolderEntry {
            source_id: uuid::Uuid::new_v4(),
            source_root: PathBuf::from(r"C:\Books"),
            source_order: 0,
            relative_parent: PathBuf::new(),
            path: PathBuf::from(r"C:\Books\Sample.cbz"),
            kind: SmartFolderEntryKind::Zip,
            mtime: now,
            file_size: 2 * 1024 * 1024,
        };
        let mut filter = crate::settings::SmartFolderFilter::default();
        filter.name_contains = "sample".into();
        filter
            .kinds
            .insert(crate::settings::SmartFolderContainerKind::Zip);
        filter.extensions.insert("cbz".into());
        filter.date_preset = Some(crate::settings::FacetDatePreset::Last7Days);
        filter.size_preset = Some(crate::settings::FacetSizePreset::MiB1To10);
        assert!(passes_cheap_filter(&entry, &filter, now));
        filter.extensions.clear();
        filter.extensions.insert("pdf".into());
        assert!(!passes_cheap_filter(&entry, &filter, now));
    }

    #[test]
    fn scan_recognizes_image_only_folder_and_supported_containers() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let image_book = root.join("images");
        let mixed = root.join("mixed");
        std::fs::create_dir_all(&image_book).unwrap();
        std::fs::create_dir_all(&mixed).unwrap();
        std::fs::write(image_book.join("001.jpg"), b"image").unwrap();
        std::fs::write(image_book.join("note.txt"), b"ignored").unwrap();
        std::fs::write(mixed.join("001.jpg"), b"image").unwrap();
        std::fs::write(mixed.join("movie.mp4"), b"video").unwrap();
        std::fs::write(root.join("book.zip"), b"zip").unwrap();
        std::fs::write(root.join("book.pdf"), b"pdf").unwrap();
        std::fs::write(root.join("book.7z"), b"archive").unwrap();

        let mut definition = crate::settings::SmartFolderDefinition::new("books");
        definition.sources.push(crate::settings::SmartFolderSource {
            id: uuid::Uuid::new_v4(),
            path: root,
            enabled: true,
        });
        let cancel = AtomicBool::new(false);
        let io_sem = crate::io_semaphore::GlobalIoSemaphore::new(1);
        let activity_gate = crate::activity_gate::ActivityGate::new(0);
        let (tx, _rx) = mpsc::channel();
        let result = scan_smart_folder(
            definition,
            false,
            true,
            &cancel,
            &io_sem,
            &activity_gate,
            &tx,
        )
        .unwrap();
        let names: HashSet<_> = result
            .snapshot
            .entries
            .iter()
            .filter_map(|entry| entry.path.file_name()?.to_str().map(str::to_owned))
            .collect();
        assert!(names.contains("images"));
        assert!(!names.contains("mixed"));
        assert!(names.contains("book.zip"));
        assert!(names.contains("book.pdf"));
        assert!(names.contains("book.7z"));
    }

    #[test]
    fn scan_keeps_readable_sources_when_another_source_is_missing() {
        let temp = tempfile::TempDir::new().unwrap();
        let readable = temp.path().join("readable");
        std::fs::create_dir_all(&readable).unwrap();
        std::fs::write(readable.join("book.zip"), b"zip").unwrap();
        let mut definition = crate::settings::SmartFolderDefinition::new("books");
        definition.sources = vec![
            crate::settings::SmartFolderSource {
                id: uuid::Uuid::new_v4(),
                path: temp.path().join("missing"),
                enabled: true,
            },
            crate::settings::SmartFolderSource {
                id: uuid::Uuid::new_v4(),
                path: readable,
                enabled: true,
            },
        ];
        let cancel = AtomicBool::new(false);
        let io_sem = crate::io_semaphore::GlobalIoSemaphore::new(1);
        let activity_gate = crate::activity_gate::ActivityGate::new(0);
        let (tx, _rx) = mpsc::channel();
        let result = scan_smart_folder(
            definition,
            false,
            true,
            &cancel,
            &io_sem,
            &activity_gate,
            &tx,
        )
        .unwrap();
        assert_eq!(result.snapshot.diag.source_failures, 1);
        assert_eq!(result.snapshot.entries.len(), 1);
    }

    #[test]
    fn scan_deduplicates_overlapping_sources_and_keeps_specific_membership() {
        let temp = tempfile::TempDir::new().unwrap();
        let child = temp.path().join("child");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join("001.jpg"), b"image").unwrap();
        let parent_id = uuid::Uuid::new_v4();
        let child_id = uuid::Uuid::new_v4();
        let mut definition = crate::settings::SmartFolderDefinition::new("books");
        definition.sources = vec![
            crate::settings::SmartFolderSource {
                id: parent_id,
                path: temp.path().to_path_buf(),
                enabled: true,
            },
            crate::settings::SmartFolderSource {
                id: child_id,
                path: child.clone(),
                enabled: true,
            },
        ];
        let cancel = AtomicBool::new(false);
        let io_sem = crate::io_semaphore::GlobalIoSemaphore::new(1);
        let activity_gate = crate::activity_gate::ActivityGate::new(0);
        let (tx, _rx) = mpsc::channel();
        let result = scan_smart_folder(
            definition,
            false,
            true,
            &cancel,
            &io_sem,
            &activity_gate,
            &tx,
        )
        .unwrap();
        assert_eq!(result.snapshot.entries.len(), 1);
        assert_eq!(result.snapshot.entries[0].path, child);
        assert_eq!(result.snapshot.entries[0].source_id, child_id);
    }

    #[test]
    fn metadata_filter_combines_rating_tag_and_untagged_conditions() {
        let key = "c:/books/sample.cbz".to_string();
        let ratings = HashMap::from([(key.clone(), 4)]);
        let tags = HashMap::from([(key.clone(), vec!["あとで読む".to_string()])]);
        let mut filter = crate::settings::SmartFolderFilter::default();
        filter.ratings = [false; 6];
        filter.ratings[4] = true;
        filter.tags.insert("あとで読む".into());
        assert!(metadata_filter_passes(&filter, &key, &ratings, &tags,));
        filter.tags.clear();
        filter.include_untagged = true;
        assert!(!metadata_filter_passes(&filter, &key, &ratings, &tags,));
    }

    #[test]
    fn folder_grouped_sort_prioritizes_source_and_relative_folder() {
        let mut definition = crate::settings::SmartFolderDefinition::new("books");
        definition.sort = crate::settings::SortOrder::FileName;
        definition.grouping = crate::settings::SubfolderExpansionOrder::FolderGrouped;
        let snapshot = SmartFolderSnapshot {
            definition,
            entries: Arc::new(vec![
                smart_entry(r"D:\SourceB\A\a.zip", 1, "A"),
                smart_entry(r"C:\SourceA\B\z.zip", 0, "B"),
                smart_entry(r"C:\SourceA\A\m.zip", 0, "A"),
            ]),
            diag: SmartFolderDiag::default(),
        };
        let cancel = AtomicBool::new(false);
        let (tx, _rx) = mpsc::channel();
        let prepared = prepare_smart_folder(snapshot, false, false, false, false, &cancel, &tx)
            .unwrap()
            .unwrap();
        let names: Vec<_> = prepared
            .items
            .iter()
            .filter_map(|item| match item {
                GridItem::Folder(path) | GridItem::ZipFile(path) | GridItem::PdfFile(path) => {
                    path.file_name()?.to_str()
                }
                GridItem::ConvertibleArchive { path, .. } => path.file_name()?.to_str(),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["m.zip", "z.zip", "a.zip"]);
    }

    #[test]
    fn synthetic_path_round_trips_definition_id() {
        let id = uuid::Uuid::new_v4();
        let path = smart_folder_synthetic_path(id);
        assert_eq!(smart_folder_id_from_synthetic_path(&path), Some(id));
        assert!(is_smart_folder_synthetic_path(&path));
    }
}
