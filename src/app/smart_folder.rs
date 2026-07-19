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
    video_items: Vec<(usize, PathBuf, u64)>,
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
struct ActiveRule {
    id: uuid::Uuid,
    source: PathBuf,
    definition_order: usize,
    include_descendants: bool,
    filter: crate::settings::SmartFolderFilter,
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
        .map(|(definition_order, rule)| ActiveRule {
            id: rule.id,
            source: rule.source.clone(),
            definition_order,
            include_descendants: rule.include_descendants,
            filter: rule.filter.clone(),
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
    mtime: i64,
    file_size: i64,
    filter: &crate::settings::SmartFolderFilter,
    now: i64,
) -> bool {
    if !filter.kinds.is_empty() && !filter.kinds.contains(&kind.setting_kind()) {
        return false;
    }
    let name = path
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

    for candidate in candidates {
        let SmartFolderCandidate {
            path,
            kind,
            mtime,
            file_size,
        } = candidate;
        let mut matching_rules = applicable_rules
            .iter()
            .copied()
            .filter(|rule| {
                passes_cheap_filter_values(kind, &path, mtime, file_size, &rule.filter, now)
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
    diag.source_failures = rules
        .iter()
        .filter(|rule| {
            !scanned_root_keys.contains(&crate::path_key::normalize_keep_drive(&rule.source))
        })
        .count();

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
    entry: &SmartFolderEntry,
    key: &str,
    ratings: &HashMap<String, u8>,
    tags: &HashMap<String, Vec<String>>,
    edits: &SmartEditKeySets,
) -> bool {
    use crate::settings::{FacetEditFlag, FacetTagMode};

    let rating = ratings.get(key).copied().unwrap_or(0).min(5) as usize;
    if !filter.ratings[rating] {
        return false;
    }
    let item_tags = tags.get(key).map(Vec::as_slice).unwrap_or(&[]);
    if !filter.tags.is_empty() || filter.include_untagged {
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
            FacetEditFlag::Adjustment | FacetEditFlag::AiAdjustment => edit_key_matches(
                &edits.adjustment,
                entry,
                key,
                filter.edit_include_descendants,
            ),
            FacetEditFlag::LocalAdjustment => edit_key_matches(
                &edits.local_adjust,
                entry,
                key,
                filter.edit_include_descendants,
            ),
            FacetEditFlag::Mask => {
                edit_key_matches(&edits.mask, entry, key, filter.edit_include_descendants)
            }
            FacetEditFlag::Conceal => {
                edit_key_matches(&edits.conceal, entry, key, filter.edit_include_descendants)
            }
            FacetEditFlag::Annotation => edit_key_matches(
                &edits.annotation,
                entry,
                key,
                filter.edit_include_descendants,
            ),
            FacetEditFlag::Rotation => {
                edit_key_matches(&edits.rotation, entry, key, filter.edit_include_descendants)
            }
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
    name: crate::filename_sort::SortNameKey,
    relative_parent: crate::filename_sort::SortNameKey,
}

fn prepare_smart_folder(
    snapshot: SmartFolderSnapshot,
    sort: crate::settings::SortOrder,
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

    let wanted_edit_flags = snapshot
        .definition
        .rules
        .iter()
        .flat_map(|rule| rule.filter.edits.iter().copied())
        .collect::<std::collections::BTreeSet<_>>();
    let edit_keys = load_edit_key_sets(&wanted_edit_flags, local_adjust.iter().cloned().collect())?;

    report(SmartFolderPhase::Filtering, 0);
    let mut included = Vec::with_capacity(total);
    for (index, key) in keys.iter().enumerate() {
        if index.is_multiple_of(METADATA_CHUNK_SIZE) {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            report(SmartFolderPhase::Filtering, index);
        }
        let entry = &snapshot.entries[index];
        if entry.matching_rule_indices.iter().any(|rule_index| {
            snapshot
                .definition
                .rules
                .get(*rule_index)
                .is_some_and(|rule| {
                    metadata_filter_passes(&rule.filter, entry, key, &ratings, &tags, &edit_keys)
                })
        }) {
            included.push(index);
        }
    }

    report(SmartFolderPhase::Sorting, 0);
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
    let video_items = crate::filename_stack_ui::stack_video_items(&items, &image_metas);
    Ok(Some(PreparedSmartFolder {
        snapshot,
        items,
        image_metas,
        video_items,
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
    sort: crate::settings::SortOrder,
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
                sort,
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
        if active_rules(&definition).is_empty() {
            self.show_feedback_toast("表示条件を追加してください".into());
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
            self.settings.sort_order,
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
                        == active_rules(&result.snapshot.definition).len()
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
            video_items,
            metadata,
            refresh,
        } = prepared;
        let definition_id = snapshot.definition.id;
        let definition_name = snapshot.definition.name.clone();
        let diag = snapshot.diag.clone();
        let item_count = items.len();
        self.video_thumb_overrides.clear();
        self.video_thumb_overrides
            .extend(snapshot.video_thumb_overrides.clone());
        self.start_loading_subfolder_items(
            smart_folder_synthetic_path(definition_id),
            items,
            image_metas,
            video_items,
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
            self.cancel_smart_folder_pending();
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
            now,
            2 * 1024 * 1024,
            &filter,
            now,
        ));
        filter.extensions.clear();
        filter.extensions.insert("pdf".into());
        assert!(!passes_cheap_filter_values(
            SmartFolderEntryKind::Zip,
            &path,
            now,
            2 * 1024 * 1024,
            &filter,
            now,
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
        assert!(metadata_filter_passes(
            &filter, &entry, &key, &ratings, &tags, &edits,
        ));
        filter.tags.clear();
        filter.include_untagged = true;
        assert!(!metadata_filter_passes(
            &filter, &entry, &key, &ratings, &tags, &edits,
        ));
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
            false,
            false,
            false,
            false,
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

    #[test]
    fn synthetic_path_round_trips_definition_id() {
        let id = uuid::Uuid::new_v4();
        let path = smart_folder_synthetic_path(id);
        assert_eq!(smart_folder_id_from_synthetic_path(&path), Some(id));
        assert!(is_smart_folder_synthetic_path(&path));
    }
}
