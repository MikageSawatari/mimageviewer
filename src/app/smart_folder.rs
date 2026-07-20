//! スマートフォルダ: 現在の一覧条件から保存した複数ルールを OR 結合し、
//! 実フォルダ / 画像 / 動画 / 音声 / アーカイブを収集する flat snapshot view。

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
    Folder,
    Image,
    Video,
    Audio,
    Zip,
    Pdf,
    Archive,
}

impl SmartFolderEntryKind {
    fn setting_kind(self) -> crate::settings::FacetItemKind {
        match self {
            Self::Folder => crate::settings::FacetItemKind::Folder,
            Self::Image => crate::settings::FacetItemKind::Image,
            Self::Video => crate::settings::FacetItemKind::Video,
            Self::Audio => crate::settings::FacetItemKind::Audio,
            Self::Zip => crate::settings::FacetItemKind::Zip,
            Self::Pdf => crate::settings::FacetItemKind::Pdf,
            Self::Archive => crate::settings::FacetItemKind::Archive,
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
    /// 安価な条件を通過したルールの definition.rules 上の index。prepare の ★ / タグ /
    /// 編集状態条件はこの集合を OR 評価する。
    pub(crate) matching_rule_indices: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SmartFolderDiag {
    pub(crate) dirs_scanned: usize,
    pub(crate) containers_found: usize,
    pub(crate) source_failures: usize,
    pub(crate) source_failure_details: Vec<(PathBuf, String)>,
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
    /// 動画 path の正規化キー -> 同じ物理フォルダで一覧から除外した同名画像。
    pub(crate) video_thumb_overrides: HashMap<String, PathBuf>,
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
    /// Tombstones already present when this filesystem scan began. A successful fresh snapshot
    /// supersedes them; tombstones added later must still be applied to the scan result.
    tombstones_at_start: HashSet<String>,
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
    tombstones_at_start: HashSet<String>,
}

struct PreparedSmartFolder {
    snapshot: SmartFolderSnapshot,
    items: Vec<GridItem>,
    image_metas: Vec<Option<(i64, i64)>>,
    video_items: Vec<(usize, PathBuf, u64)>,
    metadata: super::subfolder_expansion::PreparedSubfolderMetadata,
    resort_metadata: Arc<ReusedSmartFolderMetadata>,
    /// Metadata generation captured before the worker opened its DB snapshots. Installation
    /// rejects this result if an edit/tag/rating write advanced the UI-side generation.
    metadata_revision: u64,
    refresh: bool,
    /// True only for a newly completed filesystem scan.  Its snapshot is authoritative and may
    /// retire the previous generation's delete tombstones after successful installation.
    authoritative_rescan: bool,
    authoritative_ignored_tombstones: HashSet<String>,
    applied_tombstones: HashSet<String>,
}

/// Path-keyed state retained from the currently installed smart-folder generation.  A sort-only
/// rebuild changes indices, not membership or metadata, so it can remap these values without
/// reopening every metadata database.
#[derive(Default)]
pub(crate) struct ReusedSmartFolderMetadata {
    /// Normalized keys are tied to `snapshot.entries` by index. Sort-only prepares reuse the Arc
    /// instead of allocating one String per entry again.
    normalized_keys: Arc<Vec<String>>,
    /// Membership in the same snapshot generation. Entry indices avoid cloning every included
    /// path and doing a HashSet lookup for each candidate during a sort-only prepare.
    included_entry_indices: Arc<Vec<usize>>,
    ratings_by_path: HashMap<String, u8>,
    tags_by_path: HashMap<String, Vec<String>>,
    local_adjust_paths: HashSet<String>,
    adjustment_by_path: HashMap<String, crate::adjustment::AdjustParams>,
    export_crop_by_path: HashMap<String, crate::export_crop::CropSettings>,
    view_trim_by_path: HashMap<String, crate::view_trim::ViewTrimPageOverride>,
    mask_paths: HashSet<String>,
    conceal_paths: HashSet<String>,
    comic_paths: HashSet<String>,
    folder_pin_map: HashMap<String, crate::folder_thumb_pins::FolderPinSource>,
    converted_archive_cache_paths: HashMap<String, PathBuf>,
}

enum SmartFolderPrepareEvent {
    Progress(SmartFolderProgress),
    Done(Box<PreparedSmartFolder>),
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
struct ActiveRule {
    id: uuid::Uuid,
    source: PathBuf,
    definition_order: usize,
    include_descendants: bool,
    filter: crate::settings::SmartFolderFilter,
    /// Candidate ごと・ルールごとの小文字化を避けるため、走査開始時に一度だけ作る。
    name_contains_lower: String,
}

#[derive(Clone)]
struct SmartFolderScanOptions {
    show_hidden_files: bool,
    include_convertible_archives: bool,
    skip_zip_if_folder_exists: bool,
    skip_archive_if_zip_exists: bool,
    skip_image_if_video_exists: bool,
    skip_duplicate_images: bool,
    video_thumb_use_sidecar_image: bool,
    image_ext_priority: Vec<String>,
}

impl From<&crate::settings::Settings> for SmartFolderScanOptions {
    fn from(settings: &crate::settings::Settings) -> Self {
        Self {
            show_hidden_files: settings.show_hidden_files,
            include_convertible_archives: !settings.archive_file_handling_ignores_convertible(),
            skip_zip_if_folder_exists: settings.skip_zip_if_folder_exists,
            skip_archive_if_zip_exists: settings.skip_archive_if_zip_exists,
            skip_image_if_video_exists: settings.skip_image_if_video_exists,
            skip_duplicate_images: settings.skip_duplicate_images,
            video_thumb_use_sidecar_image: settings.video_thumb_use_sidecar_image,
            image_ext_priority: settings.image_ext_priority.clone(),
        }
    }
}

struct SmartFolderCandidate {
    path: PathBuf,
    kind: SmartFolderEntryKind,
    mtime: i64,
    file_size: i64,
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

fn active_rules(definition: &crate::settings::SmartFolderDefinition) -> Vec<ActiveRule> {
    definition
        .rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| rule.enabled && !rule.source.as_os_str().is_empty())
        .map(|(definition_order, rule)| {
            let filter = rule.filter.clone();
            let name_contains_lower = filter.name_contains.to_lowercase();
            ActiveRule {
                id: rule.id,
                source: rule.source.clone(),
                definition_order,
                include_descendants: rule.include_descendants,
                filter,
                name_contains_lower,
            }
        })
        .collect()
}

fn unique_rule_roots(rules: &[ActiveRule]) -> Vec<PathBuf> {
    let mut roots: Vec<_> = rules.iter().map(|rule| rule.source.clone()).collect();
    roots.sort_by(|a, b| path_depth(b).cmp(&path_depth(a)).then_with(|| a.cmp(b)));
    let mut seen = HashSet::new();
    roots.retain(|root| seen.insert(crate::path_key::normalize_keep_drive(root)));
    roots
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

fn passes_cheap_filter_values(
    kind: SmartFolderEntryKind,
    path: &Path,
    name_lower: &str,
    mtime: i64,
    file_size: i64,
    filter: &crate::settings::SmartFolderFilter,
    name_contains_lower: &str,
    now: i64,
) -> bool {
    if !filter.kinds.is_empty() && !filter.kinds.contains(&kind.setting_kind()) {
        return false;
    }
    if !filter.name_contains.is_empty() && !name_lower.contains(name_contains_lower) {
        return false;
    }
    if !filter.extensions.is_empty() {
        let extension = if kind == SmartFolderEntryKind::Folder {
            String::new()
        } else {
            path.extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
        };
        if extension.is_empty() || !filter.extensions.contains(&extension) {
            return false;
        }
    }
    if let Some(preset) = filter.date_preset {
        if !preset.matches_mtime(mtime, now) {
            return false;
        }
    }
    if let Some(preset) = filter.size_preset {
        // The normal size facet treats an unavailable/zero size as unknown and excludes it.
        // A captured smart-folder rule must produce the same set rather than admitting folders
        // and metadata failures into the "under 1 MB" bucket.
        if file_size <= 0 {
            return false;
        }
        let (min, max) = preset.range_bytes();
        let size = file_size.max(0) as u64;
        if size < min || max.is_some_and(|max| size >= max) {
            return false;
        }
    }
    true
}

fn classify_entry_kind(
    extension: &str,
    include_convertible_archives: bool,
) -> Option<SmartFolderEntryKind> {
    if crate::folder_tree::is_recognized_image_ext(extension) {
        Some(SmartFolderEntryKind::Image)
    } else if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension) {
        Some(SmartFolderEntryKind::Video)
    } else if crate::folder_tree::is_audio_ext(extension) {
        Some(SmartFolderEntryKind::Audio)
    } else if crate::folder_tree::is_zip_extension(extension) {
        Some(SmartFolderEntryKind::Zip)
    } else if extension == "pdf" {
        Some(SmartFolderEntryKind::Pdf)
    } else if include_convertible_archives
        && crate::archive_converter::ArchiveFormat::from_extension(extension).is_some()
    {
        Some(SmartFolderEntryKind::Archive)
    } else {
        None
    }
}

fn rules_for_directory<'a>(rules: &'a [ActiveRule], dir: &Path) -> Vec<&'a ActiveRule> {
    rules
        .iter()
        .filter(|rule| {
            crate::folder_tree::path_eq(dir, &rule.source)
                || (rule.include_descendants
                    && crate::books::path_is_under_or_equal(dir, &rule.source))
        })
        .collect()
}

fn applicable_rules_need_name_lower(rules: &[&ActiveRule]) -> bool {
    rules
        .iter()
        .any(|rule| !rule.name_contains_lower.is_empty())
}

/// 通常一覧と同じ同名ファイル規則を、1 つの物理フォルダ分の候補へ適用する。
/// 条件適用より先に呼ぶことで、たとえば「画像だけ」のルールでも同名動画の sidecar
/// 画像を独立アイテムとして復活させない。フラット一覧全体では呼ばない。
fn normalize_smart_folder_candidates(
    candidates: &mut Vec<SmartFolderCandidate>,
    entry_file_names_ci: &HashSet<String>,
    options: &SmartFolderScanOptions,
) -> HashMap<String, PathBuf> {
    use super::folder_scan::ScanMediaKind;

    let mut media = candidates
        .iter()
        .filter_map(|candidate| {
            let kind = match candidate.kind {
                SmartFolderEntryKind::Image => ScanMediaKind::Image,
                SmartFolderEntryKind::Video => ScanMediaKind::Video,
                SmartFolderEntryKind::Audio => ScanMediaKind::Audio,
                _ => return None,
            };
            Some((
                candidate.path.clone(),
                kind,
                candidate.mtime,
                candidate.file_size,
            ))
        })
        .collect::<Vec<_>>();
    super::folder_scan::filter_upscaled_video_pairs_fast(&mut media, entry_file_names_ci);
    let mut video_thumb_overrides = HashMap::new();
    if options.skip_image_if_video_exists {
        for (video, image) in super::folder_scan::filter_video_image_duplicates(
            &mut media,
            options.video_thumb_use_sidecar_image,
        ) {
            video_thumb_overrides.insert(crate::path_key::normalize_keep_drive(&video), image);
        }
    }
    if options.skip_duplicate_images {
        super::folder_scan::filter_image_ext_duplicates(&mut media, &options.image_ext_priority);
    }

    let mut containers = Vec::new();
    let mut container_metas = Vec::new();
    for candidate in candidates.iter() {
        let item = match candidate.kind {
            SmartFolderEntryKind::Folder => Some(GridItem::Folder(candidate.path.clone())),
            SmartFolderEntryKind::Zip => Some(GridItem::ZipFile(candidate.path.clone())),
            SmartFolderEntryKind::Pdf => Some(GridItem::PdfFile(candidate.path.clone())),
            SmartFolderEntryKind::Archive => candidate
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .and_then(|extension| {
                    crate::archive_converter::ArchiveFormat::from_extension(&extension)
                })
                .map(|format| GridItem::ConvertibleArchive {
                    path: candidate.path.clone(),
                    format,
                }),
            _ => None,
        };
        if let Some(item) = item {
            containers.push(item);
            container_metas.push(Some((candidate.mtime, candidate.file_size)));
        }
    }
    if options.skip_zip_if_folder_exists {
        super::folder_scan::filter_virtual_folder_duplicates(&mut containers, &mut container_metas);
    }
    if options.skip_archive_if_zip_exists {
        super::folder_scan::filter_convertible_archive_duplicates(
            &mut containers,
            &mut container_metas,
        );
    }

    let keep_paths = media
        .iter()
        .map(|(path, _, _, _)| crate::path_key::normalize_keep_drive(path))
        .chain(containers.iter().filter_map(|item| {
            item.container_path()
                .map(crate::path_key::normalize_keep_drive)
        }))
        .collect::<HashSet<_>>();
    candidates.retain(|candidate| {
        keep_paths.contains(&crate::path_key::normalize_keep_drive(&candidate.path))
    });
    video_thumb_overrides
}

#[allow(clippy::too_many_arguments)]
fn scan_one_directory(
    rules: &[ActiveRule],
    dir: &Path,
    entries: std::fs::ReadDir,
    options: &SmartFolderScanOptions,
    cancel: &AtomicBool,
    result: &mut Vec<SmartFolderEntry>,
    video_thumb_overrides: &mut HashMap<String, PathBuf>,
    diag: &mut SmartFolderDiag,
) -> Vec<PathBuf> {
    let applicable_rules = rules_for_directory(rules, dir);
    if applicable_rules.is_empty() {
        return Vec::new();
    }
    let mut subdirs = Vec::new();
    let mut candidates = Vec::new();
    let mut entry_file_names_ci = HashSet::new();
    let now = now_unix_secs();

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
        entry_file_names_ci.insert(entry.file_name().to_string_lossy().to_lowercase());
        if crate::fs_entry::should_hide_fs_entry(&entry, options.show_hidden_files) {
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
        let kind = if entry_kind.is_directory() {
            if crate::video::upscale::paths::has_work_dir_suffix(&path) {
                continue;
            }
            if rules.iter().any(|rule| {
                rule.include_descendants
                    && crate::books::path_is_under_or_equal(&path, &rule.source)
            }) {
                subdirs.push(path.clone());
            }
            SmartFolderEntryKind::Folder
        } else {
            if !entry_kind.is_file() || crate::folder_tree::is_apple_double(&path) {
                continue;
            }
            let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
                continue;
            };
            let extension = extension.to_ascii_lowercase();
            let Some(kind) = classify_entry_kind(&extension, options.include_convertible_archives)
            else {
                continue;
            };
            kind
        };
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
        candidates.push(SmartFolderCandidate {
            path,
            kind,
            mtime,
            file_size,
        });
    }

    let before_normalize = candidates.len();
    let directory_video_overrides =
        normalize_smart_folder_candidates(&mut candidates, &entry_file_names_ci, options);
    diag.duplicates_removed += before_normalize.saturating_sub(candidates.len());
    let needs_name_lower = applicable_rules_need_name_lower(&applicable_rules);

    for candidate in candidates {
        let SmartFolderCandidate {
            path,
            kind,
            mtime,
            file_size,
        } = candidate;
        let name_lower = needs_name_lower.then(|| {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_lowercase()
        });
        let mut matching_rules = applicable_rules
            .iter()
            .copied()
            .filter(|rule| {
                passes_cheap_filter_values(
                    kind,
                    &path,
                    name_lower.as_deref().unwrap_or(""),
                    mtime,
                    file_size,
                    &rule.filter,
                    &rule.name_contains_lower,
                    now,
                )
            })
            .collect::<Vec<_>>();
        if matching_rules.is_empty() {
            continue;
        }
        matching_rules.sort_by(|a, b| {
            path_depth(&b.source)
                .cmp(&path_depth(&a.source))
                .then_with(|| a.definition_order.cmp(&b.definition_order))
        });
        let primary = matching_rules[0];
        if kind == SmartFolderEntryKind::Video {
            let video_key = crate::path_key::normalize_keep_drive(&path);
            if let Some(image) = directory_video_overrides.get(&video_key) {
                video_thumb_overrides.insert(video_key, image.clone());
            }
        }
        result.push(SmartFolderEntry {
            source_id: primary.id,
            source_root: primary.source.clone(),
            source_order: primary.definition_order,
            relative_parent: entry_relative_parent(&path, &primary.source),
            path,
            kind,
            mtime,
            file_size,
            matching_rule_indices: matching_rules
                .iter()
                .map(|rule| rule.definition_order)
                .collect(),
        });
    }
    subdirs
}

fn scan_smart_folder(
    definition: crate::settings::SmartFolderDefinition,
    options: SmartFolderScanOptions,
    cancel: &AtomicBool,
    io_sem: &crate::io_semaphore::GlobalIoSemaphore,
    activity_gate: &crate::activity_gate::ActivityGate,
    tx: &mpsc::Sender<SmartFolderScanEvent>,
) -> Option<SmartFolderScanResult> {
    let rules = active_rules(&definition);
    let roots = unique_rule_roots(&rules);
    let mut entries = Vec::new();
    let mut video_thumb_overrides = HashMap::new();
    let mut diag = SmartFolderDiag::default();
    let root_scanned = std::cell::RefCell::new(vec![false; roots.len()]);
    let containers_found = std::cell::Cell::new(0usize);
    let last_progress = std::cell::Cell::new(Instant::now());
    let walk_diag = super::recursive_snapshot_scan::walk_snapshot_roots(
        &roots,
        SMART_FOLDER_MAX_DEPTH,
        cancel,
        Some(io_sem),
        Some(activity_gate),
        |root_index, dir, read_dir, cancel| {
            if crate::folder_tree::path_eq(dir, &roots[root_index]) {
                root_scanned.borrow_mut()[root_index] = true;
            }
            let subdirs = scan_one_directory(
                &rules,
                dir,
                read_dir,
                &options,
                cancel,
                &mut entries,
                &mut video_thumb_overrides,
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
    let scanned_root_keys: HashSet<_> = root_scanned
        .borrow()
        .iter()
        .enumerate()
        .filter(|(_, scanned)| **scanned)
        .map(|(index, _)| crate::path_key::normalize_keep_drive(&roots[index]))
        .collect();
    diag.source_failure_details = rules
        .iter()
        .filter(|rule| {
            !scanned_root_keys.contains(&crate::path_key::normalize_keep_drive(&rule.source))
        })
        .map(|rule| {
            let detail = walk_diag
                .read_dir_failures
                .iter()
                .find(|(path, _)| crate::folder_tree::path_eq(path, &rule.source))
                .map(|(_, error)| error.clone())
                .unwrap_or_else(|| "検索元を読み込めませんでした".to_string());
            (rule.source.clone(), detail)
        })
        .collect();
    diag.source_failures = diag.source_failure_details.len();

    let before_dedupe = entries.len();
    let mut seen = HashSet::new();
    entries.retain(|entry| seen.insert(crate::path_key::normalize_keep_drive(&entry.path)));
    diag.duplicates_removed += before_dedupe.saturating_sub(entries.len());
    diag.containers_found = entries.len();
    Some(SmartFolderScanResult {
        snapshot: SmartFolderSnapshot {
            definition,
            entries: Arc::new(entries),
            video_thumb_overrides,
            diag,
        },
    })
}

#[allow(clippy::too_many_arguments)]
fn spawn_smart_folder_scan(
    definition: crate::settings::SmartFolderDefinition,
    generation: u64,
    refresh: bool,
    tombstones_at_start: HashSet<String>,
    options: SmartFolderScanOptions,
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
            let started = Instant::now();
            let event = match scan_smart_folder(
                definition,
                options,
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
            if crate::perf::is_enabled() {
                let (status, entries) = match &event {
                    SmartFolderScanEvent::Done(result) => ("done", result.snapshot.entries.len()),
                    SmartFolderScanEvent::Cancelled => ("cancelled", 0),
                    SmartFolderScanEvent::Progress(_) => ("progress", 0),
                };
                crate::perf::event(
                    "smart_folder",
                    "scan_end",
                    None,
                    generation,
                    &[
                        ("status", serde_json::Value::from(status)),
                        ("entries", serde_json::Value::from(entries)),
                        (
                            "ms",
                            serde_json::Value::from(started.elapsed().as_secs_f64() * 1000.0),
                        ),
                    ],
                );
            }
            let _ = tx.send(event);
        })
        .map_err(|error| format!("スマートフォルダ走査を開始できませんでした: {error}"))?;
    Ok(SmartFolderPending {
        definition_id,
        generation,
        refresh,
        tombstones_at_start,
        cancel,
        rx,
    })
}

fn metadata_filter_passes(
    filter: &crate::settings::SmartFolderFilter,
    entry: &SmartFolderEntry,
    key: &str,
    ratings: &HashMap<String, u8>,
    tags: &HashMap<String, Vec<String>>,
    edits: &SmartEditKeySets,
    converted_archive_paths: &HashMap<String, PathBuf>,
) -> bool {
    use crate::settings::{FacetEditFlag, FacetTagMode};

    let rating = ratings.get(key).copied().unwrap_or(0).min(5) as usize;
    if !filter.ratings[rating] {
        return false;
    }
    let item_tags = tags.get(key).map(Vec::as_slice).unwrap_or(&[]);
    // The normal tag facet keeps Folder rows navigable regardless of tag selection.  Capturing
    // the current facet into a smart-folder rule must preserve that pass-through behavior.
    if entry.kind != SmartFolderEntryKind::Folder
        && (!filter.tags.is_empty() || filter.include_untagged)
    {
        let normalized_tags = item_tags
            .iter()
            .map(|tag| crate::tags_db::normalize_tag_key(tag))
            .collect::<HashSet<_>>();
        let tag_match = match filter.tag_mode {
            FacetTagMode::Any => filter.tags.iter().any(|tag| normalized_tags.contains(tag)),
            FacetTagMode::All => filter.tags.iter().all(|tag| normalized_tags.contains(tag)),
        };
        if !tag_match && !(filter.include_untagged && item_tags.is_empty()) {
            return false;
        }
    }
    for flag in &filter.edits {
        let matched = match flag {
            FacetEditFlag::Adjustment | FacetEditFlag::AiAdjustment => edit_key_matches_for_entry(
                &edits.adjustment,
                entry,
                key,
                filter.edit_include_descendants,
                converted_archive_paths,
            ),
            FacetEditFlag::LocalAdjustment => edit_key_matches_for_entry(
                &edits.local_adjust,
                entry,
                key,
                filter.edit_include_descendants,
                converted_archive_paths,
            ),
            FacetEditFlag::Mask => edit_key_matches_for_entry(
                &edits.mask,
                entry,
                key,
                filter.edit_include_descendants,
                converted_archive_paths,
            ),
            FacetEditFlag::Conceal => edit_key_matches_for_entry(
                &edits.conceal,
                entry,
                key,
                filter.edit_include_descendants,
                converted_archive_paths,
            ),
            FacetEditFlag::Annotation => edit_key_matches_for_entry(
                &edits.annotation,
                entry,
                key,
                filter.edit_include_descendants,
                converted_archive_paths,
            ),
            FacetEditFlag::Rotation => edit_key_matches_for_entry(
                &edits.rotation,
                entry,
                key,
                filter.edit_include_descendants,
                converted_archive_paths,
            ),
            FacetEditFlag::Tagged => !item_tags.is_empty(),
            FacetEditFlag::Untagged => item_tags.is_empty(),
            FacetEditFlag::Rated => rating > 0,
            FacetEditFlag::Unrated => rating == 0,
        };
        if !matched {
            return false;
        }
    }
    true
}

fn edit_key_matches_for_entry(
    keys: &std::collections::BTreeSet<String>,
    entry: &SmartFolderEntry,
    key: &str,
    include_descendants: bool,
    converted_archive_paths: &HashMap<String, PathBuf>,
) -> bool {
    if edit_key_matches(keys, entry, key, include_descendants) {
        return true;
    }
    if entry.kind != SmartFolderEntryKind::Archive {
        return false;
    }
    converted_archive_paths
        .get(&crate::path_key::normalize_keep_drive(&entry.path))
        .is_some_and(|cache_path| {
            let cache_key = crate::adjustment_db::normalize_path(cache_path);
            edit_key_matches(keys, entry, &cache_key, include_descendants)
        })
}

#[derive(Default)]
struct SmartEditKeySets {
    adjustment: std::collections::BTreeSet<String>,
    local_adjust: std::collections::BTreeSet<String>,
    mask: std::collections::BTreeSet<String>,
    conceal: std::collections::BTreeSet<String>,
    annotation: std::collections::BTreeSet<String>,
    rotation: std::collections::BTreeSet<String>,
}

fn edit_key_matches(
    keys: &std::collections::BTreeSet<String>,
    entry: &SmartFolderEntry,
    key: &str,
    include_descendants: bool,
) -> bool {
    if keys.contains(key) {
        return true;
    }
    let separator = match entry.kind {
        SmartFolderEntryKind::Folder => "/",
        SmartFolderEntryKind::Zip | SmartFolderEntryKind::Pdf | SmartFolderEntryKind::Archive => {
            "::"
        }
        SmartFolderEntryKind::Image | SmartFolderEntryKind::Video | SmartFolderEntryKind::Audio => {
            return false;
        }
    };
    let prefix = format!("{}{}", key.trim_end_matches(['/', ':']), separator);
    for candidate in keys.range(prefix.clone()..) {
        if !candidate.starts_with(&prefix) {
            break;
        }
        if include_descendants {
            return true;
        }
        let rest = &candidate[prefix.len()..];
        if !rest.is_empty() && !rest.contains('/') {
            return true;
        }
    }
    false
}

fn load_edit_key_sets(
    wanted: &std::collections::BTreeSet<crate::settings::FacetEditFlag>,
    local_adjust: std::collections::BTreeSet<String>,
) -> Result<SmartEditKeySets, String> {
    use crate::settings::FacetEditFlag;
    let mut result = SmartEditKeySets {
        local_adjust,
        ..SmartEditKeySets::default()
    };
    if wanted.contains(&FacetEditFlag::Adjustment) || wanted.contains(&FacetEditFlag::AiAdjustment)
    {
        result.adjustment = crate::adjustment_db::AdjustmentDb::open()
            .map_err(|error| format!("補正 DB を読み込めませんでした: {error}"))?
            .load_page_param_keys();
    }
    if wanted.contains(&FacetEditFlag::Mask) {
        result.mask = crate::mask_db::MaskDb::open()
            .map_err(|error| format!("消しゴム DB を読み込めませんでした: {error}"))?
            .load_all_mask_keys();
    }
    if wanted.contains(&FacetEditFlag::Conceal) {
        result.conceal = crate::conceal_db::ConcealDb::open()
            .map_err(|error| format!("隠蔽加工 DB を読み込めませんでした: {error}"))?
            .load_all_conceal_keys();
    }
    if wanted.contains(&FacetEditFlag::Annotation) {
        result.annotation = crate::comic_db::ComicDb::open()
            .map_err(|error| format!("注釈 DB を読み込めませんでした: {error}"))?
            .load_all_comic_keys();
    }
    if wanted.contains(&FacetEditFlag::Rotation) {
        result.rotation = crate::rotation_db::RotationDb::open()
            .map_err(|error| format!("回転 DB を読み込めませんでした: {error}"))?
            .load_rotated_keys();
    }
    Ok(result)
}

struct SmartEntrySortKey {
    display_row: usize,
    name: crate::filename_sort::SortNameKey,
    relative_parent: Arc<crate::filename_sort::SortNameKey>,
}

fn build_smart_entry_sort_keys(
    entries: &[SmartFolderEntry],
    included: &[usize],
    sort: crate::settings::SortOrder,
    display_order: &crate::settings::GridDisplayOrder,
) -> Vec<SmartEntrySortKey> {
    let mut relative_parent_keys =
        HashMap::<PathBuf, Arc<crate::filename_sort::SortNameKey>>::new();
    included
        .iter()
        .map(|&entry_index| {
            let entry = &entries[entry_index];
            let name = entry
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            let relative_parent = relative_parent_keys
                .entry(entry.relative_parent.clone())
                .or_insert_with(|| {
                    Arc::new(crate::filename_sort::SortNameKey::file_name(
                        &entry.relative_parent.to_string_lossy(),
                    ))
                })
                .clone();
            SmartEntrySortKey {
                display_row: display_order.row_for(match entry.kind {
                    SmartFolderEntryKind::Folder => crate::settings::GridItemDisplayKind::Folder,
                    SmartFolderEntryKind::Zip
                    | SmartFolderEntryKind::Pdf
                    | SmartFolderEntryKind::Archive => {
                        crate::settings::GridItemDisplayKind::Archive
                    }
                    SmartFolderEntryKind::Image => crate::settings::GridItemDisplayKind::Image,
                    SmartFolderEntryKind::Video | SmartFolderEntryKind::Audio => {
                        crate::settings::GridItemDisplayKind::VideoAudio
                    }
                }),
                name: sort.name_key(name),
                relative_parent,
            }
        })
        .collect()
}

#[derive(Clone, Default)]
struct SmartFolderPrepareResources {
    prepare_catalog: bool,
    load_adjustments: bool,
    load_export_crops: bool,
    load_view_trims: bool,
    load_masks: bool,
    load_conceals: bool,
    load_comics: bool,
    load_video_pins: bool,
    folder_thumb_sort: crate::settings::SortOrder,
    folder_thumb_depth: u32,
    folder_pin_db: Option<Arc<crate::folder_thumb_pins::FolderThumbPinDb>>,
    archive_cache_db: Option<Arc<crate::archive_cache::ArchiveCacheDb>>,
    reused_catalog_db: Option<Arc<crate::catalog::CatalogDb>>,
    reused_catalog_entries:
        Option<Arc<std::sync::RwLock<HashMap<String, crate::catalog::CacheEntry>>>>,
}

fn prepared_converted_archive_path(
    path: &Path,
    mut mtime: i64,
    mut size: i64,
    db: &crate::archive_cache::ArchiveCacheDb,
) -> Option<PathBuf> {
    let mut source = path.to_path_buf();
    if crate::rar_loader::is_rar_path(path) {
        match crate::rar_loader::inspect_for_direct_read(path) {
            Ok(inspection)
                if inspection.decision == crate::rar_loader::RarDirectReadDecision::Direct =>
            {
                return Some(inspection.resolved_path);
            }
            Ok(inspection) => {
                source = inspection.resolved_path;
                if let Ok(metadata) = std::fs::metadata(&source) {
                    mtime = crate::ui_helpers::mtime_secs(&metadata);
                    size = metadata.len() as i64;
                }
            }
            Err(_) => {}
        }
    }
    db.peek(&source, mtime, size)
}

struct PreparedVideoFolderPinSeed {
    cache_key: String,
    video_path: PathBuf,
    mtime: i64,
    file_size: i64,
}

fn prepare_video_folder_pin_seeds(
    items: &[GridItem],
    folder_pin_map: &HashMap<String, crate::folder_thumb_pins::FolderPinSource>,
    resources: &SmartFolderPrepareResources,
    video_pin_db: Option<&crate::video_pins::VideoPinDb>,
    cancel: &AtomicBool,
) -> Vec<(PreparedVideoFolderPinSeed, Option<Vec<u8>>)> {
    if folder_pin_map.is_empty() {
        return Vec::new();
    }
    let mut resolved_seeds = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if index.is_multiple_of(METADATA_CHUNK_SIZE) && cancel.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let GridItem::Folder(container_path) = item else {
            continue;
        };
        let container_key = crate::path_key::normalize_keep_drive(container_path);
        let Some(source) = folder_pin_map.get(&container_key) else {
            continue;
        };
        let Some(resolved) = super::resolve_pin_target_cascaded(
            container_path,
            source,
            resources.folder_pin_db.as_deref(),
            resources.folder_thumb_depth as usize,
        ) else {
            continue;
        };
        if resolved.kind != crate::folder_thumb_pins::ResolvedKind::Video {
            continue;
        }
        let Some(base_key) = super::container_cache_base_key(
            item,
            true,
            Some(resources.folder_thumb_sort),
            resources.folder_thumb_depth,
        ) else {
            continue;
        };
        resolved_seeds.push(PreparedVideoFolderPinSeed {
            cache_key: format!(
                "{}{}{}",
                base_key,
                crate::thumb_loader::CACHE_KEY_PIN_SUFFIX,
                resolved.source_id
            ),
            video_path: resolved.abs_path,
            mtime: resolved.mtime,
            file_size: resolved.file_size,
        });
    }
    if resolved_seeds.is_empty() || cancel.load(Ordering::Relaxed) {
        return Vec::new();
    }
    let Some(db) = video_pin_db else {
        return Vec::new();
    };
    let webps = db.lookup_webps_many(resolved_seeds.iter().map(|seed| &seed.video_path));
    resolved_seeds
        .into_iter()
        .map(|seed| {
            let webp = webps
                .get(&seed.video_path)
                .filter(|bytes| !bytes.is_empty())
                .cloned();
            (seed, webp)
        })
        .collect()
}

fn prepare_smart_folder(
    snapshot: SmartFolderSnapshot,
    sort: crate::settings::SortOrder,
    display_order: crate::settings::GridDisplayOrder,
    refresh: bool,
    authoritative_rescan: bool,
    authoritative_ignored_tombstones: HashSet<String>,
    removed_paths: HashSet<String>,
    load_ratings: bool,
    load_tags: bool,
    load_local_adjust: bool,
    reused_metadata: Option<Arc<ReusedSmartFolderMetadata>>,
    resources: SmartFolderPrepareResources,
    cancel: &AtomicBool,
    tx: &mpsc::Sender<SmartFolderPrepareEvent>,
) -> Result<Option<PreparedSmartFolder>, String> {
    let reuse_metadata = reused_metadata.is_some();
    let reuse_resort_metadata_unchanged = reuse_metadata && removed_paths.is_empty();
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
    let keys: Arc<Vec<String>> = reused_metadata
        .as_ref()
        .map(|metadata| Arc::clone(&metadata.normalized_keys))
        .unwrap_or_else(|| {
            Arc::new(
                snapshot
                    .entries
                    .iter()
                    .map(|entry| crate::adjustment_db::normalize_path(&entry.path))
                    .collect(),
            )
        });

    let mut ratings = HashMap::new();
    if !reuse_metadata && load_ratings {
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
    if !reuse_metadata && load_tags {
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
    if !reuse_metadata && load_local_adjust {
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

    let wanted_edit_flags = snapshot
        .definition
        .rules
        .iter()
        .flat_map(|rule| rule.filter.edits.iter().copied())
        .collect::<std::collections::BTreeSet<_>>();
    let edit_keys = if reuse_metadata {
        SmartEditKeySets::default()
    } else {
        load_edit_key_sets(&wanted_edit_flags, local_adjust.iter().cloned().collect())?
    };
    let mut converted_archive_paths_for_filter = reused_metadata
        .as_ref()
        .map(|metadata| metadata.converted_archive_cache_paths.clone())
        .unwrap_or_default();
    if !reuse_metadata && let Some(db) = resources.archive_cache_db.as_deref() {
        for (index, entry) in snapshot.entries.iter().enumerate() {
            if index.is_multiple_of(METADATA_CHUNK_SIZE) && cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if entry.kind != SmartFolderEntryKind::Archive {
                continue;
            }
            if let Some(cache_path) =
                prepared_converted_archive_path(&entry.path, entry.mtime, entry.file_size, db)
            {
                converted_archive_paths_for_filter.insert(
                    crate::path_key::normalize_keep_drive(&entry.path),
                    cache_path,
                );
            }
        }
    }

    report(SmartFolderPhase::Filtering, 0);
    let mut included = Vec::with_capacity(
        reused_metadata
            .as_ref()
            .map(|metadata| metadata.included_entry_indices.len())
            .unwrap_or(total),
    );
    if let Some(metadata) = reused_metadata.as_ref() {
        for (position, &entry_index) in metadata.included_entry_indices.iter().enumerate() {
            if position.is_multiple_of(METADATA_CHUNK_SIZE) {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                report(SmartFolderPhase::Filtering, position);
            }
            let Some(entry) = snapshot.entries.get(entry_index) else {
                // Reuse is valid only for the same snapshot generation. Treat a mismatched cache
                // as unusable rather than indexing a different entry.
                return Err("スマートフォルダの再ソート用キャッシュが一致しません".into());
            };
            if !removed_paths.is_empty()
                && smart_folder_path_is_removed(
                    &crate::path_key::normalize_keep_drive(&entry.path),
                    &removed_paths,
                )
            {
                continue;
            }
            included.push(entry_index);
        }
    } else {
        for (index, key) in keys.iter().enumerate() {
            if index.is_multiple_of(METADATA_CHUNK_SIZE) {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                report(SmartFolderPhase::Filtering, index);
            }
            let entry = &snapshot.entries[index];
            if !removed_paths.is_empty()
                && smart_folder_path_is_removed(
                    &crate::path_key::normalize_keep_drive(&entry.path),
                    &removed_paths,
                )
            {
                continue;
            }
            let included_by_rule = entry.matching_rule_indices.iter().any(|rule_index| {
                snapshot
                    .definition
                    .rules
                    .get(*rule_index)
                    .is_some_and(|rule| {
                        metadata_filter_passes(
                            &rule.filter,
                            entry,
                            key,
                            &ratings,
                            &tags,
                            &edit_keys,
                            &converted_archive_paths_for_filter,
                        )
                    })
            });
            if included_by_rule {
                included.push(index);
            }
        }
    }

    // Aggregate views cannot hydrate per-item state from one folder prefix. Query only the
    // filtered exact keys on this worker; loading each entire metadata DB would duplicate all
    // user edit rows and inflate peak memory for million-item snapshots.
    // Sort-only rebuilds reuse every path-keyed metadata map below.  Building a second
    // `Vec<&str>` for all included rows is therefore pure overhead (about 32 MiB at two million
    // entries on 64-bit targets).  Keep the exact-key batch only for fresh DB reads.
    let included_keys_storage: Option<Vec<&str>> =
        (!reuse_metadata).then(|| included.iter().map(|&index| keys[index].as_str()).collect());
    let included_keys = included_keys_storage.as_deref().unwrap_or_default();
    let all_adjustments = if reuse_metadata {
        reused_metadata
            .as_ref()
            .map(|metadata| metadata.adjustment_by_path.clone())
            .unwrap_or_default()
    } else if resources.load_adjustments {
        crate::adjustment_db::AdjustmentDb::open()
            .map_err(|error| format!("補正 DB を読み込めませんでした: {error}"))?
            .load_page_params_many(&included_keys)
    } else {
        HashMap::new()
    };
    let all_export_crops = if reuse_metadata {
        reused_metadata
            .as_ref()
            .map(|metadata| metadata.export_crop_by_path.clone())
            .unwrap_or_default()
    } else if resources.load_export_crops {
        crate::export_crop::CropDb::open()
            .map_err(|error| format!("切り取り DB を読み込めませんでした: {error}"))?
            .load_many(&included_keys)
    } else {
        HashMap::new()
    };
    let all_view_trims = if reuse_metadata {
        reused_metadata
            .as_ref()
            .map(|metadata| metadata.view_trim_by_path.clone())
            .unwrap_or_default()
    } else if resources.load_view_trims {
        crate::view_trim_db::ViewTrimDb::open()
            .map_err(|error| format!("表示トリミング DB を読み込めませんでした: {error}"))?
            .load_page_overrides_many(&included_keys)
    } else {
        HashMap::new()
    };
    let all_masks = if reuse_metadata {
        reused_metadata
            .as_ref()
            .map(|metadata| metadata.mask_paths.clone())
            .unwrap_or_default()
    } else if resources.load_masks {
        crate::mask_db::MaskDb::open()
            .map_err(|error| format!("消しゴム DB を読み込めませんでした: {error}"))?
            .load_existing_mask_keys(&included_keys)
    } else {
        HashSet::new()
    };
    let all_conceals = if reuse_metadata {
        reused_metadata
            .as_ref()
            .map(|metadata| metadata.conceal_paths.clone())
            .unwrap_or_default()
    } else if resources.load_conceals {
        crate::conceal_db::ConcealDb::open()
            .map_err(|error| format!("隠蔽加工 DB を読み込めませんでした: {error}"))?
            .load_existing_conceal_keys(&included_keys)
    } else {
        HashSet::new()
    };
    let all_comics = if reuse_metadata {
        reused_metadata
            .as_ref()
            .map(|metadata| metadata.comic_paths.clone())
            .unwrap_or_default()
    } else if resources.load_comics {
        crate::comic_db::ComicDb::open()
            .map_err(|error| format!("注釈 DB を読み込めませんでした: {error}"))?
            .load_existing_comic_keys(&included_keys)
    } else {
        HashSet::new()
    };
    drop(included_keys_storage);

    report(SmartFolderPhase::Sorting, 0);
    let grouping = snapshot.definition.grouping;
    let display_order = display_order.normalized();
    let sort_keys = build_smart_entry_sort_keys(&snapshot.entries, &included, sort, &display_order);
    let sorted_positions = super::recursive_snapshot_scan::cancelable_sorted_indices(
        included.len(),
        cancel,
        |a_position, b_position| {
            let a_index = included[a_position];
            let b_index = included[b_position];
            let a = &snapshot.entries[a_index];
            let b = &snapshot.entries[b_index];
            let ak = &sort_keys[a_position];
            let bk = &sort_keys[b_position];
            let within = || {
                sort.compare_name_keys(&ak.name, a.mtime, &bk.name, b.mtime)
                    .then_with(|| a.path.cmp(&b.path))
            };
            ak.display_row
                .cmp(&bk.display_row)
                .then_with(|| match grouping {
                    crate::settings::SubfolderExpansionOrder::Flat => within()
                        .then_with(|| a.source_order.cmp(&b.source_order))
                        .then_with(|| ak.relative_parent.compare_file_name(&bk.relative_parent)),
                    crate::settings::SubfolderExpansionOrder::FolderGrouped => a
                        .source_order
                        .cmp(&b.source_order)
                        .then_with(|| ak.relative_parent.compare_file_name(&bk.relative_parent))
                        .then_with(within),
                })
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
    let mut adjustment_page_params = HashMap::new();
    let mut export_crop_page_settings = HashMap::new();
    let mut view_trim_page_overrides = HashMap::new();
    let mut mask_pages = HashSet::new();
    let mut conceal_pages = HashSet::new();
    let mut comic_pages = HashSet::new();
    let mut resort_ratings_by_path = HashMap::new();
    let mut resort_tags_by_path = HashMap::new();
    let mut resort_local_adjust_paths = HashSet::new();
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
            SmartFolderEntryKind::Folder => GridItem::Folder(entry.path.clone()),
            SmartFolderEntryKind::Image => GridItem::Image(entry.path.clone()),
            SmartFolderEntryKind::Video => GridItem::Video(entry.path.clone()),
            SmartFolderEntryKind::Audio => GridItem::Audio(entry.path.clone()),
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
        let rating = reused_metadata
            .as_ref()
            .and_then(|metadata| metadata.ratings_by_path.get(key))
            .or_else(|| ratings.get(key))
            .copied()
            .filter(|rating| *rating > 0);
        if let Some(rating) = rating {
            rating_cache.insert(display_index, rating);
            resort_ratings_by_path.insert(key.clone(), rating);
        }
        let item_tags = reused_metadata
            .as_ref()
            .and_then(|metadata| metadata.tags_by_path.get(key))
            .or_else(|| tags.get(key))
            .filter(|tags| !tags.is_empty());
        if let Some(item_tags) = item_tags {
            tags_cache.insert(key.clone(), item_tags.clone());
            resort_tags_by_path.insert(key.clone(), item_tags.clone());
        }
        if reused_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.local_adjust_paths.contains(key))
            || local_adjust.contains(key)
        {
            local_adjust_pages.insert(display_index);
            resort_local_adjust_paths.insert(key.clone());
        }
        if let Some(params) = all_adjustments.get(key) {
            adjustment_page_params.insert(display_index, params.clone());
        }
        if let Some(settings) = all_export_crops.get(key) {
            export_crop_page_settings.insert(display_index, *settings);
        }
        if let Some(page_override) = all_view_trims.get(key) {
            view_trim_page_overrides.insert(display_index, *page_override);
        }
        if all_masks.contains(key) {
            mask_pages.insert(display_index);
        }
        if all_conceals.contains(key) {
            conceal_pages.insert(display_index);
        }
        if all_comics.contains(key) {
            comic_pages.insert(display_index);
        }
    }
    let video_items = crate::filename_stack_ui::stack_video_items(&items, &image_metas);
    let folder_pin_map = if reuse_metadata {
        reused_metadata
            .as_ref()
            .map(|metadata| metadata.folder_pin_map.clone())
            .unwrap_or_default()
    } else {
        resources
            .folder_pin_db
            .as_ref()
            .map(|db| db.lookup_many(items.iter().filter_map(GridItem::container_path)))
            .unwrap_or_default()
    };
    let video_pin_db =
        if resources.load_video_pins && (!video_items.is_empty() || !folder_pin_map.is_empty()) {
            Some(
                crate::video_pins::VideoPinDb::open()
                    .map_err(|error| format!("動画ピン DB を読み込めませんでした: {error}"))?,
            )
        } else {
            None
        };
    let video_pin_blobs = video_pin_db
        .as_ref()
        .filter(|_| !video_items.is_empty())
        .map(|db| db.lookup_webps_many(video_items.iter().map(|(_, path, _)| path)))
        .unwrap_or_default();
    let video_folder_pin_seeds = prepare_video_folder_pin_seeds(
        &items,
        &folder_pin_map,
        &resources,
        video_pin_db.as_ref(),
        cancel,
    );
    let visible_archive_keys: HashSet<String> = items
        .iter()
        .filter_map(|item| match item {
            GridItem::ConvertibleArchive { path, .. } => {
                Some(crate::path_key::normalize_keep_drive(path))
            }
            _ => None,
        })
        .collect();
    let mut converted_archive_cache_paths = converted_archive_paths_for_filter;
    converted_archive_cache_paths.retain(|key, _| visible_archive_keys.contains(key));
    let resort_metadata = if reuse_resort_metadata_unchanged {
        Arc::clone(reused_metadata.as_ref().expect("checked above"))
    } else {
        Arc::new(ReusedSmartFolderMetadata {
            normalized_keys: Arc::clone(&keys),
            included_entry_indices: Arc::new(included.clone()),
            ratings_by_path: resort_ratings_by_path,
            tags_by_path: resort_tags_by_path,
            local_adjust_paths: resort_local_adjust_paths,
            adjustment_by_path: all_adjustments.clone(),
            export_crop_by_path: all_export_crops.clone(),
            view_trim_by_path: all_view_trims.clone(),
            mask_paths: all_masks.clone(),
            conceal_paths: all_conceals.clone(),
            comic_paths: all_comics.clone(),
            folder_pin_map: folder_pin_map.clone(),
            converted_archive_cache_paths: converted_archive_cache_paths.clone(),
        })
    };
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let catalog = if let (Some(db), Some(shared_entries)) = (
        resources.reused_catalog_db.as_ref(),
        resources.reused_catalog_entries.as_ref(),
    ) {
        Some(super::subfolder_expansion::PreparedAggregateCatalog {
            db: Arc::clone(db),
            entries: HashMap::new(),
            shared_entries: Some(Arc::clone(shared_entries)),
        })
    } else if !resources.prepare_catalog {
        None
    } else {
        match crate::catalog::CatalogDb::open(
            &crate::catalog::default_cache_dir(),
            &smart_folder_synthetic_path(snapshot.definition.id),
        ) {
            Ok(db) => {
                let db = Arc::new(db);
                let mut entries = db.load_all().unwrap_or_else(|error| {
                    crate::logger::log(format!(
                        "smart_folder: catalog load failed in prepare worker: {error}"
                    ));
                    HashMap::new()
                });
                let had_video_folder_pin_seeds = !video_folder_pin_seeds.is_empty();
                for (seed, webp) in video_folder_pin_seeds {
                    if cancel.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    if let Some(webp) = webp {
                        let unchanged = entries.get(&seed.cache_key).is_some_and(|entry| {
                            entry.mtime == seed.mtime
                                && entry.file_size == seed.file_size
                                && entry.jpeg_data == webp
                        });
                        if !unchanged
                            && !matches!(
                                db.save_thumb_bytes(
                                    &seed.cache_key,
                                    seed.mtime,
                                    seed.file_size,
                                    None,
                                    &webp,
                                ),
                                Ok(true)
                            )
                        {
                            let _ = db.delete_one(&seed.cache_key);
                        }
                    } else if entries.contains_key(&seed.cache_key) {
                        let _ = db.delete_one(&seed.cache_key);
                    }
                }
                if had_video_folder_pin_seeds {
                    entries = db.load_all().unwrap_or(entries);
                }
                Some(super::subfolder_expansion::PreparedAggregateCatalog {
                    db,
                    entries,
                    shared_entries: None,
                })
            }
            Err(error) => {
                crate::logger::log(format!(
                    "smart_folder: catalog open failed in prepare worker: {error}"
                ));
                None
            }
        }
    };
    Ok(Some(PreparedSmartFolder {
        snapshot,
        items,
        image_metas,
        video_items,
        metadata: super::subfolder_expansion::PreparedSubfolderMetadata {
            rating_cache,
            tags_cache,
            local_adjust_pages,
            video_pin_blobs,
            legacy_paths: Vec::new(),
            aggregate: Some(super::subfolder_expansion::PreparedAggregateMetadata {
                adjustment_page_params,
                export_crop_page_settings,
                view_trim_page_overrides,
                mask_pages,
                conceal_pages,
                comic_pages,
                folder_pin_map,
                converted_archive_cache_paths,
                catalog,
            }),
        },
        resort_metadata,
        metadata_revision: 0,
        refresh,
        authoritative_rescan,
        authoritative_ignored_tombstones,
        applied_tombstones: removed_paths,
    }))
}

fn spawn_smart_folder_prepare(
    snapshot: SmartFolderSnapshot,
    sort: crate::settings::SortOrder,
    display_order: crate::settings::GridDisplayOrder,
    generation: u64,
    metadata_revision: u64,
    refresh: bool,
    authoritative_rescan: bool,
    authoritative_ignored_tombstones: HashSet<String>,
    removed_paths: HashSet<String>,
    load_ratings: bool,
    load_tags: bool,
    load_local_adjust: bool,
    reused_metadata: Option<Arc<ReusedSmartFolderMetadata>>,
    resources: SmartFolderPrepareResources,
) -> Result<SmartFolderPreparePending, String> {
    let definition_id = snapshot.definition.id;
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("smart-folder-prepare".into())
        .spawn(move || {
            let started = Instant::now();
            let event = match prepare_smart_folder(
                snapshot,
                sort,
                display_order,
                refresh,
                authoritative_rescan,
                authoritative_ignored_tombstones,
                removed_paths,
                load_ratings,
                load_tags,
                load_local_adjust,
                reused_metadata,
                resources,
                &cancel_worker,
                &tx,
            ) {
                Ok(Some(mut prepared)) if !cancel_worker.load(Ordering::Relaxed) => {
                    prepared.metadata_revision = metadata_revision;
                    SmartFolderPrepareEvent::Done(Box::new(prepared))
                }
                Ok(_) => SmartFolderPrepareEvent::Cancelled,
                Err(message) => SmartFolderPrepareEvent::Error(message),
            };
            if crate::perf::is_enabled() {
                let (status, items) = match &event {
                    SmartFolderPrepareEvent::Done(prepared) => ("done", prepared.items.len()),
                    SmartFolderPrepareEvent::Cancelled => ("cancelled", 0),
                    SmartFolderPrepareEvent::Error(_) => ("error", 0),
                    SmartFolderPrepareEvent::Progress(_) => ("progress", 0),
                };
                crate::perf::event(
                    "smart_folder",
                    "prepare_end",
                    None,
                    generation,
                    &[
                        ("status", serde_json::Value::from(status)),
                        ("items", serde_json::Value::from(items)),
                        (
                            "ms",
                            serde_json::Value::from(started.elapsed().as_secs_f64() * 1000.0),
                        ),
                    ],
                );
            }
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

fn smart_folder_scan_rules_match(
    left: &crate::settings::SmartFolderDefinition,
    right: &crate::settings::SmartFolderDefinition,
) -> bool {
    left.id == right.id && left.rules == right.rules
}

fn adopt_smart_folder_presentation(
    target: &mut crate::settings::SmartFolderDefinition,
    current: &crate::settings::SmartFolderDefinition,
) {
    target.name.clone_from(&current.name);
    target.grouping = current.grouping;
}

fn remove_paths_from_smart_folder_snapshot(
    snapshot: &mut SmartFolderSnapshot,
    removed: &HashSet<String>,
) -> bool {
    let before = snapshot.entries.len();
    Arc::make_mut(&mut snapshot.entries).retain(|entry| {
        !smart_folder_path_is_removed(&crate::path_key::normalize_keep_drive(&entry.path), removed)
    });
    snapshot
        .video_thumb_overrides
        .retain(|video_key, image_path| {
            !smart_folder_path_is_removed(video_key, removed)
                && !smart_folder_path_is_removed(
                    &crate::path_key::normalize_keep_drive(image_path),
                    removed,
                )
        });
    let changed = snapshot.entries.len() != before;
    if changed {
        snapshot.diag.containers_found = snapshot.entries.len();
    }
    changed
}

fn smart_folder_path_is_removed(path_key: &str, removed: &HashSet<String>) -> bool {
    removed.iter().any(|root| {
        path_key == root
            || path_key
                .strip_prefix(root)
                .is_some_and(|rest| rest.starts_with('/') || rest.starts_with("::"))
    })
}

fn smart_folder_tombstones_after_scan_start(
    current: &HashSet<String>,
    at_scan_start: &HashSet<String>,
) -> HashSet<String> {
    current.difference(at_scan_start).cloned().collect()
}

/// prepare worker と共有していない snapshot なら tombstone を実体へ反映し、
/// その世代の削除記録を解放できる状態にする。
fn compact_smart_folder_tombstones_if_unique(
    snapshot: &mut SmartFolderSnapshot,
    tombstones: &mut HashSet<String>,
) -> bool {
    if tombstones.is_empty() || Arc::strong_count(&snapshot.entries) != 1 {
        return false;
    }
    remove_paths_from_smart_folder_snapshot(snapshot, tombstones);
    tombstones.clear();
    true
}

#[derive(Clone, Copy)]
pub(crate) enum SmartFolderMetadataDependency {
    Rating,
    Tags,
    Edits,
}

fn smart_folder_definition_uses_metadata(
    definition: &crate::settings::SmartFolderDefinition,
    dependency: SmartFolderMetadataDependency,
) -> bool {
    definition.rules.iter().any(|rule| match dependency {
        SmartFolderMetadataDependency::Rating => {
            rule.filter.ratings != [true; 6]
                || rule.filter.edits.iter().any(|flag| {
                    matches!(
                        flag,
                        crate::settings::FacetEditFlag::Rated
                            | crate::settings::FacetEditFlag::Unrated
                    )
                })
        }
        SmartFolderMetadataDependency::Tags => {
            !rule.filter.tags.is_empty()
                || rule.filter.include_untagged
                || rule.filter.edits.iter().any(|flag| {
                    matches!(
                        flag,
                        crate::settings::FacetEditFlag::Tagged
                            | crate::settings::FacetEditFlag::Untagged
                    )
                })
        }
        SmartFolderMetadataDependency::Edits => rule.filter.edits.iter().any(|flag| {
            matches!(
                flag,
                crate::settings::FacetEditFlag::Adjustment
                    | crate::settings::FacetEditFlag::AiAdjustment
                    | crate::settings::FacetEditFlag::LocalAdjustment
                    | crate::settings::FacetEditFlag::Mask
                    | crate::settings::FacetEditFlag::Conceal
                    | crate::settings::FacetEditFlag::Annotation
                    | crate::settings::FacetEditFlag::Rotation
            )
        }),
    })
}

impl App {
    /// Move the final `Arc` drops for million-item sort metadata away from the UI thread.
    ///
    /// The channel handoff is deliberate: if thread creation fails, the closure (and everything
    /// it captured) is destroyed by `Builder::spawn` on the caller.  Keeping the heavy values out
    /// of the closure until spawn succeeds prevents that failure path from recreating the stall.
    fn retire_smart_folder_resort_metadata(
        &mut self,
        values: impl IntoIterator<Item = Arc<ReusedSmartFolderMetadata>>,
    ) {
        let mut retired = std::mem::take(&mut self.smart_folder_retired_resort_metadata);
        retired.extend(values);
        if retired.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel::<Vec<Arc<ReusedSmartFolderMetadata>>>();
        match std::thread::Builder::new()
            .name("smart-folder-cache-drop".into())
            .spawn(move || {
                if let Ok(retired) = rx.recv() {
                    drop(retired);
                }
            }) {
            Ok(_thread) => {
                if let Err(error) = tx.send(retired) {
                    self.smart_folder_retired_resort_metadata = error.0;
                }
            }
            Err(error) => {
                crate::logger::log(format!(
                    "smart_folder: failed to spawn cache drop worker; defer until next boundary: {error}"
                ));
                self.smart_folder_retired_resort_metadata = retired;
            }
        }
    }

    /// Invalidate all installed sort metadata after a path-keyed metadata write. A cached smart
    /// snapshot may be opened later even when it is not the current view, so clearing only the
    /// current definition is insufficient.
    pub(crate) fn invalidate_smart_folder_resort_metadata(&mut self) {
        self.smart_folder_metadata_revision = self.smart_folder_metadata_revision.wrapping_add(1);
        let retired = std::mem::take(&mut self.smart_folder_resort_metadata)
            .into_values()
            .collect::<Vec<_>>();
        self.retire_smart_folder_resort_metadata(retired);
    }

    /// A smart-folder scan/prepare owns the top-level grid.  Search modes and Snapshot Lock own
    /// the same surface, so they must be retired before a smart-folder generation is started.
    ///
    /// Search close is intentionally allowed to restore its saved origin first.  That restored
    /// real/synthetic location becomes the smart folder's history/return origin; the later smart
    /// install remains the only operation that replaces the grid.
    fn close_transient_views_before_smart_folder(&mut self) -> super::ViewReturnContext {
        let mut return_context = super::ViewReturnContext {
            path: self.folder_nav_current_location(),
            subfolder_restore: None,
        };
        if self.is_snapshot_active() {
            if let Some(snapshot_origin) = self.dismiss_snapshot_without_restore() {
                return_context = snapshot_origin;
            }
        }
        if self.favsearch.active {
            return_context = self.dismiss_favsearch_without_restore();
        }
        if self.global_search.active {
            return_context = self.dismiss_global_search_without_restore();
        }
        if self.tag_view.active {
            return_context = self.dismiss_tag_view_without_restore();
        }
        if self.show_search_bar {
            self.show_search_bar = false;
            self.search_query.clear();
            self.search_filter = None;
            self.search_filter_origin_folder = None;
            self.search_has_focus = false;
            self.search_tag_bridge.clear();
            self.cancel_search_pending();
            self.rebuild_visible_indices();
        }
        self.cancel_pending_folder_nav();
        return_context
    }

    fn smart_folder_has_conflicting_top_level_view(&self) -> bool {
        self.is_snapshot_active()
            // Ctrl+F is an in-view filter once a smart grid is installed. Other search modes
            // replace the grid, while Ctrl+F keeps it and is re-applied after a prepare.
            || (self.show_search_bar
                && !self.items_are_smart_folder_view
                && self.smart_folder_local_search_reapply.is_none())
            || self.favsearch.active
            || self.global_search.active
            || self.tag_view.active
    }

    /// Search entry points call this before they start their own asynchronous work.  The poll-side
    /// invariant below is the final guard, but cancelling here avoids wasting I/O until it notices.
    pub(crate) fn take_smart_folder_origin_for_search_entry(
        &mut self,
    ) -> Option<super::ViewReturnContext> {
        let origin = self.smart_folder_open_origin.take();
        self.cancel_smart_folder_pending();
        origin
    }

    pub(crate) fn schedule_current_smart_folder_metadata_refresh(
        &mut self,
        dependency: SmartFolderMetadataDependency,
    ) {
        // Even when this metadata kind is not part of the definition's filter, the prepared grid
        // owns badges/adjustments/crops/pins that a later sort would otherwise roll back.
        self.invalidate_smart_folder_resort_metadata();
        // `current_smart_folder_id` belongs to the main aggregate view even while a detached
        // fullscreen context is temporarily mounted. Remember the invalidation globally and
        // apply it after the main context is restored instead of losing detached edits.
        let relevant = self
            .current_smart_folder_id
            .and_then(|id| {
                self.settings
                    .smart_folders
                    .iter()
                    .find(|definition| definition.id == id)
            })
            .is_some_and(|definition| {
                smart_folder_definition_uses_metadata(definition, dependency)
            });
        if relevant {
            self.smart_folder_metadata_refresh_due =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(250));
        }
    }

    pub(crate) fn poll_smart_folder_metadata_refresh(&mut self, ctx: &egui::Context) {
        let Some(due) = self.smart_folder_metadata_refresh_due else {
            return;
        };
        if !self.items_are_smart_folder_view {
            self.smart_folder_metadata_refresh_due = None;
            return;
        }
        let now = std::time::Instant::now();
        if now < due {
            ctx.request_repaint_after(due.saturating_duration_since(now));
            return;
        }
        if self.smart_folder_pending.is_some()
            || self.smart_folder_prepare_pending.is_some()
            || self.smart_folder_confirm_pending.is_some()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        self.smart_folder_metadata_refresh_due = None;
        let Some(id) = self.current_smart_folder_id else {
            return;
        };
        if let Some(snapshot) = self.smart_folder_snapshots.get(&id).cloned() {
            self.start_smart_folder_prepare(snapshot, false);
        }
    }

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
        if active_rules(&definition).is_empty() {
            self.show_feedback_toast("表示条件を追加してください".into());
            self.open_smart_folder_manager(Some(definition_id));
            return;
        }
        let open_origin = self.close_transient_views_before_smart_folder();
        self.cancel_smart_folder_pending();
        self.smart_folder_open_origin = Some(open_origin);
        self.smart_folder_generation = self.smart_folder_generation.wrapping_add(1);
        let generation = self.smart_folder_generation;
        if !self.items_are_smart_folder_view {
            self.smart_folder_saved_folder = self
                .effective_folder()
                .filter(|path| !is_synthetic_view_path(path));
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
        let tombstones_at_start = self
            .smart_folder_removed_paths
            .get(&definition_id)
            .cloned()
            .unwrap_or_default();
        match spawn_smart_folder_scan(
            definition.clone(),
            generation,
            refresh,
            tombstones_at_start,
            SmartFolderScanOptions::from(&self.settings),
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
            Err(message) => {
                self.show_feedback_toast(message);
                self.restore_pending_smart_folder_origin();
            }
        }
    }

    pub(crate) fn start_smart_folder_prepare(
        &mut self,
        snapshot: SmartFolderSnapshot,
        refresh: bool,
    ) {
        self.start_smart_folder_prepare_inner(snapshot, refresh, false, HashSet::new(), None);
    }

    /// Start preparing a snapshot that has just been produced by a successful filesystem scan.
    /// Unlike a resort/re-filter of a cached snapshot, this generation is authoritative: paths
    /// recreated after an earlier delete may reappear, but old tombstones are retired only after
    /// the prepared snapshot is actually accepted and installed.
    fn start_smart_folder_prepare_after_scan(
        &mut self,
        snapshot: SmartFolderSnapshot,
        refresh: bool,
        tombstones_at_start: HashSet<String>,
    ) {
        self.start_smart_folder_prepare_inner(snapshot, refresh, true, tombstones_at_start, None);
    }

    fn start_smart_folder_prepare_inner(
        &mut self,
        snapshot: SmartFolderSnapshot,
        refresh: bool,
        authoritative_rescan: bool,
        authoritative_ignored_tombstones: HashSet<String>,
        reused_metadata: Option<Arc<ReusedSmartFolderMetadata>>,
    ) {
        if self.items_are_smart_folder_view && self.show_search_bar {
            self.smart_folder_local_search_reapply = Some(self.search_query.clone());
            self.cancel_search_pending();
        }
        if let Some(pending) = self.smart_folder_prepare_pending.take() {
            pending.cancel();
        }
        let is_sort_only = reused_metadata.is_some();
        let reused_catalog_db = is_sort_only
            .then(|| self.current_color_catalog.clone())
            .flatten();
        let reused_catalog_entries = is_sort_only
            .then(|| self.current_color_cache_map.clone())
            .flatten();
        let generation = self.smart_folder_generation;
        let definition_id = snapshot.definition.id;
        let current_tombstones = self
            .smart_folder_removed_paths
            .get(&definition_id)
            .cloned()
            .unwrap_or_default();
        let applied_tombstones = if authoritative_rescan {
            smart_folder_tombstones_after_scan_start(
                &current_tombstones,
                &authoritative_ignored_tombstones,
            )
        } else {
            current_tombstones
        };
        match spawn_smart_folder_prepare(
            snapshot,
            self.settings.sort_order,
            self.settings.grid_display_order.clone(),
            generation,
            self.smart_folder_metadata_revision,
            refresh,
            authoritative_rescan,
            authoritative_ignored_tombstones,
            applied_tombstones,
            self.rating_db.is_some(),
            self.tags_db.is_some(),
            self.local_adjust_db.is_some(),
            reused_metadata,
            SmartFolderPrepareResources {
                prepare_catalog: !is_sort_only,
                load_adjustments: self.adjustment_db.is_some(),
                load_export_crops: self.export_crop_db.is_some(),
                load_view_trims: self.view_trim_db.is_some(),
                load_masks: self.mask_db.is_some(),
                load_conceals: self.conceal_db.is_some(),
                load_comics: self.comic_db.is_some(),
                // Existing generation catalog entries already contain completed video/pinned
                // thumbnails.  Sort-only rebuilds reuse that catalog and need no video-pin DB.
                load_video_pins: !is_sort_only && self.video_pin_db.is_some(),
                folder_thumb_sort: self.settings.folder_thumb_sort,
                folder_thumb_depth: self.settings.folder_thumb_depth,
                folder_pin_db: self.folder_thumb_pin_db.clone(),
                archive_cache_db: self.archive_cache_db.clone(),
                reused_catalog_db,
                reused_catalog_entries,
            },
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
        let scan_pending = self.smart_folder_pending.is_some();
        let prepare_pending = self.smart_folder_prepare_pending.is_some();
        if let Some(pending) = self.smart_folder_pending.take() {
            pending.cancel();
        }
        if let Some(pending) = self.smart_folder_prepare_pending.take() {
            pending.cancel();
        }
        self.smart_folder_confirm_pending = None;
        self.smart_folder_progress = None;
        self.smart_folder_open_origin = None;
        self.smart_folder_local_search_reapply = None;
        self.suppress_nav_record_for_search_restore = false;
        self.set_active_folder_nav_suppress_record_once(false);
        if crate::perf::is_enabled() && (scan_pending || prepare_pending) {
            crate::perf::event(
                "smart_folder",
                "cancel",
                None,
                self.smart_folder_generation,
                &[
                    ("scan_pending", serde_json::Value::from(scan_pending)),
                    ("prepare_pending", serde_json::Value::from(prepare_pending)),
                ],
            );
        }
    }

    fn cancel_smart_folder_pending_and_restore_origin(&mut self) {
        let origin = self.smart_folder_open_origin.take();
        self.cancel_smart_folder_pending();
        if let Some(origin) = origin {
            self.restore_view_return_context(origin);
        }
    }

    fn restore_pending_smart_folder_origin(&mut self) {
        if let Some(origin) = self.smart_folder_open_origin.take() {
            self.restore_view_return_context(origin);
        }
    }

    /// 定義変更時に古い snapshot / 進行中 worker を無効化する。
    fn invalidate_smart_folder_definition_state(&mut self, definition_id: uuid::Uuid) {
        self.smart_folder_snapshots.remove(&definition_id);
        let retired = self.smart_folder_resort_metadata.remove(&definition_id);
        self.retire_smart_folder_resort_metadata(retired);
        self.smart_folder_removed_paths.remove(&definition_id);
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

    pub(crate) fn invalidate_smart_folder_definition_without_reopen(
        &mut self,
        definition_id: uuid::Uuid,
    ) {
        self.invalidate_smart_folder_definition_state(definition_id);
    }

    pub(crate) fn invalidate_smart_folder_definition(&mut self, definition_id: uuid::Uuid) {
        let reopen_current = self.current_smart_folder_id == Some(definition_id)
            && self
                .settings
                .smart_folders
                .iter()
                .any(|definition| definition.id == definition_id);
        self.invalidate_smart_folder_definition_state(definition_id);
        if reopen_current {
            self.open_smart_folder(definition_id, true);
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
        if let Some(mut snapshot) = self
            .smart_folder_snapshots
            .get(&definition_id)
            .filter(|snapshot| smart_folder_scan_rules_match(&snapshot.definition, &definition))
            .cloned()
        {
            adopt_smart_folder_presentation(&mut snapshot.definition, &definition);
            self.current_smart_folder_id = Some(definition_id);
            self.start_smart_folder_prepare(snapshot, false);
        } else {
            self.open_smart_folder(definition_id, false);
        }
        true
    }

    pub(crate) fn poll_smart_folder(&mut self, ctx: &egui::Context) {
        // Search/Snapshot may be entered while a scan is running (for example from a keyboard
        // shortcut before the modal is painted).  Definition id + generation alone cannot prove
        // that this worker still owns the top-level grid, so reject the whole generation here.
        if self.smart_folder_has_conflicting_top_level_view() {
            self.cancel_smart_folder_pending();
            return;
        }
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
                Ok(SmartFolderScanEvent::Done(mut result)) => {
                    let Some(pending) = self.smart_folder_pending.take() else {
                        break;
                    };
                    let current_definition =
                        self.settings.smart_folders.iter().find(|definition| {
                            definition.id == pending.definition_id
                                && smart_folder_scan_rules_match(
                                    definition,
                                    &result.snapshot.definition,
                                )
                        });
                    if pending.generation != self.smart_folder_generation
                        || current_definition.is_none()
                    {
                        self.smart_folder_progress = None;
                        break;
                    }
                    adopt_smart_folder_presentation(
                        &mut result.snapshot.definition,
                        current_definition.expect("checked above"),
                    );
                    if result.snapshot.diag.source_failures
                        == active_rules(&result.snapshot.definition).len()
                    {
                        self.smart_folder_progress = None;
                        self.show_feedback_toast(
                            "スマートフォルダの検索元を1件も読み込めませんでした".into(),
                        );
                        self.restore_pending_smart_folder_origin();
                    } else if result.snapshot.entries.len() >= SMART_FOLDER_CONFIRM_THRESHOLD {
                        self.smart_folder_confirm_pending = Some(SmartFolderConfirmPending {
                            snapshot: result.snapshot,
                            generation: pending.generation,
                            refresh: pending.refresh,
                            tombstones_at_start: pending.tombstones_at_start,
                        });
                    } else {
                        self.start_smart_folder_prepare_after_scan(
                            result.snapshot,
                            pending.refresh,
                            pending.tombstones_at_start,
                        );
                    }
                    break;
                }
                Ok(SmartFolderScanEvent::Cancelled) => {
                    self.smart_folder_pending = None;
                    self.smart_folder_progress = None;
                    self.restore_pending_smart_folder_origin();
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.smart_folder_pending = None;
                    self.smart_folder_progress = None;
                    self.show_feedback_toast("スマートフォルダ走査が中断されました".into());
                    self.restore_pending_smart_folder_origin();
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
                    let prepared = *prepared;
                    let Some(pending) = self.smart_folder_prepare_pending.take() else {
                        break;
                    };
                    let current_definition = self
                        .settings
                        .smart_folders
                        .iter()
                        .find(|definition| {
                            definition.id == pending.definition_id
                                && smart_folder_scan_rules_match(
                                    definition,
                                    &prepared.snapshot.definition,
                                )
                        })
                        .cloned();
                    if pending.generation == self.smart_folder_generation
                        && pending.definition_id == prepared.snapshot.definition.id
                        && let Some(current_definition) = current_definition
                    {
                        if prepared.snapshot.definition.grouping != current_definition.grouping {
                            let mut snapshot = prepared.snapshot;
                            adopt_smart_folder_presentation(
                                &mut snapshot.definition,
                                &current_definition,
                            );
                            self.start_smart_folder_prepare_inner(
                                snapshot,
                                prepared.refresh,
                                prepared.authoritative_rescan,
                                prepared.authoritative_ignored_tombstones,
                                None,
                            );
                        } else {
                            let mut prepared = prepared;
                            adopt_smart_folder_presentation(
                                &mut prepared.snapshot.definition,
                                &current_definition,
                            );
                            if self.install_prepared_smart_folder(prepared) {
                                self.reapply_local_search_after_smart_folder_prepare(ctx);
                            }
                        }
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
                    self.restore_pending_smart_folder_origin();
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.smart_folder_prepare_pending = None;
                    self.smart_folder_progress = None;
                    self.show_feedback_toast("スマートフォルダの表示準備が中断されました".into());
                    self.restore_pending_smart_folder_origin();
                    break;
                }
            }
        }
    }

    fn install_prepared_smart_folder(&mut self, prepared: PreparedSmartFolder) -> bool {
        let install_started = Instant::now();
        if self.smart_folder_local_search_reapply.is_some() && self.show_search_bar {
            // The user can keep typing while scan/prepare is active. Capture the last UI-owned
            // value before `start_loading_subfolder_items` clears index-based search state.
            self.smart_folder_local_search_reapply = Some(self.search_query.clone());
        }
        let PreparedSmartFolder {
            mut snapshot,
            items,
            image_metas,
            video_items,
            metadata,
            resort_metadata,
            metadata_revision,
            refresh,
            authoritative_rescan,
            authoritative_ignored_tombstones,
            applied_tombstones,
        } = prepared;
        if metadata_revision != self.smart_folder_metadata_revision {
            // A metadata write raced this worker. Rebuild from the same filesystem snapshot with
            // fresh DB reads; never install a completed but stale metadata generation.
            self.start_smart_folder_prepare_inner(
                snapshot,
                refresh,
                authoritative_rescan,
                authoritative_ignored_tombstones,
                None,
            );
            return false;
        }
        let definition_id = snapshot.definition.id;
        if authoritative_rescan {
            let current_tombstones = self
                .smart_folder_removed_paths
                .get(&definition_id)
                .cloned()
                .unwrap_or_default();
            let current_after_scan_start = smart_folder_tombstones_after_scan_start(
                &current_tombstones,
                &authoritative_ignored_tombstones,
            );
            if !current_after_scan_start.is_subset(&applied_tombstones) {
                // A delete landed after this prepare worker captured its tombstones. Re-run the
                // cheap metadata/sort/build phase against the already scanned snapshot rather
                // than installing a row that was removed concurrently.
                self.start_smart_folder_prepare_inner(
                    snapshot,
                    refresh,
                    true,
                    authoritative_ignored_tombstones,
                    None,
                );
                return false;
            }
        }
        let open_origin = self.smart_folder_open_origin.take();
        // worker 起動元の旧 snapshot を先に手放し、共有が解けた完成 snapshot へ
        // その世代の tombstone を実体化してから cache へ戻す。
        self.smart_folder_snapshots.remove(&definition_id);
        let tombstones_compacted = if authoritative_rescan {
            false
        } else {
            self.smart_folder_removed_paths
                .get_mut(&definition_id)
                .is_some_and(|tombstones| {
                    compact_smart_folder_tombstones_if_unique(&mut snapshot, tombstones)
                })
        };
        if authoritative_rescan || tombstones_compacted {
            self.smart_folder_removed_paths.remove(&definition_id);
        }
        let definition_name = snapshot.definition.name.clone();
        let diag = snapshot.diag.clone();
        let item_count = items.len();
        let synthetic_path = smart_folder_synthetic_path(definition_id);
        // Opening becomes a history transition only now that scan+prepare succeeded.  A failed
        // or cancelled scan therefore never creates a dead entry in the Back stack.
        if let Some(origin) = open_origin.as_ref().and_then(|origin| origin.path.clone()) {
            self.record_folder_nav_transition_from(&synthetic_path, origin);
        } else {
            self.record_folder_nav_transition(&synthetic_path);
        }
        if let Some(restore) = open_origin.and_then(|origin| origin.subfolder_restore) {
            self.folder_nav_subfolder_restore = Some(restore);
        } else if self.items_are_subfolder_expansion_view {
            let subfolder_path = super::subfolder_expansion_synthetic_path();
            self.folder_nav_subfolder_restore =
                self.take_subfolder_expansion_restore_for_synthetic_path(Some(&subfolder_path));
        }
        // A smart folder definition already owns all of its filtering. Facet / star filters
        // from the source view must not be applied a second time; suppress them for the
        // synthetic scope and let the normal scope-exit path restore them on return.
        // A -> B direct switching must transfer the existing suppression before the common
        // loader rebuilds visible indices.  Restoring A and suppressing B afterwards briefly
        // reapplies A's saved filters to B's items and leaves a stale visible set.
        if let Some(top) = self.facet_filter_suppression_stack.last_mut() {
            top.anchor = synthetic_path.clone();
        } else {
            self.suppress_current_facet_filter_at(
                synthetic_path.clone(),
                "元の一覧の絞り込みを退避しました (戻ると復元)".to_string(),
            );
        }
        if let Some((anchor, _)) = self.rating_filter_suppressed_at.as_mut() {
            *anchor = synthetic_path.clone();
        } else if self.rating_filter_active() {
            self.rating_filter_suppressed_at =
                Some((synthetic_path.clone(), self.settings.rating_filter));
        }
        self.video_thumb_overrides.clear();
        self.video_thumb_overrides
            .extend(snapshot.video_thumb_overrides.clone());
        self.start_loading_subfolder_items(
            synthetic_path,
            items,
            image_metas,
            video_items,
            metadata,
        );
        self.items_are_subfolder_expansion_view = false;
        self.items_are_smart_folder_view = true;
        self.current_smart_folder_id = Some(definition_id);
        self.smart_folder_snapshots.insert(definition_id, snapshot);
        let retired = self
            .smart_folder_resort_metadata
            .insert(definition_id, resort_metadata);
        self.retire_smart_folder_resort_metadata(retired);
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
        for (path, error) in &diag.source_failure_details {
            crate::logger::log(format!(
                "smart_folder: source failed path={} error={error}",
                path.display()
            ));
        }
        if skipped > 0 {
            let source_detail = diag
                .source_failure_details
                .first()
                .map(|(path, error)| format!(" / {}: {error}", path.display()))
                .unwrap_or_default();
            self.show_feedback_toast(format!(
                "スマートフォルダを{prefix}: {item_count}件 (読めなかった項目 {skipped}件){source_detail}"
            ));
        } else {
            self.show_feedback_toast(format!("スマートフォルダを{prefix}: {item_count}件"));
        }
        if crate::perf::is_enabled() {
            crate::perf::event(
                "smart_folder",
                "install_end",
                None,
                self.smart_folder_generation,
                &[
                    ("items", serde_json::Value::from(item_count)),
                    ("skipped", serde_json::Value::from(skipped)),
                    (
                        "ms",
                        serde_json::Value::from(install_started.elapsed().as_secs_f64() * 1000.0),
                    ),
                ],
            );
        }
        true
    }

    fn reapply_local_search_after_smart_folder_prepare(&mut self, ctx: &egui::Context) {
        let Some(query) = self.smart_folder_local_search_reapply.take() else {
            return;
        };
        self.show_search_bar = true;
        self.search_query = query;
        self.search_filter = None;
        self.search_filter_origin_folder = self.effective_folder();
        if self.search_query.trim().is_empty() {
            self.rebuild_visible_indices();
        } else {
            self.execute_search(ctx);
        }
    }

    pub(crate) fn reprepare_current_smart_folder_for_sort(&mut self) -> bool {
        if !self.items_are_smart_folder_view {
            return false;
        }
        let Some(id) = self.current_smart_folder_id else {
            return true;
        };
        // 通常一覧と同じグローバルなソート順を使う。スマートフォルダ定義へは
        // 書き戻さず、保存済み snapshot を現在の設定で prepare し直すだけにする。
        if let Some(snapshot) = self.smart_folder_snapshots.get(&id).cloned() {
            let reused_metadata = self.smart_folder_resort_metadata.get(&id).cloned();
            self.start_smart_folder_prepare_inner(
                snapshot,
                false,
                false,
                HashSet::new(),
                reused_metadata,
            );
        }
        true
    }

    /// 名前は snapshot identity ではなく表示情報、grouping は走査後の prepare 情報。
    /// rules が同じなら実フォルダ走査を捨てずに更新する。
    fn apply_smart_folder_presentation(
        &mut self,
        definition: &crate::settings::SmartFolderDefinition,
    ) -> bool {
        let grouping_changed = self
            .smart_folder_snapshots
            .get(&definition.id)
            .is_some_and(|snapshot| snapshot.definition.grouping != definition.grouping);
        if let Some(snapshot) = self.smart_folder_snapshots.get_mut(&definition.id) {
            adopt_smart_folder_presentation(&mut snapshot.definition, definition);
        }
        if self.current_smart_folder_id == Some(definition.id) {
            self.address = format!("スマートフォルダ: {}", definition.name);
        }
        grouping_changed
    }

    pub(crate) fn update_smart_folder_presentation_without_reprepare(
        &mut self,
        definition: crate::settings::SmartFolderDefinition,
    ) {
        self.apply_smart_folder_presentation(&definition);
    }

    pub(crate) fn update_smart_folder_presentation(
        &mut self,
        definition: crate::settings::SmartFolderDefinition,
    ) {
        let grouping_changed = self.apply_smart_folder_presentation(&definition);
        if self.current_smart_folder_id == Some(definition.id)
            && grouping_changed
            && let Some(snapshot) = self.smart_folder_snapshots.get(&definition.id).cloned()
        {
            self.start_smart_folder_prepare(snapshot, false);
        }
    }

    /// 削除済み実パスを全 snapshot から除外する。prepare worker が Arc を共有中なら
    /// UI スレッドで巨大 Vec を複製せず tombstone を渡し、worker を再起動する。
    pub(crate) fn remove_paths_from_smart_folder_snapshots(&mut self, paths: &[PathBuf]) {
        if paths.is_empty() || self.smart_folder_snapshots.is_empty() {
            return;
        }
        let removed: HashSet<String> = paths
            .iter()
            .map(|path| crate::path_key::normalize_keep_drive(path))
            .collect();
        if removed.is_empty() {
            return;
        }

        // A filesystem scan does not share the old snapshot Arc, so strong_count alone cannot
        // prove that a tombstone is safe to retire. Keep it while any result for the same
        // definition can still arrive and reintroduce the deleted path.
        let mut in_flight_ids = HashSet::new();
        if let Some(pending) = self.smart_folder_pending.as_ref() {
            in_flight_ids.insert(pending.definition_id);
        }
        if let Some(pending) = self.smart_folder_prepare_pending.as_ref() {
            in_flight_ids.insert(pending.definition_id);
        }
        if let Some(pending) = self.smart_folder_confirm_pending.as_ref() {
            in_flight_ids.insert(pending.snapshot.definition.id);
        }

        let mut affected_ids = Vec::new();
        let mut compacted_ids = Vec::new();
        for (id, snapshot) in &mut self.smart_folder_snapshots {
            if snapshot.entries.iter().any(|entry| {
                smart_folder_path_is_removed(
                    &crate::path_key::normalize_keep_drive(&entry.path),
                    &removed,
                )
            }) {
                affected_ids.push(*id);
                let tombstones = self.smart_folder_removed_paths.entry(*id).or_default();
                tombstones.extend(removed.iter().cloned());
                if !in_flight_ids.contains(id)
                    && compact_smart_folder_tombstones_if_unique(snapshot, tombstones)
                {
                    compacted_ids.push(*id);
                }
            }
        }
        for id in compacted_ids {
            self.smart_folder_removed_paths.remove(&id);
        }
        if let Some(confirm) = self.smart_folder_confirm_pending.as_mut()
            && affected_ids.contains(&confirm.snapshot.definition.id)
        {
            remove_paths_from_smart_folder_snapshot(&mut confirm.snapshot, &removed);
        }
        let pending_affected = self
            .smart_folder_prepare_pending
            .as_ref()
            .is_some_and(|pending| affected_ids.contains(&pending.definition_id));
        if pending_affected {
            if let Some(pending) = self.smart_folder_prepare_pending.take() {
                pending.cancel();
            }
            if let Some(id) = self.current_smart_folder_id
                && affected_ids.contains(&id)
                && let Some(snapshot) = self.smart_folder_snapshots.get(&id).cloned()
            {
                self.start_smart_folder_prepare(snapshot, false);
            }
        }
    }

    /// リネーム移行完了後は全定義の snapshot を失効させる。旧 path の即時除外は
    /// tombstone で行い、新 path はメタデータ移行完了後の authoritative scan でだけ採用する。
    pub(crate) fn refresh_smart_folders_after_rename(&mut self) {
        let reopen = self.current_smart_folder_id;
        self.cancel_smart_folder_pending();
        self.smart_folder_snapshots.clear();
        let retired = std::mem::take(&mut self.smart_folder_resort_metadata)
            .into_values()
            .collect::<Vec<_>>();
        self.retire_smart_folder_resort_metadata(retired);
        self.smart_folder_removed_paths.clear();
        if let Some(id) = reopen {
            self.open_smart_folder(id, true);
        }
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
                        self.start_smart_folder_prepare_after_scan(
                            confirm.snapshot,
                            confirm.refresh,
                            confirm.tombstones_at_start,
                        );
                    }
                }
            } else if cancel {
                self.smart_folder_confirm_pending = None;
                self.smart_folder_progress = None;
                self.restore_pending_smart_folder_origin();
            }
            return;
        }
        let Some(progress) = self.smart_folder_progress.clone() else {
            return;
        };
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("smart_folder_progress_modal")).show(ctx, |ui| {
            // サブ展開と同じ幅・構成にして、走査中は背面の一覧を操作できないことを
            // 見た目でも明示する。処理本体と cancel の所有境界は変更しない。
            ui.set_min_width(460.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.heading(if progress.phase == SmartFolderPhase::Scanning {
                    "スマートフォルダを走査中..."
                } else {
                    "スマートフォルダの表示を準備中..."
                });
            });
            ui.add_space(6.0);
            if progress.phase == SmartFolderPhase::Scanning {
                ui.label(format!("対象項目: {} 件", progress.containers_found));
                ui.label(format!("確認済みフォルダ: {} 件", progress.dirs_scanned));
                if let Some(current_dir) = progress.current_dir.as_ref() {
                    let full_path = current_dir.to_string_lossy().into_owned();
                    ui.label("現在のフォルダ:").on_hover_text(&full_path);
                    ui.add(
                        egui::Label::new(
                            current_dir
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or(&full_path),
                        )
                        .truncate(),
                    )
                    .on_hover_text(full_path);
                }
            } else {
                ui.add(
                    egui::Label::new(format!(
                        "{}: {} / {} 件",
                        progress.phase.label(),
                        progress.completed,
                        progress.total
                    ))
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
                if progress.total > 0 {
                    ui.add(
                        egui::ProgressBar::new(progress.completed as f32 / progress.total as f32)
                            .show_percentage(),
                    );
                }
            }
            ui.add_space(8.0);
            if ui.button("中止").clicked() {
                cancel = true;
            }
        });
        if cancel {
            self.cancel_smart_folder_pending_and_restore_origin();
            self.show_feedback_toast("スマートフォルダ処理を中止しました".into());
        }
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(
        id: uuid::Uuid,
        source: PathBuf,
        enabled: bool,
        include_descendants: bool,
        filter: crate::settings::SmartFolderFilter,
    ) -> crate::settings::SmartFolderRule {
        crate::settings::SmartFolderRule {
            id,
            source,
            enabled,
            include_descendants,
            filter,
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
            matching_rule_indices: vec![0],
        }
    }

    fn unfiltered_scan_options() -> SmartFolderScanOptions {
        SmartFolderScanOptions {
            show_hidden_files: false,
            include_convertible_archives: true,
            skip_zip_if_folder_exists: false,
            skip_archive_if_zip_exists: false,
            skip_image_if_video_exists: false,
            skip_duplicate_images: false,
            video_thumb_use_sidecar_image: true,
            image_ext_priority: Vec::new(),
        }
    }

    #[test]
    fn metadata_refresh_dependency_matches_only_saved_filter_inputs() {
        let mut definition = crate::settings::SmartFolderDefinition::new("books");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            PathBuf::from(r"C:\Books"),
            true,
            true,
            Default::default(),
        ));

        assert!(!smart_folder_definition_uses_metadata(
            &definition,
            SmartFolderMetadataDependency::Rating
        ));
        assert!(!smart_folder_definition_uses_metadata(
            &definition,
            SmartFolderMetadataDependency::Tags
        ));
        assert!(!smart_folder_definition_uses_metadata(
            &definition,
            SmartFolderMetadataDependency::Edits
        ));

        definition.rules[0].filter.ratings[0] = false;
        assert!(smart_folder_definition_uses_metadata(
            &definition,
            SmartFolderMetadataDependency::Rating
        ));
        definition.rules[0].filter.ratings = [true; 6];

        definition.rules[0]
            .filter
            .tags
            .insert("favorite".to_owned());
        assert!(smart_folder_definition_uses_metadata(
            &definition,
            SmartFolderMetadataDependency::Tags
        ));
        definition.rules[0].filter.tags.clear();

        definition.rules[0]
            .filter
            .edits
            .insert(crate::settings::FacetEditFlag::Adjustment);
        assert!(smart_folder_definition_uses_metadata(
            &definition,
            SmartFolderMetadataDependency::Edits
        ));
    }

    fn run_test_scan(
        definition: crate::settings::SmartFolderDefinition,
        options: SmartFolderScanOptions,
    ) -> SmartFolderScanResult {
        let cancel = AtomicBool::new(false);
        let io_sem = crate::io_semaphore::GlobalIoSemaphore::new(1);
        let activity_gate = crate::activity_gate::ActivityGate::new(0);
        let (tx, _rx) = mpsc::channel();
        scan_smart_folder(definition, options, &cancel, &io_sem, &activity_gate, &tx).unwrap()
    }

    #[test]
    fn unique_rule_roots_put_specific_roots_first_and_ignore_disabled_rules() {
        let mut definition = crate::settings::SmartFolderDefinition::new("books");
        definition.rules = vec![
            rule(
                uuid::Uuid::new_v4(),
                PathBuf::from(r"C:\Books"),
                true,
                true,
                Default::default(),
            ),
            rule(
                uuid::Uuid::new_v4(),
                PathBuf::from(r"C:\Books\Done"),
                true,
                true,
                Default::default(),
            ),
            rule(
                uuid::Uuid::new_v4(),
                PathBuf::from(r"D:\Download"),
                false,
                true,
                Default::default(),
            ),
        ];
        let roots = unique_rule_roots(&active_rules(&definition));
        assert_eq!(
            roots,
            [PathBuf::from(r"C:\Books\Done"), PathBuf::from(r"C:\Books")]
        );
    }

    #[test]
    fn candidate_names_are_lowercased_only_when_an_applicable_rule_uses_name_filter() {
        let root = PathBuf::from(r"C:\Books");
        let mut definition = crate::settings::SmartFolderDefinition::new("books");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            root.clone(),
            true,
            true,
            Default::default(),
        ));
        let active = active_rules(&definition);
        assert!(!applicable_rules_need_name_lower(&rules_for_directory(
            &active, &root
        )));

        definition.rules[0].filter.name_contains = "Sample".into();
        let active = active_rules(&definition);
        assert!(applicable_rules_need_name_lower(&rules_for_directory(
            &active, &root
        )));
    }

    #[test]
    fn cheap_filter_matches_kind_name_extension_date_and_size() {
        let now = now_unix_secs();
        let path = PathBuf::from(r"C:\Books\Sample.cbz");
        let mut filter = crate::settings::SmartFolderFilter::default();
        filter.name_contains = "sample".into();
        filter.kinds.insert(crate::settings::FacetItemKind::Zip);
        filter.extensions.insert("cbz".into());
        filter.date_preset = Some(crate::settings::FacetDatePreset::Last7Days);
        filter.size_preset = Some(crate::settings::FacetSizePreset::MiB1To10);
        assert!(passes_cheap_filter_values(
            SmartFolderEntryKind::Zip,
            &path,
            "sample.cbz",
            now,
            2 * 1024 * 1024,
            &filter,
            "sample",
            now,
        ));
        filter.extensions.clear();
        filter.extensions.insert("pdf".into());
        assert!(!passes_cheap_filter_values(
            SmartFolderEntryKind::Zip,
            &path,
            "sample.cbz",
            now,
            2 * 1024 * 1024,
            &filter,
            "sample",
            now,
        ));
    }

    #[test]
    fn cheap_size_filter_rejects_unknown_zero_size() {
        let mut filter = crate::settings::SmartFolderFilter {
            size_preset: Some(crate::settings::FacetSizePreset::Under1MiB),
            ..Default::default()
        };
        assert!(!passes_cheap_filter_values(
            SmartFolderEntryKind::Folder,
            Path::new(r"C:\Books\unknown"),
            "unknown",
            now_unix_secs(),
            0,
            &filter,
            "",
            now_unix_secs(),
        ));
        filter.size_preset = None;
        assert!(passes_cheap_filter_values(
            SmartFolderEntryKind::Folder,
            Path::new(r"C:\Books\unknown"),
            "unknown",
            now_unix_secs(),
            0,
            &filter,
            "",
            now_unix_secs(),
        ));
    }

    #[test]
    fn prepare_sort_keys_cover_only_included_entries_and_share_parent_keys() {
        let entries = vec![
            smart_entry(r"C:\Books\series\a.jpg", 0, "series"),
            smart_entry(r"C:\Books\excluded.jpg", 0, ""),
            smart_entry(r"C:\Books\series\b.jpg", 0, "series"),
        ];
        let keys = build_smart_entry_sort_keys(
            &entries,
            &[0, 2],
            crate::settings::SortOrder::FileName,
            &crate::settings::GridDisplayOrder::default(),
        );

        assert_eq!(
            keys.len(),
            2,
            "excluded snapshot rows must not own sort keys"
        );
        assert!(Arc::ptr_eq(
            &keys[0].relative_parent,
            &keys[1].relative_parent
        ));
    }

    #[test]
    fn scan_collects_folders_images_videos_audio_and_supported_containers() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let child = root.join("child");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(root.join("cover.jpg"), b"image").unwrap();
        std::fs::write(root.join("movie.mp4"), b"video").unwrap();
        std::fs::write(root.join("track.mp3"), b"audio").unwrap();
        std::fs::write(root.join("book.zip"), b"zip").unwrap();
        std::fs::write(root.join("book.pdf"), b"pdf").unwrap();
        std::fs::write(root.join("book.7z"), b"archive").unwrap();
        std::fs::write(root.join("note.txt"), b"ignored").unwrap();
        std::fs::write(child.join("inside.webp"), b"image").unwrap();

        let mut definition = crate::settings::SmartFolderDefinition::new("books");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            root,
            true,
            true,
            Default::default(),
        ));
        let cancel = AtomicBool::new(false);
        let io_sem = crate::io_semaphore::GlobalIoSemaphore::new(1);
        let activity_gate = crate::activity_gate::ActivityGate::new(0);
        let (tx, _rx) = mpsc::channel();
        let result = scan_smart_folder(
            definition,
            unfiltered_scan_options(),
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
        assert!(names.contains("child"));
        assert!(names.contains("cover.jpg"));
        assert!(names.contains("movie.mp4"));
        assert!(names.contains("track.mp3"));
        assert!(names.contains("book.zip"));
        assert!(names.contains("book.pdf"));
        assert!(names.contains("book.7z"));
        assert!(names.contains("inside.webp"));
        assert!(!names.contains("note.txt"));
    }

    #[test]
    fn scan_hides_same_folder_video_sidecar_and_keeps_it_as_thumbnail_override() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let video = root.join("movie.mp4");
        let sidecar = root.join("movie.jpg");
        std::fs::write(&video, b"video").unwrap();
        std::fs::write(&sidecar, b"image").unwrap();

        let mut definition = crate::settings::SmartFolderDefinition::new("videos");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            root,
            true,
            false,
            Default::default(),
        ));
        let mut options = unfiltered_scan_options();
        options.skip_image_if_video_exists = true;
        let result = run_test_scan(definition, options);

        assert!(
            result
                .snapshot
                .entries
                .iter()
                .any(|entry| crate::folder_tree::path_eq(&entry.path, &video))
        );
        assert!(
            !result
                .snapshot
                .entries
                .iter()
                .any(|entry| crate::folder_tree::path_eq(&entry.path, &sidecar))
        );
        assert_eq!(
            result
                .snapshot
                .video_thumb_overrides
                .get(&crate::path_key::normalize_keep_drive(&video)),
            Some(&sidecar)
        );
    }

    #[test]
    fn scan_does_not_merge_same_stem_media_from_different_physical_folders() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let video_dir = root.join("videos");
        let image_dir = root.join("images");
        std::fs::create_dir_all(&video_dir).unwrap();
        std::fs::create_dir_all(&image_dir).unwrap();
        let video = video_dir.join("same.mp4");
        let image = image_dir.join("same.jpg");
        std::fs::write(&video, b"video").unwrap();
        std::fs::write(&image, b"image").unwrap();

        let mut definition = crate::settings::SmartFolderDefinition::new("media");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            root,
            true,
            true,
            Default::default(),
        ));
        let mut options = unfiltered_scan_options();
        options.skip_image_if_video_exists = true;
        let result = run_test_scan(definition, options);

        assert!(
            result
                .snapshot
                .entries
                .iter()
                .any(|entry| crate::folder_tree::path_eq(&entry.path, &video))
        );
        assert!(
            result
                .snapshot
                .entries
                .iter()
                .any(|entry| crate::folder_tree::path_eq(&entry.path, &image))
        );
        assert!(result.snapshot.video_thumb_overrides.is_empty());
    }

    #[test]
    fn scan_applies_same_name_normalization_before_saved_kind_filter() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("movie.mp4"), b"video").unwrap();
        std::fs::write(root.join("movie.jpg"), b"image").unwrap();

        let mut filter = crate::settings::SmartFolderFilter::default();
        filter.kinds.insert(crate::settings::FacetItemKind::Image);
        let mut definition = crate::settings::SmartFolderDefinition::new("images");
        definition
            .rules
            .push(rule(uuid::Uuid::new_v4(), root, true, false, filter));
        let mut options = unfiltered_scan_options();
        options.skip_image_if_video_exists = true;
        let result = run_test_scan(definition, options);

        assert!(result.snapshot.entries.is_empty());
        assert!(result.snapshot.video_thumb_overrides.is_empty());
    }

    #[test]
    fn scan_keeps_video_and_same_name_image_when_duplicate_setting_is_disabled() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("movie.mp4"), b"video").unwrap();
        std::fs::write(root.join("movie.jpg"), b"image").unwrap();

        let mut definition = crate::settings::SmartFolderDefinition::new("media");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            root,
            true,
            false,
            Default::default(),
        ));
        let result = run_test_scan(definition, unfiltered_scan_options());

        assert_eq!(result.snapshot.entries.len(), 2);
        assert!(result.snapshot.video_thumb_overrides.is_empty());
    }

    #[test]
    fn scan_applies_container_and_image_duplicate_settings_per_directory() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir_all(root.join("book.v1")).unwrap();
        std::fs::write(root.join("book.v1.zip"), b"zip").unwrap();
        std::fs::write(root.join("book.v1.pdf"), b"pdf").unwrap();
        std::fs::write(root.join("book.v1.7z"), b"archive").unwrap();
        std::fs::write(root.join("native.zip"), b"zip").unwrap();
        std::fs::write(root.join("native.rar"), b"archive").unwrap();
        std::fs::write(root.join("cover.jpg"), b"jpg").unwrap();
        std::fs::write(root.join("cover.png"), b"png").unwrap();

        let mut definition = crate::settings::SmartFolderDefinition::new("books");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            root,
            true,
            false,
            Default::default(),
        ));
        let mut options = unfiltered_scan_options();
        options.skip_zip_if_folder_exists = true;
        options.skip_archive_if_zip_exists = true;
        options.skip_duplicate_images = true;
        options.image_ext_priority = vec!["jpg".into(), "png".into()];
        let result = run_test_scan(definition, options);
        let names = result
            .snapshot
            .entries
            .iter()
            .filter_map(|entry| entry.path.file_name()?.to_str())
            .collect::<HashSet<_>>();

        assert!(names.contains("book.v1"));
        assert!(!names.contains("book.v1.zip"));
        assert!(!names.contains("book.v1.pdf"));
        assert!(!names.contains("book.v1.7z"));
        assert!(names.contains("native.zip"));
        assert!(!names.contains("native.rar"));
        assert!(names.contains("cover.jpg"));
        assert!(!names.contains("cover.png"));
    }

    #[test]
    fn non_recursive_rule_keeps_direct_items_but_does_not_scan_children() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let child = root.join("child");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(root.join("direct.mp4"), b"video").unwrap();
        std::fs::write(child.join("nested.mp4"), b"video").unwrap();
        let mut definition = crate::settings::SmartFolderDefinition::new("videos");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            root,
            true,
            false,
            Default::default(),
        ));
        let cancel = AtomicBool::new(false);
        let io_sem = crate::io_semaphore::GlobalIoSemaphore::new(1);
        let activity_gate = crate::activity_gate::ActivityGate::new(0);
        let (tx, _rx) = mpsc::channel();
        let result = scan_smart_folder(
            definition,
            unfiltered_scan_options(),
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
            .filter_map(|entry| entry.path.file_name()?.to_str())
            .collect();
        assert!(names.contains("child"));
        assert!(names.contains("direct.mp4"));
        assert!(!names.contains("nested.mp4"));
    }

    #[test]
    fn scan_keeps_readable_sources_when_another_source_is_missing() {
        let temp = tempfile::TempDir::new().unwrap();
        let readable = temp.path().join("readable");
        std::fs::create_dir_all(&readable).unwrap();
        std::fs::write(readable.join("book.zip"), b"zip").unwrap();
        let mut definition = crate::settings::SmartFolderDefinition::new("books");
        definition.rules = vec![
            rule(
                uuid::Uuid::new_v4(),
                temp.path().join("missing"),
                true,
                true,
                Default::default(),
            ),
            rule(
                uuid::Uuid::new_v4(),
                readable,
                true,
                true,
                Default::default(),
            ),
        ];
        let cancel = AtomicBool::new(false);
        let io_sem = crate::io_semaphore::GlobalIoSemaphore::new(1);
        let activity_gate = crate::activity_gate::ActivityGate::new(0);
        let (tx, _rx) = mpsc::channel();
        let result = scan_smart_folder(
            definition,
            unfiltered_scan_options(),
            &cancel,
            &io_sem,
            &activity_gate,
            &tx,
        )
        .unwrap();
        assert_eq!(result.snapshot.diag.source_failures, 1);
        assert_eq!(result.snapshot.diag.source_failure_details.len(), 1);
        assert!(
            result.snapshot.diag.source_failure_details[0]
                .0
                .ends_with("missing")
        );
        assert!(!result.snapshot.diag.source_failure_details[0].1.is_empty());
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
        definition.rules = vec![
            rule(
                parent_id,
                temp.path().to_path_buf(),
                true,
                true,
                Default::default(),
            ),
            rule(child_id, child.clone(), true, true, Default::default()),
        ];
        let cancel = AtomicBool::new(false);
        let io_sem = crate::io_semaphore::GlobalIoSemaphore::new(1);
        let activity_gate = crate::activity_gate::ActivityGate::new(0);
        let (tx, _rx) = mpsc::channel();
        let result = scan_smart_folder(
            definition,
            unfiltered_scan_options(),
            &cancel,
            &io_sem,
            &activity_gate,
            &tx,
        )
        .unwrap();
        let image = result
            .snapshot
            .entries
            .iter()
            .find(|entry| entry.path.ends_with("001.jpg"))
            .unwrap();
        assert_eq!(image.source_id, child_id);
        assert_eq!(image.matching_rule_indices, [1, 0]);
        assert_eq!(
            result
                .snapshot
                .entries
                .iter()
                .filter(|entry| entry.path.ends_with("001.jpg"))
                .count(),
            1
        );
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
        let entry = smart_entry(r"C:\Books\sample.cbz", 0, "");
        let edits = SmartEditKeySets::default();
        let converted = HashMap::new();
        assert!(metadata_filter_passes(
            &filter, &entry, &key, &ratings, &tags, &edits, &converted,
        ));
        filter.tags.clear();
        filter.include_untagged = true;
        assert!(!metadata_filter_passes(
            &filter, &entry, &key, &ratings, &tags, &edits, &converted,
        ));
    }

    #[test]
    fn metadata_tag_filter_keeps_folder_rows_navigable() {
        let key = "c:/books/series".to_string();
        let mut filter = crate::settings::SmartFolderFilter::default();
        filter.tags.insert("あとで読む".into());
        let mut entry = smart_entry(r"C:\Books\series", 0, "");
        entry.kind = SmartFolderEntryKind::Folder;
        assert!(metadata_filter_passes(
            &filter,
            &entry,
            &key,
            &HashMap::new(),
            &HashMap::new(),
            &SmartEditKeySets::default(),
            &HashMap::new(),
        ));
    }

    #[test]
    fn metadata_edit_filter_checks_converted_archive_cache_pages() {
        use crate::settings::FacetEditFlag;

        let archive_path = PathBuf::from(r"C:\Books\sample.rar");
        let cache_path = PathBuf::from(r"C:\Cache\sample.zip");
        let key = crate::adjustment_db::normalize_path(&archive_path);
        let cache_key = crate::adjustment_db::normalize_path(&cache_path);
        let mut entry = smart_entry(r"C:\Books\sample.rar", 0, "");
        entry.kind = SmartFolderEntryKind::Archive;
        let mut filter = crate::settings::SmartFolderFilter::default();
        filter.edits.insert(FacetEditFlag::Adjustment);
        filter.edit_include_descendants = true;
        let mut edits = SmartEditKeySets::default();
        edits.adjustment.insert(format!("{cache_key}::page:0"));
        let converted = HashMap::from([(
            crate::path_key::normalize_keep_drive(&archive_path),
            cache_path,
        )]);

        assert!(metadata_filter_passes(
            &filter,
            &entry,
            &key,
            &HashMap::new(),
            &HashMap::new(),
            &edits,
            &converted,
        ));
    }

    #[test]
    fn flat_prepare_honors_custom_category_rows() {
        use crate::settings::{GridDisplayOrder, GridItemDisplayKind};
        let mut definition = crate::settings::SmartFolderDefinition::new("mixed");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            PathBuf::from(r"C:\Mixed"),
            true,
            true,
            Default::default(),
        ));
        let mut entries = vec![
            smart_entry(r"C:\Mixed\archive.zip", 0, ""),
            smart_entry(r"C:\Mixed\folder", 0, ""),
            smart_entry(r"C:\Mixed\image.jpg", 0, ""),
            smart_entry(r"C:\Mixed\video.mp4", 0, ""),
        ];
        entries[1].kind = SmartFolderEntryKind::Folder;
        entries[2].kind = SmartFolderEntryKind::Image;
        entries[3].kind = SmartFolderEntryKind::Video;
        let snapshot = SmartFolderSnapshot {
            definition,
            entries: Arc::new(entries),
            video_thumb_overrides: HashMap::new(),
            diag: SmartFolderDiag::default(),
        };
        let display_order = GridDisplayOrder::from_rows([
            vec![GridItemDisplayKind::VideoAudio],
            vec![GridItemDisplayKind::Image],
            vec![GridItemDisplayKind::Archive],
            vec![GridItemDisplayKind::Folder],
        ]);
        let cancel = AtomicBool::new(false);
        let (tx, _rx) = mpsc::channel();
        let prepared = prepare_smart_folder(
            snapshot,
            crate::settings::SortOrder::FileName,
            display_order,
            false,
            false,
            HashSet::new(),
            HashSet::new(),
            false,
            false,
            false,
            None,
            SmartFolderPrepareResources::default(),
            &cancel,
            &tx,
        )
        .unwrap()
        .unwrap();
        let names: Vec<_> = prepared
            .items
            .iter()
            .filter_map(|item| {
                item.drag_source_path()?
                    .file_name()?
                    .to_str()
                    .map(str::to_owned)
            })
            .collect();
        assert_eq!(
            names,
            ["video.mp4", "image.jpg", "archive.zip", "folder"].map(str::to_owned)
        );
    }

    #[test]
    fn folder_grouped_prepare_uses_active_sort_after_source_and_relative_folder() {
        let mut definition = crate::settings::SmartFolderDefinition::new("books");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            PathBuf::from(r"C:\Books"),
            true,
            true,
            Default::default(),
        ));
        definition.grouping = crate::settings::SubfolderExpansionOrder::FolderGrouped;
        let snapshot = SmartFolderSnapshot {
            definition,
            entries: Arc::new(vec![
                smart_entry(r"D:\SourceB\A\a.zip", 1, "A"),
                smart_entry(r"C:\SourceA\B\z.zip", 0, "B"),
                smart_entry(r"C:\SourceA\A\m.zip", 0, "A"),
            ]),
            video_thumb_overrides: HashMap::new(),
            diag: SmartFolderDiag::default(),
        };
        let cancel = AtomicBool::new(false);
        let (tx, _rx) = mpsc::channel();
        let prepared = prepare_smart_folder(
            snapshot,
            crate::settings::SortOrder::FileName,
            crate::settings::GridDisplayOrder::default(),
            false,
            false,
            HashSet::new(),
            HashSet::new(),
            false,
            false,
            false,
            None,
            SmartFolderPrepareResources::default(),
            &cancel,
            &tx,
        )
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

    /// Manual release-gate benchmark. Run each size in a fresh test process so the external
    /// harness can sample WorkingSet independently:
    /// `MIV_SMART_FOLDER_BENCH_ITEMS=100000 cargo test --bin mimageviewer-core
    /// smart_folder_prepare_scale_benchmark -- --ignored --nocapture`
    #[test]
    #[ignore = "manual 100k/500k/2m smart-folder prepare measurement"]
    fn smart_folder_prepare_scale_benchmark() {
        let count = std::env::var("MIV_SMART_FOLDER_BENCH_ITEMS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(100_000);
        let mut definition = crate::settings::SmartFolderDefinition::new("scale-benchmark");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            PathBuf::from(r"C:\SmartFolderBench"),
            true,
            true,
            Default::default(),
        ));
        let entries = (0..count)
            .map(|index| {
                let mut entry =
                    smart_entry(&format!(r"C:\SmartFolderBench\item-{index:07}.jpg"), 0, "");
                entry.kind = SmartFolderEntryKind::Image;
                entry
            })
            .collect();
        let snapshot = SmartFolderSnapshot {
            definition,
            entries: Arc::new(entries),
            video_thumb_overrides: HashMap::new(),
            diag: SmartFolderDiag::default(),
        };
        let cancel = AtomicBool::new(false);
        let (tx, _rx) = mpsc::channel();
        let started = Instant::now();
        let prepared = prepare_smart_folder(
            snapshot,
            crate::settings::SortOrder::FileName,
            crate::settings::GridDisplayOrder::default(),
            false,
            false,
            HashSet::new(),
            HashSet::new(),
            false,
            false,
            false,
            None,
            SmartFolderPrepareResources::default(),
            &cancel,
            &tx,
        )
        .unwrap()
        .unwrap();
        assert_eq!(prepared.items.len(), count);
        let initial_elapsed = started.elapsed();
        let resort_snapshot = prepared.snapshot.clone();
        let resort_metadata = Arc::clone(&prepared.resort_metadata);
        drop(prepared);
        let resort_started = Instant::now();
        let resort = prepare_smart_folder(
            resort_snapshot,
            crate::settings::SortOrder::DateDesc,
            crate::settings::GridDisplayOrder::default(),
            false,
            false,
            HashSet::new(),
            HashSet::new(),
            true,
            true,
            true,
            Some(resort_metadata),
            SmartFolderPrepareResources::default(),
            &cancel,
            &tx,
        )
        .unwrap()
        .unwrap();
        assert_eq!(resort.items.len(), count);
        eprintln!(
            "smart_folder_prepare_scale items={count} initial_ms={:.1} sort_only_ms={:.1}",
            initial_elapsed.as_secs_f64() * 1000.0,
            resort_started.elapsed().as_secs_f64() * 1000.0
        );
    }

    #[test]
    fn sort_only_prepare_reuses_path_metadata_without_db_phases() {
        let definition_id = uuid::Uuid::new_v4();
        let mut definition = crate::settings::SmartFolderDefinition::new("再ソート");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            PathBuf::from(r"C:\Books"),
            true,
            true,
            crate::settings::SmartFolderFilter::default(),
        ));
        definition.id = definition_id;
        let a = PathBuf::from(r"C:\Books\a.zip");
        let b = PathBuf::from(r"C:\Books\b.zip");
        let a_key = crate::adjustment_db::normalize_path(&a);
        let b_key = crate::adjustment_db::normalize_path(&b);
        let snapshot = SmartFolderSnapshot {
            definition,
            entries: Arc::new(vec![
                SmartFolderEntry {
                    path: b.clone(),
                    ..smart_entry(r"C:\Books\b.zip", 0, "")
                },
                SmartFolderEntry {
                    path: a.clone(),
                    ..smart_entry(r"C:\Books\a.zip", 0, "")
                },
            ]),
            video_thumb_overrides: HashMap::new(),
            diag: SmartFolderDiag::default(),
        };
        let reused = ReusedSmartFolderMetadata {
            normalized_keys: Arc::new(vec![b_key.clone(), a_key.clone()]),
            included_entry_indices: Arc::new(vec![0, 1]),
            ratings_by_path: HashMap::from([(a_key.clone(), 4)]),
            tags_by_path: HashMap::from([(b_key.clone(), vec!["本".into()])]),
            local_adjust_paths: HashSet::from([b_key.clone()]),
            ..ReusedSmartFolderMetadata::default()
        };
        let reused = Arc::new(reused);
        let reused_identity = Arc::clone(&reused);
        let (tx, rx) = mpsc::channel();
        let prepared = prepare_smart_folder(
            snapshot,
            crate::settings::SortOrder::FileName,
            crate::settings::GridDisplayOrder::default(),
            false,
            false,
            HashSet::new(),
            HashSet::new(),
            true,
            true,
            true,
            Some(reused),
            SmartFolderPrepareResources::default(),
            &AtomicBool::new(false),
            &tx,
        )
        .unwrap()
        .unwrap();
        assert!(Arc::ptr_eq(&prepared.resort_metadata, &reused_identity));

        assert!(matches!(
            prepared.items.as_slice(),
            [GridItem::ZipFile(first), GridItem::ZipFile(second)] if first == &a && second == &b
        ));
        assert_eq!(prepared.metadata.rating_cache, HashMap::from([(0, 4)]));
        assert_eq!(
            prepared.metadata.tags_cache,
            HashMap::from([(b_key, vec!["本".into()])])
        );
        assert_eq!(prepared.metadata.local_adjust_pages, HashSet::from([1]));
        let phases = rx
            .try_iter()
            .filter_map(|event| match event {
                SmartFolderPrepareEvent::Progress(progress) => Some(progress.phase),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!phases.contains(&SmartFolderPhase::Ratings));
        assert!(!phases.contains(&SmartFolderPhase::Tags));
        assert!(!phases.contains(&SmartFolderPhase::Adjustments));
    }

    #[test]
    fn deleted_paths_are_removed_from_cached_smart_folder_snapshot() {
        let definition = crate::settings::SmartFolderDefinition::new("books");
        let mut snapshot = SmartFolderSnapshot {
            definition,
            entries: Arc::new(vec![
                smart_entry(r"C:\Books\keep.zip", 0, ""),
                smart_entry(r"C:\Books\deleted.zip", 0, ""),
            ]),
            video_thumb_overrides: HashMap::new(),
            diag: SmartFolderDiag {
                containers_found: 2,
                ..Default::default()
            },
        };
        let removed = [crate::path_key::normalize_keep_drive(Path::new(
            r"C:\Books\deleted.zip",
        ))]
        .into_iter()
        .collect();

        assert!(remove_paths_from_smart_folder_snapshot(
            &mut snapshot,
            &removed
        ));
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.diag.containers_found, 1);
        assert!(snapshot.entries[0].path.ends_with("keep.zip"));
    }

    #[test]
    fn removed_folder_tombstone_excludes_descendants() {
        let definition = crate::settings::SmartFolderDefinition::new("books");
        let mut snapshot = SmartFolderSnapshot {
            definition,
            entries: Arc::new(vec![
                smart_entry(r"C:\Books\keep.zip", 0, ""),
                smart_entry(r"C:\Books\renamed\child.zip", 0, "renamed"),
            ]),
            video_thumb_overrides: HashMap::new(),
            diag: SmartFolderDiag::default(),
        };
        let removed = [crate::path_key::normalize_keep_drive(Path::new(
            r"C:\Books\renamed",
        ))]
        .into_iter()
        .collect();
        assert!(remove_paths_from_smart_folder_snapshot(
            &mut snapshot,
            &removed
        ));
        assert_eq!(snapshot.entries.len(), 1);
        assert!(snapshot.entries[0].path.ends_with("keep.zip"));
    }

    #[test]
    fn authoritative_rescan_ignores_old_tombstones_but_keeps_concurrent_deletes() {
        let old = crate::path_key::normalize_keep_drive(Path::new(r"C:\Books\recreated.zip"));
        let late = crate::path_key::normalize_keep_drive(Path::new(r"C:\Books\deleted-late.zip"));
        let at_scan_start = HashSet::from([old.clone()]);
        let current = HashSet::from([old, late.clone()]);

        assert_eq!(
            smart_folder_tombstones_after_scan_start(&current, &at_scan_start),
            HashSet::from([late])
        );
    }

    #[test]
    fn prepare_applies_drive_preserving_concurrent_tombstone() {
        let mut definition = crate::settings::SmartFolderDefinition::new("books");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            PathBuf::from(r"C:\Books"),
            true,
            true,
            Default::default(),
        ));
        let deleted = PathBuf::from(r"C:\Books\deleted.zip");
        let snapshot = SmartFolderSnapshot {
            definition,
            entries: Arc::new(vec![smart_entry(deleted.to_string_lossy().as_ref(), 0, "")]),
            video_thumb_overrides: HashMap::new(),
            diag: SmartFolderDiag::default(),
        };
        let removed = HashSet::from([crate::path_key::normalize_keep_drive(&deleted)]);
        let cancel = AtomicBool::new(false);
        let (tx, _rx) = mpsc::channel();

        let prepared = prepare_smart_folder(
            snapshot,
            crate::settings::SortOrder::FileName,
            crate::settings::GridDisplayOrder::default(),
            false,
            true,
            HashSet::new(),
            removed,
            false,
            false,
            false,
            None,
            SmartFolderPrepareResources::default(),
            &cancel,
            &tx,
        )
        .unwrap()
        .unwrap();

        assert!(prepared.items.is_empty());
    }

    #[test]
    fn smart_folder_tombstones_compact_and_clear_for_unique_snapshot() {
        let definition = crate::settings::SmartFolderDefinition::new("books");
        let mut snapshot = SmartFolderSnapshot {
            definition,
            entries: Arc::new(vec![
                smart_entry(r"C:\Books\keep.zip", 0, ""),
                smart_entry(r"C:\Books\deleted.zip", 0, ""),
            ]),
            video_thumb_overrides: HashMap::new(),
            diag: SmartFolderDiag {
                containers_found: 2,
                ..Default::default()
            },
        };
        let mut tombstones = [crate::path_key::normalize_keep_drive(Path::new(
            r"C:\Books\deleted.zip",
        ))]
        .into_iter()
        .collect();

        assert!(compact_smart_folder_tombstones_if_unique(
            &mut snapshot,
            &mut tombstones,
        ));
        assert!(tombstones.is_empty());
        assert_eq!(snapshot.entries.len(), 1);
        assert!(snapshot.entries[0].path.ends_with("keep.zip"));
    }

    #[test]
    fn smart_folder_tombstones_wait_for_shared_snapshot_then_compact() {
        let definition = crate::settings::SmartFolderDefinition::new("books");
        let mut snapshot = SmartFolderSnapshot {
            definition,
            entries: Arc::new(vec![
                smart_entry(r"C:\Books\keep.zip", 0, ""),
                smart_entry(r"C:\Books\deleted.zip", 0, ""),
            ]),
            video_thumb_overrides: HashMap::new(),
            diag: SmartFolderDiag::default(),
        };
        let shared_entries = Arc::clone(&snapshot.entries);
        let mut tombstones = [crate::path_key::normalize_keep_drive(Path::new(
            r"C:\Books\deleted.zip",
        ))]
        .into_iter()
        .collect();

        assert!(!compact_smart_folder_tombstones_if_unique(
            &mut snapshot,
            &mut tombstones,
        ));
        assert_eq!(snapshot.entries.len(), 2);
        assert_eq!(tombstones.len(), 1);

        drop(shared_entries);
        assert!(compact_smart_folder_tombstones_if_unique(
            &mut snapshot,
            &mut tombstones,
        ));
        assert!(tombstones.is_empty());
        assert_eq!(snapshot.entries.len(), 1);
    }

    #[test]
    fn display_name_and_grouping_do_not_change_scan_identity() {
        let mut left = crate::settings::SmartFolderDefinition::new("before");
        left.rules.push(rule(
            uuid::Uuid::new_v4(),
            PathBuf::from(r"C:\Books"),
            true,
            true,
            Default::default(),
        ));
        let mut right = left.clone();
        right.name = "after".into();
        right.grouping = crate::settings::SubfolderExpansionOrder::FolderGrouped;
        assert!(smart_folder_scan_rules_match(&left, &right));

        right.rules[0].include_descendants = false;
        assert!(!smart_folder_scan_rules_match(&left, &right));
    }

    #[test]
    fn synthetic_path_round_trips_definition_id() {
        let id = uuid::Uuid::new_v4();
        let path = smart_folder_synthetic_path(id);
        assert_eq!(smart_folder_id_from_synthetic_path(&path), Some(id));
        assert!(is_smart_folder_synthetic_path(&path));
    }
}
