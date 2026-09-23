//! スマートフォルダ: 現在の一覧条件から保存した複数ルールを OR 結合し、
//! 実フォルダ / 画像 / 動画 / 音声 / アーカイブを収集する flat snapshot view。

use super::*;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
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
    pub(crate) file_size: Option<i64>,
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
    Bookmarks,
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
            Self::Bookmarks => "ブックマークを確認中",
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
    /// Exact result count after saved rating/tag/edit filters were applied.  This is separate
    /// from `snapshot.entries.len()`, which is only the cheap filesystem-candidate upper bound.
    result_count: usize,
    /// Membership metadata already loaded by the exact-count worker. Reuse it after confirmation
    /// so a 700k-candidate rating query is not repeated just to materialize a small result.
    membership: Option<EvaluatedSmartFolderMembership>,
    membership_revision: Option<u64>,
    generation: u64,
    refresh: bool,
    tombstones_at_start: HashSet<String>,
}

struct CountedSmartFolder {
    snapshot: SmartFolderSnapshot,
    result_count: usize,
    membership: EvaluatedSmartFolderMembership,
    metadata_revision: u64,
    refresh: bool,
    tombstones_at_start: HashSet<String>,
}

struct PreparedSmartFolder {
    snapshot: SmartFolderSnapshot,
    presentation: SmartFolderPresentation,
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

/// Type-erased ownership sent to the smart-folder drop worker. Payloads are never read again;
/// only their destructors matter. Keeping the type `Send` makes the UI-to-worker ownership
/// boundary explicit while allowing both cached metadata and complete prepared results.
pub(crate) type RetiredSmartFolderPayload = Box<dyn Send + 'static>;

/// Cancel in-flight work and move every owner that can retain a large queued result into the
/// payload-drop lane.  Dropping only the receiver on the UI thread is not cheap: a `Done` message
/// may already contain a million-entry snapshot/prepared grid, and a confirmation owner retains
/// the scanned snapshot directly.
fn cancelled_smart_folder_payloads(
    scan: Option<SmartFolderPending>,
    prepare: Option<SmartFolderPreparePending>,
    confirm: Option<SmartFolderConfirmPending>,
) -> Vec<RetiredSmartFolderPayload> {
    let mut retired = Vec::with_capacity(3);
    if let Some(pending) = scan {
        pending.cancel();
        retired.push(Box::new(pending) as RetiredSmartFolderPayload);
    }
    if let Some(pending) = prepare {
        pending.cancel();
        retired.push(Box::new(pending) as RetiredSmartFolderPayload);
    }
    if let Some(confirm) = confirm {
        retired.push(Box::new(confirm) as RetiredSmartFolderPayload);
    }
    retired
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
    converted_archive_cache_paths: HashMap<String, ConvertedArchiveSourceState>,
}

/// The already-materialized root grid is moved here while the user is inside a folder, PDF, or
/// archive opened from the smart folder. Moving (rather than cloning) keeps million-row sessions
/// bounded, and restoring this state does not run the metadata/filter/sort/build pipeline again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SmartFolderPreparedGridMode {
    Thumbnail { cols: usize },
    Details,
}

impl SmartFolderPreparedGridMode {
    fn current(settings: &crate::settings::Settings) -> Self {
        match settings.grid_view_mode {
            crate::settings::GridViewMode::Thumbnail => Self::Thumbnail {
                cols: settings.grid_cols.max(1),
            },
            crate::settings::GridViewMode::Details => Self::Details,
        }
    }
}

struct SmartFolderPreparedGridLayout {
    /// The offset and the auto-aspect state that determined its row height are one snapshot.
    /// `AutoAspectState::samples` is also index-keyed, so moving only the resolved aspect would
    /// leave the restored grid with another context's decision inputs.
    scroll_offset_y: f32,
    auto_aspect: crate::auto_aspect::AutoAspectState,
    /// This is comparison-only: changed user settings are never restored. A mismatch asks the
    /// ordinary folder-navigation scroll owner to keep `selected` visible in the new layout.
    mode: SmartFolderPreparedGridMode,
    window_inner_size: Option<[f32; 2]>,
}

impl SmartFolderPreparedGridLayout {
    fn matches_current_layout(&self, app: &App) -> bool {
        fn size_matches(left: Option<[f32; 2]>, right: Option<[f32; 2]>) -> bool {
            match (left, right) {
                (Some(left), Some(right)) => {
                    (left[0] - right[0]).abs() <= 0.5 && (left[1] - right[1]).abs() <= 0.5
                }
                (None, None) => true,
                _ => false,
            }
        }

        self.mode == SmartFolderPreparedGridMode::current(&app.settings)
            && size_matches(self.window_inner_size, app.last_inner_size)
    }
}

pub(crate) struct SmartFolderPreparedGrid {
    items: Vec<GridItem>,
    thumbnails: Vec<ThumbnailState>,
    image_metas: Vec<Option<(i64, i64)>>,
    video_thumb_overrides: HashMap<String, PathBuf>,
    selected: Option<usize>,
    grid_click_selection_anchor_index: Option<usize>,
    layout: SmartFolderPreparedGridLayout,
    scroll_to_selected: bool,
    pending_grid_scroll: Option<super::GridScrollIntent>,
    checked: HashSet<usize>,
    visible_indices: Vec<usize>,
    details_order: Vec<usize>,
    show_search_bar: bool,
    search_query: String,
    search_filter: Option<HashSet<usize>>,
    search_filter_origin_folder: Option<PathBuf>,
    rating_cache: HashMap<usize, u8>,
    tags_cache: HashMap<String, Vec<String>>,
    local_adjust_pages: HashSet<usize>,
    adjustment_page_params: HashMap<usize, crate::adjustment::AdjustParams>,
    export_crop_page_settings: HashMap<usize, crate::export_crop::CropSettings>,
    view_trim_page_overrides: HashMap<usize, crate::view_trim::ViewTrimPageOverride>,
    mask_pages: HashSet<usize>,
    conceal_pages: HashSet<usize>,
    comic_pages: HashSet<usize>,
    folder_pin_map: HashMap<String, crate::folder_thumb_pins::FolderPinSource>,
    converted_archive_cache_paths: HashMap<String, ConvertedArchiveSourceState>,
    color_cache_map: Option<Arc<std::sync::RwLock<HashMap<String, crate::catalog::CacheEntry>>>>,
    color_catalog: Option<Arc<crate::catalog::CatalogDb>>,
}

fn remap_smart_folder_grid_index(index: usize, removed: &[usize]) -> Option<usize> {
    if removed.binary_search(&index).is_ok() {
        return None;
    }
    Some(index - removed.partition_point(|removed_index| *removed_index < index))
}

fn remap_smart_folder_grid_set(values: &mut HashSet<usize>, removed: &[usize]) {
    *values = values
        .drain()
        .filter_map(|index| remap_smart_folder_grid_index(index, removed))
        .collect();
}

fn remap_smart_folder_grid_map<T>(values: &mut HashMap<usize, T>, removed: &[usize]) {
    *values = values
        .drain()
        .filter_map(|(index, value)| {
            remap_smart_folder_grid_index(index, removed).map(|index| (index, value))
        })
        .collect();
}

impl SmartFolderPreparedGrid {
    fn remove_paths(&mut self, removed_paths: &HashSet<String>) -> bool {
        let removed = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let path = item.drag_source_path()?;
                smart_folder_path_is_removed(
                    &crate::path_key::normalize_keep_drive(path),
                    removed_paths,
                )
                .then_some(index)
            })
            .collect::<Vec<_>>();
        if removed.is_empty() {
            return false;
        }
        for &index in removed.iter().rev() {
            self.items.remove(index);
            self.thumbnails.remove(index);
            self.image_metas.remove(index);
        }
        let remap_indices = |values: &mut Vec<usize>| {
            *values = values
                .drain(..)
                .filter_map(|index| remap_smart_folder_grid_index(index, &removed))
                .collect();
        };
        remap_indices(&mut self.visible_indices);
        remap_indices(&mut self.details_order);
        remap_smart_folder_grid_set(&mut self.checked, &removed);
        if let Some(filter) = self.search_filter.as_mut() {
            remap_smart_folder_grid_set(filter, &removed);
        }
        remap_smart_folder_grid_set(&mut self.local_adjust_pages, &removed);
        remap_smart_folder_grid_set(&mut self.mask_pages, &removed);
        remap_smart_folder_grid_set(&mut self.conceal_pages, &removed);
        remap_smart_folder_grid_set(&mut self.comic_pages, &removed);
        remap_smart_folder_grid_map(&mut self.rating_cache, &removed);
        remap_smart_folder_grid_map(&mut self.adjustment_page_params, &removed);
        remap_smart_folder_grid_map(&mut self.export_crop_page_settings, &removed);
        remap_smart_folder_grid_map(&mut self.view_trim_page_overrides, &removed);
        remap_smart_folder_grid_map(&mut self.layout.auto_aspect.samples, &removed);
        self.layout.auto_aspect.streak = None;
        self.grid_click_selection_anchor_index = self
            .grid_click_selection_anchor_index
            .and_then(|index| remap_smart_folder_grid_index(index, &removed));
        if let Some(index) = self.selected {
            self.selected = remap_smart_folder_grid_index(index, &removed).or_else(|| {
                (!self.items.is_empty()).then_some(
                    (index - removed.partition_point(|removed_index| *removed_index < index))
                        .min(self.items.len() - 1),
                )
            });
        }
        self.tags_cache
            .retain(|path, _| !smart_folder_path_is_removed(path, removed_paths));
        self.video_thumb_overrides.retain(|video_path, image_path| {
            !smart_folder_path_is_removed(video_path, removed_paths)
                && !smart_folder_path_is_removed(
                    &crate::path_key::normalize_keep_drive(image_path),
                    removed_paths,
                )
        });
        self.folder_pin_map
            .retain(|path, _| !smart_folder_path_is_removed(path, removed_paths));
        self.converted_archive_cache_paths.retain(|path, state| {
            !smart_folder_path_is_removed(path, removed_paths)
                && state.load_path().is_none_or(|cache_path| {
                    !smart_folder_path_is_removed(
                        &crate::path_key::normalize_keep_drive(cache_path),
                        removed_paths,
                    )
                })
        });
        true
    }
}

/// One owner for the settled session boundary. `TopLevelGridView` drops this value on every
/// explicit top-level transition (`begin`) and preserves it only for same-smart-folder scoped
/// navigation (`replace_surface`). History can still retain `SmartFolderViewState`, but it cannot
/// retain this snapshot/result owner after the session has been left.
pub(crate) struct SmartFolderSession {
    definition_id: uuid::Uuid,
    snapshot: Option<SmartFolderSnapshot>,
    resort_metadata: Option<Arc<ReusedSmartFolderMetadata>>,
    presentation: SmartFolderPresentation,
    metadata_revision: u64,
    /// Provenance of the staged navigation that installed this visible session. A Remote
    /// fullscreen restore can wait for this exact adoption instead of mistaking old rows for
    /// the refreshed result.
    adopted_request_id: Option<u64>,
    phase: SmartFolderOpenPhase,
}

enum SmartFolderOpenPhase {
    Root,
    Child {
        logical_path: PathBuf,
        parked_root: SmartFolderRootPayload,
    },
}

/// A resident Smart root has an exact moved grid. A no-resident history continuation has never
/// displayed its prepared root, so it keeps the worker result instead of fabricating a UI grid.
enum SmartFolderRootPayload {
    Visible(SmartFolderPreparedGrid),
    Offscreen(Box<PreparedSmartFolder>),
}

impl SmartFolderOpenPhase {
    fn visible(&self) -> &Self {
        self
    }

    fn into_parked_root(self) -> Option<SmartFolderRootPayload> {
        match self {
            Self::Root => None,
            Self::Child { parked_root, .. } => Some(parked_root),
        }
    }

    fn parked_root_mut(&mut self) -> Option<&mut SmartFolderPreparedGrid> {
        match self {
            Self::Root => None,
            Self::Child { parked_root, .. } => match parked_root {
                SmartFolderRootPayload::Visible(grid) => Some(grid),
                SmartFolderRootPayload::Offscreen(_) => None,
            },
        }
    }

    fn offscreen_root(&self) -> Option<&PreparedSmartFolder> {
        match self.visible() {
            Self::Child {
                parked_root: SmartFolderRootPayload::Offscreen(prepared),
                ..
            } => Some(prepared),
            _ => None,
        }
    }

    fn offscreen_root_mut(&mut self) -> Option<&mut PreparedSmartFolder> {
        match self {
            Self::Child {
                parked_root: SmartFolderRootPayload::Offscreen(prepared),
                ..
            } => Some(prepared),
            _ => None,
        }
    }
}

pub(crate) enum SmartFolderOpenOrigin {
    Direct(super::top_level_grid_view::TopLevelGridRestore),
    ReturnReprepare {
        prior: super::top_level_grid_view::TopLevelGridRestore,
        anchor: Option<PathBuf>,
    },
}

impl SmartFolderOpenOrigin {
    fn prior(&self) -> &super::top_level_grid_view::TopLevelGridRestore {
        match self {
            Self::Direct(prior) | Self::ReturnReprepare { prior, .. } => prior,
        }
    }

    fn into_prior(self) -> super::top_level_grid_view::TopLevelGridRestore {
        match self {
            Self::Direct(prior) | Self::ReturnReprepare { prior, .. } => prior,
        }
    }

    #[cfg(test)]
    pub(crate) fn legacy_path(&self) -> Option<PathBuf> {
        self.prior().legacy_path()
    }
}

impl SmartFolderSession {
    fn new(
        snapshot: SmartFolderSnapshot,
        resort_metadata: Arc<ReusedSmartFolderMetadata>,
        presentation: SmartFolderPresentation,
        metadata_revision: u64,
    ) -> Self {
        Self {
            definition_id: snapshot.definition.id,
            snapshot: Some(snapshot),
            resort_metadata: Some(resort_metadata),
            presentation,
            metadata_revision,
            adopted_request_id: None,
            phase: SmartFolderOpenPhase::Root,
        }
    }

    pub(crate) fn definition_id(&self) -> uuid::Uuid {
        self.definition_id
    }

    /// The resident root has one snapshot owner: either the installed session or the parked
    /// offscreen prepare result. Its scan entries remain stable across deletes; tombstones carry
    /// removals until the next prepare, as they do for an installed root.
    fn root_snapshot(&self) -> Option<&SmartFolderSnapshot> {
        self.phase
            .offscreen_root()
            .map(|prepared| &prepared.snapshot)
            .or(self.snapshot.as_ref())
    }

    fn root_snapshot_mut(&mut self) -> Option<&mut SmartFolderSnapshot> {
        if let Some(prepared) = self.phase.offscreen_root_mut() {
            Some(&mut prepared.snapshot)
        } else {
            self.snapshot.as_mut()
        }
    }

    fn root_resort_metadata(&self) -> Option<Arc<ReusedSmartFolderMetadata>> {
        self.phase
            .offscreen_root()
            .map(|prepared| Arc::clone(&prepared.resort_metadata))
            .or_else(|| self.resort_metadata.clone())
    }

    #[cfg(test)]
    pub(crate) fn has_reused_metadata(&self) -> bool {
        self.resort_metadata.is_some()
    }

    #[cfg(test)]
    pub(crate) fn has_prepared_grid(&self) -> bool {
        matches!(self.phase.visible(), SmartFolderOpenPhase::Child { .. })
    }

    #[cfg(test)]
    pub(crate) fn has_offscreen_root(&self) -> bool {
        self.phase.offscreen_root().is_some()
    }

    fn owns_load(&self, path: &Path, source_alias: Option<&Path>) -> bool {
        let matches_load = |target: &Path| {
            crate::folder_tree::path_eq(target, path)
                || source_alias.is_some_and(|alias| crate::folder_tree::path_eq(target, alias))
        };
        match &self.phase {
            SmartFolderOpenPhase::Child { logical_path, .. } => matches_load(logical_path),
            SmartFolderOpenPhase::Root => false,
        }
    }

    fn returned_to_root(&mut self) {
        self.phase = SmartFolderOpenPhase::Root;
    }

    fn active_root_entry(
        &self,
        state: &super::top_level_grid_view::SmartFolderViewState,
    ) -> Option<PathBuf> {
        (state.definition_id == self.definition_id)
            .then_some(self.phase.visible())
            .and_then(|phase| match phase {
                SmartFolderOpenPhase::Child { logical_path, .. } => Some(logical_path.as_path()),
                _ => None,
            })
            .and_then(|path| match &state.position {
                super::top_level_grid_view::SmartFolderPosition::Container {
                    root_entry, ..
                } => Some(root_entry.as_path()),
                _ => state.containing_entry(path),
            })
            .map(Path::to_path_buf)
    }

    fn root_parent_target(&self) -> bool {
        matches!(self.phase.visible(), SmartFolderOpenPhase::Child { .. })
    }
}

static SMART_FOLDER_SESSION_DROP_BACKLOG: OnceLock<Mutex<Vec<RetiredSmartFolderPayload>>> =
    OnceLock::new();

fn retire_smart_folder_session_payloads(values: Vec<RetiredSmartFolderPayload>) {
    let backlog = SMART_FOLDER_SESSION_DROP_BACKLOG.get_or_init(|| Mutex::new(Vec::new()));
    let mut retired = std::mem::take(&mut *backlog.lock().unwrap());
    retired.extend(values);
    if retired.is_empty() {
        return;
    }
    let (tx, rx) = mpsc::channel::<Vec<RetiredSmartFolderPayload>>();
    let spawn = std::thread::Builder::new()
        .name("smart-folder-session-drop".into())
        .spawn(move || {
            if let Ok(retired) = rx.recv() {
                drop(retired);
            }
        });
    if let Ok(_thread) = spawn {
        if let Err(error) = tx.send(retired) {
            backlog.lock().unwrap().extend(error.0);
        }
        return;
    }
    backlog.lock().unwrap().extend(retired);
}

impl Drop for SmartFolderSession {
    fn drop(&mut self) {
        let mut retired = Vec::with_capacity(3);
        if let Some(snapshot) = self.snapshot.take() {
            retired.push(Box::new(snapshot) as RetiredSmartFolderPayload);
        }
        if let Some(metadata) = self.resort_metadata.take() {
            retired.push(Box::new(metadata) as RetiredSmartFolderPayload);
        }
        if let Some(grid) =
            std::mem::replace(&mut self.phase, SmartFolderOpenPhase::Root).into_parked_root()
        {
            retired.push(Box::new(grid) as RetiredSmartFolderPayload);
        }
        retire_smart_folder_session_payloads(retired);
    }
}

enum SmartFolderPrepareEvent {
    Progress(SmartFolderProgress),
    Counted(Box<CountedSmartFolder>),
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

/// A staged Smart navigation is authorized by the visible owner's meaning, not by its item
/// generation. A PDF placeholder may finish verification while this request is waiting; that
/// changes `items_generation` without handing navigation authority to a different viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SmartFolderSourceIdentity {
    Folder(Option<PathBuf>),
    Search {
        view: super::top_level_grid_view::TopLevelSearchView,
        query: String,
        executed: String,
        location: Option<PathBuf>,
    },
    Smart {
        definition_id: uuid::Uuid,
        location: Option<PathBuf>,
    },
    Collection {
        identity: super::top_level_grid_view::CollectionGridIdentity,
        position: super::top_level_grid_view::CollectionGridPosition,
    },
    Other(super::top_level_grid_view::TopLevelGridSurface),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SmartFolderSourceLease {
    context_id: super::viewer_context_registry::ViewerContextId,
    /// Stable for a single top-level owner even when its rows/revision finish loading.
    surface_generation: u64,
    quick_slot: Option<super::QuickFolderSlotId>,
    source: SmartFolderSourceIdentity,
}

/// The next Smart navigation has exactly one worker phase and does not borrow the installed
/// surface. Installed Smart presentation work and a new navigation are different owners.
enum SmartFolderTransitionPhase {
    RootScan(SmartFolderPending),
    RootCount(SmartFolderPreparePending),
    RootConfirm(SmartFolderConfirmPending),
    RootPrepare(SmartFolderPreparePending),
    RootReady(Box<PreparedSmartFolder>),
    ChildPreflight {
        root: SmartFolderTransitionRoot,
        child: SmartPhysicalPreflight,
    },
    ChildReady {
        root: SmartFolderTransitionRoot,
        child: SmartPhysicalReady,
    },
    Retired,
}

enum SmartFolderTransitionRoot {
    Resident,
    Offscreen(Box<PreparedSmartFolder>),
}

#[derive(Clone)]
struct SmartChildSource {
    logical_source: PathBuf,
    load_path: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SmartChildKind {
    Folder,
    Pdf,
    Zip,
    ConvertibleArchive,
}

fn smart_root_navigation_entries(
    items: &[GridItem],
) -> Vec<super::top_level_grid_view::SmartRootNavEntry> {
    use super::top_level_grid_view::SmartRootNavEntry;
    items
        .iter()
        .filter_map(|item| {
            let (logical_path, kind) = match item {
                GridItem::Folder(path) => (path, SmartChildKind::Folder),
                GridItem::PdfFile(path) => (path, SmartChildKind::Pdf),
                GridItem::ZipFile(path) => (path, SmartChildKind::Zip),
                GridItem::ConvertibleArchive { path, .. } => {
                    (path, SmartChildKind::ConvertibleArchive)
                }
                _ => return None,
            };
            Some(SmartRootNavEntry {
                logical_path: logical_path.clone(),
                kind,
            })
        })
        .collect()
}

enum SmartFolderTransitionTarget {
    Root(super::top_level_grid_view::SmartFolderViewState),
    Child {
        state: super::top_level_grid_view::SmartFolderViewState,
        source: SmartChildSource,
        kind: SmartChildKind,
        auto_fullscreen: bool,
        effects: SmartPhysicalOpenEffects,
        archive_commit: Option<super::SmartFolderArchiveCommit>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SmartHistoryDirection {
    Back,
    Forward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SmartFolderRequestStatus {
    Pending,
    Adopted,
    Retired,
}

pub(crate) enum StagedSmartHistoryAction {
    Handled,
    Dispatch {
        target: super::FolderNavHistoryTarget,
        rollback: super::FolderNavHistorySnapshot,
    },
}

#[derive(Clone)]
struct SmartHistoryPeek {
    virtual_current: Option<super::FolderNavHistoryTarget>,
    previous: Option<super::FolderNavHistoryTarget>,
    original_back: Vec<super::FolderNavHistoryTarget>,
    original_forward: Vec<super::FolderNavHistoryTarget>,
    virtual_back: Vec<super::FolderNavHistoryTarget>,
    virtual_forward: Vec<super::FolderNavHistoryTarget>,
    quick_slot: Option<super::QuickFolderSlotId>,
}

impl SmartHistoryPeek {
    fn capture(
        app: &App,
        direction: SmartHistoryDirection,
        target: super::FolderNavHistoryTarget,
    ) -> Option<Self> {
        let (back, forward) = if let Some(workspace) = app.active_quick_folder_workspace() {
            (
                &workspace.history.back_stack,
                &workspace.history.forward_stack,
            )
        } else {
            (&app.folder_nav_back_stack, &app.folder_nav_forward_stack)
        };
        let mut peek = Self {
            virtual_current: app.folder_nav_current_target(),
            previous: app.folder_nav_current_target(),
            original_back: back.clone(),
            original_forward: forward.clone(),
            virtual_back: back.clone(),
            virtual_forward: forward.clone(),
            quick_slot: app.active_quick_folder_slot,
        };
        (peek.advance(direction).as_ref() == Some(&target)).then_some(peek)
    }

    fn advance(
        &mut self,
        direction: SmartHistoryDirection,
    ) -> Option<super::FolderNavHistoryTarget> {
        let (from, to) = match direction {
            SmartHistoryDirection::Back => (&mut self.virtual_back, &mut self.virtual_forward),
            SmartHistoryDirection::Forward => (&mut self.virtual_forward, &mut self.virtual_back),
        };
        let target = from.pop()?;
        if let Some(current) = &self.virtual_current
            && !App::folder_nav_targets_eq(current, &target)
        {
            App::push_folder_nav_stack(to, current.clone());
        }
        self.virtual_current = Some(target.clone());
        Some(target)
    }

    fn is_current(&self, app: &App) -> bool {
        if self.quick_slot != app.active_quick_folder_slot {
            return false;
        }
        let (back, forward) = if let Some(workspace) = app.active_quick_folder_workspace() {
            (
                &workspace.history.back_stack,
                &workspace.history.forward_stack,
            )
        } else {
            (&app.folder_nav_back_stack, &app.folder_nav_forward_stack)
        };
        back == &self.original_back
            && forward == &self.original_forward
            && app.folder_nav_current_target() == self.previous
    }

    fn commit(self, app: &mut App) {
        let (back, forward) = if let Some(workspace) = app.active_quick_folder_workspace_mut() {
            (
                &mut workspace.history.back_stack,
                &mut workspace.history.forward_stack,
            )
        } else {
            (
                &mut app.folder_nav_back_stack,
                &mut app.folder_nav_forward_stack,
            )
        };
        *back = self.virtual_back;
        *forward = self.virtual_forward;
    }
}

enum SmartTransitionIntent {
    Direct(super::top_level_grid_view::TopLevelGridRestore),
    Refresh,
    History(SmartHistoryPeek),
    Return { anchor: Option<PathBuf> },
    FolderNav(SmartFolderNavContinuation),
}

#[derive(Clone, Default)]
struct SmartPhysicalOpenEffects {
    suppress_rating_filter: bool,
    suppress_facet_filter: bool,
    select_after_load: Option<String>,
}

enum SmartPhysicalPreflight {
    Folder {
        path: PathBuf,
        cancel: Arc<AtomicBool>,
        rx: mpsc::Receiver<std::io::Result<ScannedDir>>,
    },
    Pdf {
        path: PathBuf,
        password: Option<String>,
        save_password: bool,
        handle: crate::pdf_loader::PdfEnumerateHandle,
        warm: Option<(u32, i64, u64)>,
    },
    PdfPassword {
        path: PathBuf,
        invalid_password: bool,
    },
    Zip {
        path: PathBuf,
        cancel: Arc<AtomicBool>,
        rx: mpsc::Receiver<Result<crate::zip_loader::ZipEnumeration, String>>,
    },
    ArchiveConvert,
}

enum SmartPhysicalReady {
    Folder(ScannedDir),
    PdfPages {
        pages: Vec<crate::pdf_loader::PdfPageEntry>,
        password: Option<String>,
        save_password: bool,
    },
    PdfWarm {
        page_count: u32,
        mtime: i64,
        file_size: u64,
        password: Option<String>,
        save_password: bool,
        handle: crate::pdf_loader::PdfEnumerateHandle,
    },
    Zip(crate::zip_loader::ZipEnumeration),
    ZipPrepared(super::PreparedZipGrid),
    Error(crate::empty_items_reason::EmptyItemsReason),
}

enum SmartPhysicalPoll {
    Waiting(SmartPhysicalPreflight),
    Ready(SmartPhysicalReady),
    PasswordRequired {
        path: PathBuf,
        invalid_password: bool,
    },
    Cancelled,
    Failed(String),
}

impl SmartPhysicalPreflight {
    fn poll(self) -> SmartPhysicalPoll {
        match self {
            Self::Folder { path, cancel, rx } => {
                if cancel.load(Ordering::Relaxed) {
                    return SmartPhysicalPoll::Cancelled;
                }
                match rx.try_recv() {
                    Ok(Ok(scanned)) => {
                        SmartPhysicalPoll::Ready(SmartPhysicalReady::Folder(scanned))
                    }
                    Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => {
                        SmartPhysicalPoll::Cancelled
                    }
                    Ok(Err(error)) => SmartPhysicalPoll::Failed(format!(
                        "フォルダを読み込めませんでした: {} ({error})",
                        path.display()
                    )),
                    Err(mpsc::TryRecvError::Empty) => {
                        SmartPhysicalPoll::Waiting(Self::Folder { path, cancel, rx })
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        SmartPhysicalPoll::Failed("フォルダの読み取りが中断されました".to_owned())
                    }
                }
            }
            Self::Pdf {
                path,
                password,
                save_password,
                handle,
                warm,
            } => {
                if handle.cancel.load(Ordering::Relaxed) {
                    return SmartPhysicalPoll::Cancelled;
                }
                if let Some((page_count, mtime, file_size)) = warm {
                    return SmartPhysicalPoll::Ready(SmartPhysicalReady::PdfWarm {
                        page_count,
                        mtime,
                        file_size,
                        password,
                        save_password,
                        handle,
                    });
                }
                match handle.rx.try_recv() {
                    Ok(Ok(pages)) => SmartPhysicalPoll::Ready(SmartPhysicalReady::PdfPages {
                        pages,
                        password,
                        save_password,
                    }),
                    Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => {
                        SmartPhysicalPoll::Cancelled
                    }
                    Ok(Err(error)) => {
                        let detail = error.to_string();
                        if detail.contains("Password") || detail.contains("password") {
                            SmartPhysicalPoll::PasswordRequired {
                                path,
                                invalid_password: password.is_some(),
                            }
                        } else {
                            SmartPhysicalPoll::Ready(SmartPhysicalReady::Error(
                                crate::empty_items_reason::EmptyItemsReason::PdfEnumerateFailed {
                                    detail,
                                },
                            ))
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => SmartPhysicalPoll::Waiting(Self::Pdf {
                        path,
                        password,
                        save_password,
                        handle,
                        warm: None,
                    }),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        SmartPhysicalPoll::Ready(SmartPhysicalReady::Error(
                            crate::empty_items_reason::EmptyItemsReason::PdfWorkerLost,
                        ))
                    }
                }
            }
            Self::Zip { path, cancel, rx } => {
                if cancel.load(Ordering::Relaxed) {
                    return SmartPhysicalPoll::Cancelled;
                }
                match rx.try_recv() {
                    Ok(Ok(enumeration)) => {
                        SmartPhysicalPoll::Ready(SmartPhysicalReady::Zip(enumeration))
                    }
                    Ok(Err(detail)) => SmartPhysicalPoll::Ready(SmartPhysicalReady::Error(
                        crate::empty_items_reason::EmptyItemsReason::ZipEnumerateFailed { detail },
                    )),
                    Err(mpsc::TryRecvError::Empty) => {
                        SmartPhysicalPoll::Waiting(Self::Zip { path, cancel, rx })
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        SmartPhysicalPoll::Ready(SmartPhysicalReady::Error(
                            crate::empty_items_reason::EmptyItemsReason::ZipWorkerLost,
                        ))
                    }
                }
            }
            Self::PdfPassword {
                path,
                invalid_password,
            } => SmartPhysicalPoll::Waiting(Self::PdfPassword {
                path,
                invalid_password,
            }),
            Self::ArchiveConvert => SmartPhysicalPoll::Waiting(Self::ArchiveConvert),
        }
    }
}

pub(crate) struct SmartFolderTransition {
    request_id: u64,
    source: SmartFolderSourceLease,
    intent: SmartTransitionIntent,
    target: SmartFolderTransitionTarget,
    progress: SmartFolderProgress,
    phase: SmartFolderTransitionPhase,
}

#[derive(Clone)]
struct SmartFolderNavContinuation {
    queued_steps: i32,
    mode: super::FolderNavMode,
    history_trigger: super::HistoryTrigger,
    restore_video_tile: bool,
}

impl SmartFolderTransition {
    fn cancel_work(&self) {
        match &self.phase {
            SmartFolderTransitionPhase::RootScan(pending) => pending.cancel(),
            SmartFolderTransitionPhase::RootCount(pending)
            | SmartFolderTransitionPhase::RootPrepare(pending) => pending.cancel(),
            SmartFolderTransitionPhase::RootConfirm(_)
            | SmartFolderTransitionPhase::RootReady(_)
            | SmartFolderTransitionPhase::ChildReady { .. }
            | SmartFolderTransitionPhase::Retired => {}
            SmartFolderTransitionPhase::ChildPreflight { child, .. } => match child {
                SmartPhysicalPreflight::Folder { cancel, .. }
                | SmartPhysicalPreflight::Zip { cancel, .. } => {
                    cancel.store(true, Ordering::Relaxed);
                }
                SmartPhysicalPreflight::Pdf { handle, .. } => handle.cancel(),
                SmartPhysicalPreflight::PdfPassword { .. }
                | SmartPhysicalPreflight::ArchiveConvert => {}
            },
        }
    }
}

impl Drop for SmartFolderTransition {
    fn drop(&mut self) {
        self.cancel_work();
        let phase = std::mem::replace(&mut self.phase, SmartFolderTransitionPhase::Retired);
        let retired: Option<RetiredSmartFolderPayload> = match phase {
            SmartFolderTransitionPhase::RootScan(pending) => Some(Box::new(pending)),
            SmartFolderTransitionPhase::RootCount(pending)
            | SmartFolderTransitionPhase::RootPrepare(pending) => Some(Box::new(pending)),
            SmartFolderTransitionPhase::RootConfirm(pending) => Some(Box::new(pending)),
            SmartFolderTransitionPhase::RootReady(prepared) => Some(prepared),
            SmartFolderTransitionPhase::ChildPreflight { root, child } => {
                Some(Box::new((root, child)))
            }
            SmartFolderTransitionPhase::ChildReady { root, child } => Some(Box::new((root, child))),
            SmartFolderTransitionPhase::Retired => None,
        };
        if let Some(retired) = retired {
            retire_smart_folder_session_payloads(vec![retired]);
        }
    }
}

impl App {
    fn release_staged_smart_nav_lock(&mut self, request_id: u64, intent: &SmartTransitionIntent) {
        if let SmartTransitionIntent::FolderNav(nav) = intent
            && matches!(
                &nav.mode,
                super::FolderNavMode::SmartFolder {
                    fullscreen: true,
                    ..
                }
            )
        {
            self.finish_smart_folder_navigation_sequence(request_id);
        }
    }

    pub(crate) fn staged_smart_child_preflight_from_visible_scope(&self) -> bool {
        self.smart_folder_transition
            .as_ref()
            .is_some_and(|transition| {
                self.smart_folder_source_lease().as_ref() == Some(&transition.source)
                    && matches!(
                        &transition.target,
                        SmartFolderTransitionTarget::Child { .. }
                    )
            })
    }

    fn handoff_staged_smart_folder_nav(&mut self, nav: SmartFolderNavContinuation) -> bool {
        let Some(transition) = self.smart_folder_transition.as_mut() else {
            return false;
        };
        if !matches!(
            &transition.target,
            SmartFolderTransitionTarget::Child { .. }
        ) {
            return false;
        }
        let request_id = transition.request_id;
        let fullscreen = matches!(
            &nav.mode,
            super::FolderNavMode::SmartFolder {
                fullscreen: true,
                ..
            }
        );
        transition.intent = SmartTransitionIntent::FolderNav(nav);
        if fullscreen {
            self.mark_smart_folder_navigation_sequence();
            self.bind_smart_folder_navigation_sequence(request_id);
        }
        true
    }

    fn retire_smart_folder_transition(&mut self) -> bool {
        let Some(transition) = self.smart_folder_transition.take() else {
            return false;
        };
        self.release_staged_smart_nav_lock(transition.request_id, &transition.intent);
        drop(transition);
        true
    }

    pub(crate) fn smart_folder_request_status(
        &self,
        definition_id: uuid::Uuid,
        request_id: u64,
    ) -> SmartFolderRequestStatus {
        if self
            .smart_folder_transition
            .as_ref()
            .is_some_and(|transition| {
                transition.request_id == request_id
                    && match &transition.target {
                        SmartFolderTransitionTarget::Root(state) => {
                            state.definition_id == definition_id
                        }
                        SmartFolderTransitionTarget::Child { state, .. } => {
                            state.definition_id == definition_id
                        }
                    }
            })
        {
            return SmartFolderRequestStatus::Pending;
        }
        if self
            .top_level_grid_view
            .smart_folder_session()
            .is_some_and(|session| {
                session.definition_id == definition_id
                    && session.adopted_request_id == Some(request_id)
            })
        {
            SmartFolderRequestStatus::Adopted
        } else {
            SmartFolderRequestStatus::Retired
        }
    }

    fn clear_smart_pdf_dialog_if_unclaimed(&mut self) {
        if self.pdf_password_request_pending_in_any_context() {
            return;
        }
        self.show_pdf_password_dialog = false;
        self.pdf_password_input.clear();
        self.pdf_password_error = None;
    }

    pub(crate) fn smart_pdf_password_dialog_path(&self) -> Option<PathBuf> {
        let transition = self.smart_folder_transition.as_ref()?;
        let SmartFolderTransitionPhase::ChildPreflight {
            child: SmartPhysicalPreflight::PdfPassword { path, .. },
            ..
        } = &transition.phase
        else {
            return None;
        };
        Some(path.clone())
    }

    pub(crate) fn retry_smart_pdf_password_request(
        &mut self,
        password: String,
        save_password: bool,
    ) -> bool {
        let Some(mut transition) = self.smart_folder_transition.take() else {
            return false;
        };
        let phase = std::mem::replace(&mut transition.phase, SmartFolderTransitionPhase::Retired);
        match phase {
            SmartFolderTransitionPhase::ChildPreflight {
                root,
                child: SmartPhysicalPreflight::PdfPassword { path, .. },
            } => {
                let handle = crate::pdf_loader::enumerate_pages_async(&path, Some(&password));
                transition.phase = SmartFolderTransitionPhase::ChildPreflight {
                    root,
                    child: SmartPhysicalPreflight::Pdf {
                        path,
                        password: Some(password),
                        save_password,
                        handle,
                        warm: None,
                    },
                };
                self.smart_folder_transition = Some(transition);
                true
            }
            phase => {
                transition.phase = phase;
                self.smart_folder_transition = Some(transition);
                false
            }
        }
    }

    pub(crate) fn cancel_smart_pdf_password_request(&mut self) -> bool {
        if self.smart_pdf_password_dialog_path().is_none() {
            return false;
        }
        self.retire_smart_folder_transition();
        self.clear_smart_pdf_dialog_if_unclaimed();
        self.reprepare_visible_smart_root_after_staged_terminal();
        true
    }

    /// An independently requested navigation supersedes an offscreen Smart open at the input
    /// boundary. Visible PDF/ZIP verification and the current Smart session remain owned by the
    /// old display until the new navigation itself is adopted.
    pub(crate) fn retire_staged_smart_navigation_for_independent_intent(&mut self) {
        if self.projected_viewer_context_id() != self.viewer_context_main() {
            return;
        }
        if self.retire_smart_folder_transition() {
            self.clear_smart_pdf_dialog_if_unclaimed();
        }
    }

    pub(crate) fn accumulate_staged_smart_folder_dfs_step(
        &mut self,
        forward: bool,
        mode: &super::FolderNavMode,
    ) -> bool {
        let source_current = self.smart_folder_source_lease();
        let Some(transition) = self.smart_folder_transition.as_mut() else {
            return false;
        };
        let SmartTransitionIntent::FolderNav(nav) = &mut transition.intent else {
            return false;
        };
        if source_current.as_ref() != Some(&transition.source)
            || !super::folder_nav_mode_same_kind(&nav.mode, mode)
        {
            return false;
        }
        let step = if forward { 1 } else { -1 };
        nav.queued_steps =
            (nav.queued_steps + step).clamp(-super::MAX_PENDING_NAV, super::MAX_PENDING_NAV);
        true
    }

    pub(crate) fn accumulate_staged_smart_fullscreen_ctrl_step(&mut self, forward: bool) -> bool {
        let source_current = self.smart_folder_source_lease();
        let mode = self
            .smart_folder_transition
            .as_ref()
            .and_then(|transition| match &transition.intent {
                SmartTransitionIntent::FolderNav(nav)
                    if source_current.as_ref() == Some(&transition.source)
                        && matches!(
                            &nav.mode,
                            super::FolderNavMode::SmartFolder {
                                fullscreen: true,
                                ..
                            }
                        ) =>
                {
                    Some(nav.mode.clone())
                }
                _ => None,
            });
        let Some(mode) = mode else {
            return false;
        };
        self.accumulate_staged_smart_folder_dfs_step(forward, &mode)
    }

    pub(crate) fn retire_staged_smart_fullscreen_nav_for_close(&mut self) {
        let fullscreen_nav = self
            .smart_folder_transition
            .as_ref()
            .is_some_and(|transition| {
                matches!(&transition.intent, SmartTransitionIntent::FolderNav(nav)
                if matches!(&nav.mode, super::FolderNavMode::SmartFolder { fullscreen: true, .. }))
            });
        if fullscreen_nav {
            self.retire_smart_folder_transition();
        }
    }

    /// Status for an offscreen request. The visible PDF/ZIP receiver is a different owner and
    /// keeps its own loading badge while this request prepares the next destination.
    pub(crate) fn staged_smart_loading_message(&self) -> Option<String> {
        let transition = self.smart_folder_transition.as_ref()?;
        let message = match &transition.phase {
            SmartFolderTransitionPhase::RootScan(_)
            | SmartFolderTransitionPhase::RootCount(_)
            | SmartFolderTransitionPhase::RootPrepare(_) => return None,
            SmartFolderTransitionPhase::ChildPreflight { child, .. } => match child {
                SmartPhysicalPreflight::Folder { .. } => "次のフォルダを読み込み中…".to_owned(),
                SmartPhysicalPreflight::Pdf { .. } => "次の PDF を読み込み中…".to_owned(),
                SmartPhysicalPreflight::PdfPassword { .. } => return None,
                SmartPhysicalPreflight::Zip { .. } => "次の ZIP を読み込み中…".to_owned(),
                SmartPhysicalPreflight::ArchiveConvert => "書庫を準備中…".to_owned(),
            },
            SmartFolderTransitionPhase::RootConfirm(_)
            | SmartFolderTransitionPhase::RootReady(_)
            | SmartFolderTransitionPhase::ChildReady { .. }
            | SmartFolderTransitionPhase::Retired => return None,
        };
        Some(message)
    }

    pub(crate) fn staged_smart_root_modal_visible(&self) -> bool {
        self.smart_folder_transition
            .as_ref()
            .is_some_and(|transition| {
                matches!(
                    transition.phase,
                    SmartFolderTransitionPhase::RootScan(_)
                        | SmartFolderTransitionPhase::RootCount(_)
                        | SmartFolderTransitionPhase::RootConfirm(_)
                        | SmartFolderTransitionPhase::RootPrepare(_)
                        | SmartFolderTransitionPhase::RootReady(_)
                )
            })
    }

    pub(crate) fn cancel_staged_smart_root_modal_request(&mut self, request_id: u64) -> bool {
        if !self.staged_smart_root_modal_visible()
            || !self
                .smart_folder_transition
                .as_ref()
                .is_some_and(|transition| transition.request_id == request_id)
        {
            return false;
        }
        self.retire_smart_folder_transition();
        self.reprepare_visible_smart_root_after_staged_terminal();
        self.show_feedback_toast("スマートフォルダ処理を中止しました".into());
        true
    }

    #[cfg(test)]
    pub(crate) fn replace_staged_pdf_enumeration_for_test(
        &mut self,
        path: &Path,
        result: std::io::Result<Vec<crate::pdf_loader::PdfPageEntry>>,
    ) -> bool {
        let Some(mut transition) = self.smart_folder_transition.take() else {
            return false;
        };
        let phase = std::mem::replace(&mut transition.phase, SmartFolderTransitionPhase::Retired);
        let replaced = match phase {
            SmartFolderTransitionPhase::ChildPreflight {
                root,
                child:
                    SmartPhysicalPreflight::Pdf {
                        path: pending_path,
                        password,
                        save_password,
                        handle,
                        ..
                    },
            } if crate::folder_tree::path_eq(&pending_path, path) => {
                handle.cancel();
                transition.phase = SmartFolderTransitionPhase::ChildPreflight {
                    root,
                    child: SmartPhysicalPreflight::Pdf {
                        path: pending_path.clone(),
                        password,
                        save_password,
                        handle: crate::pdf_loader::completed_enumerate_handle_for_test(
                            &pending_path,
                            result,
                        ),
                        warm: None,
                    },
                };
                true
            }
            phase => {
                transition.phase = phase;
                false
            }
        };
        self.smart_folder_transition = Some(transition);
        replaced
    }

    pub(super) fn smart_folder_source_lease(&self) -> Option<SmartFolderSourceLease> {
        let context_id = self.viewer_context_main();
        if self.projected_viewer_context_id() != context_id {
            return None;
        }
        use super::top_level_grid_view::{
            SmartFolderPosition, TopLevelGridSurface, TopLevelSearchView,
        };
        let source = match self.top_level_grid_view.surface() {
            TopLevelGridSurface::Folder => {
                SmartFolderSourceIdentity::Folder(self.current_folder.clone())
            }
            TopLevelGridSurface::Search(view) => {
                let (query, executed) = match view {
                    TopLevelSearchView::Global => (
                        self.global_search.query.clone(),
                        self.global_search.last_executed.clone(),
                    ),
                    TopLevelSearchView::Favorite => (
                        self.favsearch.query.clone(),
                        self.favsearch.last_executed.clone(),
                    ),
                    TopLevelSearchView::Tag => (
                        self.tag_view.query.clone(),
                        self.tag_view.last_executed.clone(),
                    ),
                };
                SmartFolderSourceIdentity::Search {
                    view: *view,
                    query,
                    executed,
                    location: self.current_folder.clone(),
                }
            }
            TopLevelGridSurface::SmartFolder(state) => {
                let location = match &state.position {
                    SmartFolderPosition::Root => None,
                    SmartFolderPosition::Container { current, .. }
                    | SmartFolderPosition::Scoped { current, .. } => Some(current.clone()),
                };
                SmartFolderSourceIdentity::Smart {
                    definition_id: state.definition_id,
                    location,
                }
            }
            TopLevelGridSurface::Collection(identity) => {
                let session = self.top_level_grid_view.collection_session()?;
                SmartFolderSourceIdentity::Collection {
                    identity: *identity,
                    position: session.position.clone(),
                }
            }
            other => SmartFolderSourceIdentity::Other(other.clone()),
        };
        Some(SmartFolderSourceLease {
            context_id,
            surface_generation: self.top_level_grid_view.generation(),
            quick_slot: self.active_quick_folder_slot,
            source,
        })
    }

    fn begin_smart_folder_transition_root(
        &mut self,
        definition: crate::settings::SmartFolderDefinition,
        refresh: bool,
        intent: SmartTransitionIntent,
        target: SmartFolderTransitionTarget,
    ) -> Result<u64, String> {
        let source = self
            .smart_folder_source_lease()
            .ok_or_else(|| "メインの表示状態を確認できません".to_owned())?;
        let mut request_id = self.smart_folder_transition_sequence.wrapping_add(1);
        if request_id == 0 {
            request_id = 1;
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
        let tombstones = self
            .smart_folder_removed_paths
            .get(&definition.id)
            .cloned()
            .unwrap_or_default();
        let pending = spawn_smart_folder_scan(
            definition,
            request_id,
            refresh,
            tombstones,
            SmartFolderScanOptions::from(&self.settings),
            io_sem,
            Arc::clone(&self.activity_gate),
        )?;
        if self.smart_pdf_password_dialog_path().is_some() {
            self.clear_smart_pdf_dialog_if_unclaimed();
        }
        self.smart_folder_transition_sequence = request_id;
        self.retire_smart_folder_transition();
        self.smart_folder_transition = Some(SmartFolderTransition {
            request_id,
            source,
            intent,
            target,
            progress: SmartFolderProgress::default(),
            phase: SmartFolderTransitionPhase::RootScan(pending),
        });
        Ok(request_id)
    }

    /// Rebuild a return target from the settled session's root facts without publishing an
    /// intermediate root or repeating the source scan. The old child remains the visible owner
    /// until the worker's prepared root is adopted.
    fn begin_staged_smart_root_reprepare(
        &mut self,
        mut snapshot: SmartFolderSnapshot,
        definition: &crate::settings::SmartFolderDefinition,
        state: super::top_level_grid_view::SmartFolderViewState,
        intent: SmartTransitionIntent,
    ) -> Result<u64, String> {
        let source = self
            .smart_folder_source_lease()
            .ok_or_else(|| "メインの表示状態を確認できません".to_owned())?;
        let mut request_id = self.smart_folder_transition_sequence.wrapping_add(1);
        if request_id == 0 {
            request_id = 1;
        }
        adopt_smart_folder_presentation(&mut snapshot.definition, definition);
        let mut transition = SmartFolderTransition {
            request_id,
            source,
            intent,
            target: SmartFolderTransitionTarget::Root(state),
            progress: SmartFolderProgress::default(),
            phase: SmartFolderTransitionPhase::Retired,
        };
        let pending = self.spawn_smart_transition_prepare(
            &transition,
            snapshot,
            false,
            HashSet::new(),
            None,
        )?;
        transition.phase = SmartFolderTransitionPhase::RootPrepare(pending);
        self.smart_folder_transition_sequence = request_id;
        self.retire_smart_folder_transition();
        self.smart_folder_transition = Some(transition);
        Ok(request_id)
    }

    fn smart_physical_target_state(
        &self,
        path: &Path,
    ) -> Option<super::top_level_grid_view::SmartFolderViewState> {
        self.top_level_grid_view.smart_folder_session()?;
        let mut state = self.top_level_grid_view.smart_folder()?.clone();
        let accepted = if state.contains_scoped_path(path) {
            state.move_to(path)
        } else {
            state.enter_containing_path(path)
        };
        if !accepted {
            return None;
        }
        Some(state)
    }

    /// Classify an admitted Smart destination without another UI-thread filesystem probe. The
    /// current row supplies a nested virtual child's kind; history uses its adopted typed kind
    /// after that row's source session has gone away.
    pub(crate) fn smart_physical_target_kind(&self, path: &Path) -> Option<SmartChildKind> {
        use super::top_level_grid_view::SmartFolderPosition;
        let state = self.smart_physical_target_state(path)?;
        if let Some(kind) = self.items.iter().find_map(|item| {
            item.container_path()
                .filter(|visible| crate::folder_tree::path_eq(visible, path))
                .and_then(|_| match item {
                    GridItem::Folder(_) => Some(SmartChildKind::Folder),
                    GridItem::PdfFile(_) => Some(SmartChildKind::Pdf),
                    GridItem::ZipFile(_) => Some(SmartChildKind::Zip),
                    GridItem::ConvertibleArchive { .. } => Some(SmartChildKind::ConvertibleArchive),
                    _ => None,
                })
        }) {
            return Some(kind);
        }
        match state.position {
            SmartFolderPosition::Scoped { current_kind, .. } => Some(current_kind),
            SmartFolderPosition::Container { root_entry, .. } => state
                .navigation_entries
                .iter()
                .find(|entry| crate::folder_tree::path_eq(&entry.logical_path, &root_entry))
                .map(|entry| entry.kind),
            SmartFolderPosition::Root => None,
        }
    }

    /// Stage one resident Smart physical navigation without taking its current grid, workers,
    /// or pending PDF/ZIP verification. A ready pre-scan is transferred rather than repeated.
    pub(crate) fn begin_smart_physical_navigation(
        &mut self,
        path: PathBuf,
        kind: SmartChildKind,
        auto_fullscreen: bool,
        pre_scan: Option<ScannedDir>,
        source_index: Option<usize>,
    ) -> Result<(), Option<ScannedDir>> {
        let Some(mut state) = self.smart_physical_target_state(&path) else {
            return Err(pre_scan);
        };
        if let super::top_level_grid_view::SmartFolderPosition::Scoped { current_kind, .. } =
            &mut state.position
        {
            *current_kind = kind;
        }
        let Some(source_lease) = self.smart_folder_source_lease() else {
            return Err(pre_scan);
        };
        if pre_scan.is_some() && kind != SmartChildKind::Folder {
            return Err(pre_scan);
        }
        let source = SmartChildSource {
            logical_source: path.clone(),
            load_path: path,
        };
        let mut effects = source_index
            .filter(|&index| {
                self.items.get(index).and_then(GridItem::container_path)
                    == Some(source.logical_source.as_path())
            })
            .map(|index| {
                let suppress_rating_filter =
                    self.rating_filter_suppressed_at.is_none() && self.rating_filter_active() && {
                        let stars = self.get_rating(index);
                        (1..=5).contains(&stars)
                            && self.items.get(index).is_some_and(|item| {
                                super::passes_rating_filter(
                                    item,
                                    stars,
                                    &self.settings.rating_filter,
                                )
                            })
                    };
                SmartPhysicalOpenEffects {
                    suppress_rating_filter,
                    suppress_facet_filter: self.facet_filter_active()
                        && self.passes_facet_filter(index, None),
                    select_after_load: None,
                }
            })
            .unwrap_or_default();
        if kind == SmartChildKind::Folder {
            effects.select_after_load = self.effective_folder().and_then(|current| {
                current
                    .parent()
                    .filter(|parent| crate::folder_tree::path_eq(parent, &source.logical_source))
                    .and_then(|_| current.file_name())
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            });
        }
        let mut request_id = self.smart_folder_transition_sequence.wrapping_add(1);
        if request_id == 0 {
            request_id = 1;
        }
        let phase = if let Some(scan) = pre_scan {
            SmartFolderTransitionPhase::ChildReady {
                root: SmartFolderTransitionRoot::Resident,
                child: SmartPhysicalReady::Folder(scan),
            }
        } else {
            let Ok(child) = self.begin_smart_child_preflight(&source, kind) else {
                return Err(None);
            };
            SmartFolderTransitionPhase::ChildPreflight {
                root: SmartFolderTransitionRoot::Resident,
                child,
            }
        };
        if self.smart_pdf_password_dialog_path().is_some() {
            self.clear_smart_pdf_dialog_if_unclaimed();
        }
        self.smart_folder_transition_sequence = request_id;
        self.retire_smart_folder_transition();
        self.smart_folder_transition = Some(SmartFolderTransition {
            request_id,
            source: source_lease,
            intent: SmartTransitionIntent::Direct(
                self.current_top_level_restore_snapshot()
                    .unwrap_or(super::top_level_grid_view::TopLevelGridRestore::Unavailable),
            ),
            target: SmartFolderTransitionTarget::Child {
                state,
                source,
                kind,
                auto_fullscreen,
                effects,
                archive_commit: None,
            },
            progress: SmartFolderProgress::default(),
            phase,
        });
        // Archive conversion is a separate dialog/worker owner. Retire an older dialog only
        // after this request is admitted; its exact request ID prevents the old cancel callback
        // from retiring the newly staged Smart open, including a same-source re-open.
        if self.archive_convert.is_some() {
            self.cancel_archive_convert_for_navigation("staged_smart_navigation_replaced_archive");
        }
        Ok(())
    }

    /// A Ctrl folder traversal is an input intent, not an already adopted folder. Keep its
    /// fullscreen reopen and queued DFS steps with the offscreen request until visible commit.
    pub(crate) fn begin_smart_folder_dfs_navigation(
        &mut self,
        path: PathBuf,
        kind: SmartChildKind,
        pre_scan: Option<ScannedDir>,
        queued_steps: i32,
        mode: super::FolderNavMode,
        history_trigger: super::HistoryTrigger,
        restore_video_tile: bool,
    ) -> Result<(), Option<ScannedDir>> {
        self.begin_smart_physical_navigation(path, kind, false, pre_scan, None)?;
        if let Some(transition) = self.smart_folder_transition.as_mut() {
            let request_id = transition.request_id;
            let fullscreen = matches!(
                &mode,
                super::FolderNavMode::SmartFolder {
                    fullscreen: true,
                    ..
                }
            );
            transition.intent = SmartTransitionIntent::FolderNav(SmartFolderNavContinuation {
                queued_steps,
                mode,
                history_trigger,
                restore_video_tile,
            });
            if fullscreen {
                self.bind_smart_folder_navigation_sequence(request_id);
            }
        }
        Ok(())
    }

    /// A DFS result belongs to the exact prepared root order it captured. Visible PDF page
    /// verification may advance items_generation without changing this order, so that counter
    /// is intentionally absent from the check.
    pub(crate) fn smart_folder_navigation_target_current(
        &self,
        state: &super::top_level_grid_view::SmartFolderViewState,
        path: &Path,
    ) -> bool {
        self.top_level_grid_view
            .smart_folder()
            .is_some_and(|current| {
                current.definition_id == state.definition_id
                    && Arc::ptr_eq(&current.navigation_entries, &state.navigation_entries)
            })
            && !self.smart_folder_navigation_target_removed(state.definition_id, path)
    }

    fn smart_folder_navigation_target_removed(
        &self,
        definition_id: uuid::Uuid,
        path: &Path,
    ) -> bool {
        self.smart_folder_removed_paths
            .get(&definition_id)
            .is_some_and(|removed| {
                smart_folder_path_is_removed(&crate::path_key::normalize_keep_drive(path), removed)
            })
    }

    /// The grid's Smart container opens share one staged owner. Ordinary, collection, search,
    /// and detached opens keep their existing navigation path when no Smart session is mounted.
    pub(crate) fn begin_smart_grid_container_navigation(
        &mut self,
        index: usize,
        path: PathBuf,
        auto_fullscreen: bool,
    ) -> bool {
        if self.top_level_grid_view.smart_folder_session().is_none() {
            return false;
        }
        let kind = match self.items.get(index) {
            Some(GridItem::Folder(_)) => SmartChildKind::Folder,
            Some(GridItem::PdfFile(_)) => SmartChildKind::Pdf,
            Some(GridItem::ZipFile(_)) => SmartChildKind::Zip,
            Some(GridItem::ConvertibleArchive { .. }) => SmartChildKind::ConvertibleArchive,
            _ => return false,
        };
        if kind == SmartChildKind::ConvertibleArchive
            && self.settings.archive_file_handling_ignores_convertible()
        {
            self.show_feedback_toast("設定により RAR / 7z / LZH アーカイブを無視しています".into());
            return true;
        }
        if self
            .begin_smart_physical_navigation(path.clone(), kind, auto_fullscreen, None, Some(index))
            .is_err()
        {
            self.show_feedback_toast("コンテナの読み取りを開始できませんでした".into());
            return true;
        }
        if kind == SmartChildKind::ConvertibleArchive {
            self.start_smart_archive_conversion_current();
        }
        true
    }

    /// Conversion belongs to the staged request, including a history target whose root has
    /// only been prepared offscreen and therefore has no visible source item index.
    fn start_smart_archive_conversion_current(&mut self) {
        let Some(transition) = self.smart_folder_transition.as_ref() else {
            return;
        };
        let SmartFolderTransitionTarget::Child {
            source,
            kind: SmartChildKind::ConvertibleArchive,
            auto_fullscreen,
            ..
        } = &transition.target
        else {
            return;
        };
        let path = source.logical_source.clone();
        let auto_fullscreen = *auto_fullscreen;
        let owner =
            super::OpenRequestOwner::MainGridArchive(super::MainGridArchiveTransitionIntent {
                source_path: path.clone(),
                reading_history_return_from: None,
                suppress_rating_filter: false,
                suppress_facet_filter: false,
                smart_folder_owner: super::SmartGridArchiveOwner::Transition(transition.request_id),
                collection_grid_owner: None,
                collection_navigation_continuation: None,
            });
        let format = path
            .extension()
            .and_then(|extension| extension.to_str())
            .and_then(crate::archive_converter::ArchiveFormat::from_extension);
        let started = match format {
            Some(crate::archive_converter::ArchiveFormat::Rar) => {
                let fallback = self.try_archive_cache_lookup(&path);
                self.request_rar_open_owned(path, auto_fullscreen, fallback, owner)
            }
            Some(format) => {
                if let Some(cached) = self.try_archive_cache_lookup(&path) {
                    self.supply_smart_archive_load_alias(&path, &cached, &owner)
                } else {
                    self.request_archive_convert_owned(path, format, auto_fullscreen, owner)
                }
            }
            None => false,
        };
        if !started {
            self.retire_smart_folder_transition();
            self.reprepare_visible_smart_root_after_staged_terminal();
            self.show_feedback_toast("アーカイブの読み取りを開始できませんでした".into());
        }
    }

    pub(crate) fn smart_grid_archive_owner_for_source(
        &self,
        path: &Path,
    ) -> super::SmartGridArchiveOwner {
        if let Some(transition) = self.smart_folder_transition.as_ref()
            && self.smart_folder_source_lease().as_ref() == Some(&transition.source)
            && matches!(
                &transition.target,
                SmartFolderTransitionTarget::Child { source, kind: SmartChildKind::ConvertibleArchive, .. }
                    if crate::folder_tree::path_eq(&source.logical_source, path)
            )
        {
            return super::SmartGridArchiveOwner::Transition(transition.request_id);
        }
        if self.top_level_grid_view.smart_folder_session().is_some() {
            super::SmartGridArchiveOwner::UnclaimedSmart
        } else {
            super::SmartGridArchiveOwner::None
        }
    }

    pub(crate) fn smart_folder_transition_request_is_current(
        &self,
        request_id: u64,
        source_path: &Path,
    ) -> bool {
        self.smart_folder_transition.as_ref().is_some_and(|transition| {
            transition.request_id == request_id
                && self.smart_folder_source_lease().as_ref() == Some(&transition.source)
                && matches!(
                    &transition.target,
                    SmartFolderTransitionTarget::Child { source, kind: SmartChildKind::ConvertibleArchive, .. }
                        if crate::folder_tree::path_eq(&source.logical_source, source_path)
                )
        })
    }

    /// A converted cache path (or directly readable RAR) is a load alias for the original
    /// Smart row. It joins the same offscreen request instead of entering the ordinary loader.
    pub(crate) fn supply_smart_archive_load_alias(
        &mut self,
        source_path: &Path,
        load_path: &Path,
        owner: &super::OpenRequestOwner,
    ) -> bool {
        let super::OpenRequestOwner::MainGridArchive(intent) = owner else {
            return false;
        };
        let super::SmartGridArchiveOwner::Transition(request_id) = intent.smart_folder_owner else {
            return false;
        };
        if !self.smart_folder_transition_request_is_current(request_id, source_path) {
            return false;
        }
        let Some(mut transition) = self.smart_folder_transition.take() else {
            return false;
        };
        let phase = std::mem::replace(&mut transition.phase, SmartFolderTransitionPhase::Retired);
        let SmartFolderTransitionPhase::ChildPreflight {
            root,
            child: SmartPhysicalPreflight::ArchiveConvert,
        } = phase
        else {
            transition.phase = phase;
            self.smart_folder_transition = Some(transition);
            return false;
        };
        let load_source = SmartChildSource {
            logical_source: source_path.to_path_buf(),
            load_path: load_path.to_path_buf(),
        };
        let child = match self.begin_smart_child_preflight(&load_source, SmartChildKind::Zip) {
            Ok(child) => child,
            Err(message) => {
                // The alias could not acquire its ZIP worker. This request is terminal;
                // retaining an ArchiveConvert phase would wait for a callback that is done.
                transition.phase = SmartFolderTransitionPhase::ChildPreflight {
                    root,
                    child: SmartPhysicalPreflight::ArchiveConvert,
                };
                drop(transition);
                self.show_feedback_toast(message);
                self.reprepare_visible_smart_root_after_staged_terminal();
                return false;
            }
        };
        let restore_reading_history = self
            .reading_history_return_from
            .as_ref()
            .is_some_and(|from| crate::folder_tree::path_eq(from, source_path));
        let restore_bookmark_view = self
            .bookmark_view_state
            .as_ref()
            .filter(|state| {
                state
                    .target()
                    .is_some_and(|target| target.matches_loaded_container(source_path))
            })
            .cloned();
        if let SmartFolderTransitionTarget::Child {
            source,
            kind,
            archive_commit,
            ..
        } = &mut transition.target
        {
            *source = load_source;
            *kind = SmartChildKind::Zip;
            *archive_commit = Some(super::SmartFolderArchiveCommit {
                owner: owner.clone(),
                source_path: source_path.to_path_buf(),
                cache_path: load_path.to_path_buf(),
                restore_reading_history,
                restore_bookmark_view,
            });
        }
        transition.phase = SmartFolderTransitionPhase::ChildPreflight { root, child };
        self.smart_folder_transition = Some(transition);
        true
    }

    /// A Smart history target is only peeked here. The visible source and both stacks remain
    /// owned by the current view until the prepared root or child is actually adopted.
    pub(crate) fn begin_smart_history_navigation(
        &mut self,
        state: super::top_level_grid_view::SmartFolderViewState,
        direction: SmartHistoryDirection,
    ) -> bool {
        let target = super::FolderNavHistoryTarget::SmartFolder(state.clone());
        let Some(peek) = SmartHistoryPeek::capture(self, direction, target) else {
            return false;
        };
        self.begin_staged_smart_target_navigation(state, SmartTransitionIntent::History(peek))
    }

    /// Back/Forward while a Smart history destination is still preparing advances a virtual
    /// cursor, not the live stacks. A reverse step to the still-visible origin simply cancels
    /// the request. A later non-Smart target hands its final cursor to the ordinary dispatcher.
    pub(crate) fn advance_staged_smart_history(
        &mut self,
        direction: SmartHistoryDirection,
    ) -> Option<StagedSmartHistoryAction> {
        let SmartTransitionIntent::History(peek) = &self.smart_folder_transition.as_ref()?.intent
        else {
            return None;
        };
        if !peek.is_current(self) {
            self.retire_smart_folder_transition();
            self.reprepare_visible_smart_root_after_staged_terminal();
            return Some(StagedSmartHistoryAction::Handled);
        }
        let mut next = peek.clone();
        let Some(target) = next.advance(direction) else {
            return Some(StagedSmartHistoryAction::Handled);
        };
        self.retire_smart_folder_transition();
        if next.virtual_current == next.previous {
            self.reprepare_visible_smart_root_after_staged_terminal();
            return Some(StagedSmartHistoryAction::Handled);
        }
        match target {
            super::FolderNavHistoryTarget::SmartFolder(state) => {
                let _ = self.begin_staged_smart_target_navigation(
                    state,
                    SmartTransitionIntent::History(next),
                );
                Some(StagedSmartHistoryAction::Handled)
            }
            target => {
                let rollback = self.folder_nav_history_snapshot();
                next.commit(self);
                self.set_active_folder_nav_suppress_record_once(true);
                Some(StagedSmartHistoryAction::Dispatch { target, rollback })
            }
        }
    }

    /// Search remains the visible owner until the saved Smart destination is adopted.
    pub(crate) fn begin_smart_search_return_navigation(
        &mut self,
        state: super::top_level_grid_view::SmartFolderViewState,
    ) -> bool {
        self.begin_staged_smart_target_navigation(
            state,
            SmartTransitionIntent::Return { anchor: None },
        )
    }

    fn resident_smart_root_restore_is_exact(
        &self,
        definition: &crate::settings::SmartFolderDefinition,
    ) -> bool {
        let Some(session) = self.top_level_grid_view.smart_folder_session() else {
            return false;
        };
        if session.definition_id != definition.id
            || session.metadata_revision != self.smart_folder_metadata_revision
            || session.presentation
                != SmartFolderPresentation::for_location(
                    self,
                    &smart_folder_synthetic_path(definition.id),
                    definition.grouping,
                )
        {
            return false;
        }
        match session.phase.visible() {
            SmartFolderOpenPhase::Child {
                parked_root: SmartFolderRootPayload::Visible(_),
                ..
            } => session.root_snapshot().is_some_and(|snapshot| {
                smart_folder_prepared_definition_matches(&snapshot.definition, definition)
            }),
            SmartFolderOpenPhase::Child {
                parked_root: SmartFolderRootPayload::Offscreen(prepared),
                ..
            } => {
                let removed_since_scan = self
                    .smart_folder_removed_paths
                    .get(&definition.id)
                    .map(|removed| {
                        smart_folder_tombstones_after_scan_start(
                            removed,
                            &prepared.authoritative_ignored_tombstones,
                        )
                    })
                    .unwrap_or_default();
                prepared.metadata_revision == self.smart_folder_metadata_revision
                    && smart_folder_prepared_definition_matches(
                        &prepared.snapshot.definition,
                        definition,
                    )
                    && removed_since_scan.is_subset(&prepared.applied_tombstones)
            }
            SmartFolderOpenPhase::Root => false,
        }
    }

    fn begin_staged_smart_target_navigation(
        &mut self,
        state: super::top_level_grid_view::SmartFolderViewState,
        intent: SmartTransitionIntent,
    ) -> bool {
        use super::top_level_grid_view::SmartFolderPosition;
        let Some(definition) = self
            .settings
            .smart_folders
            .iter()
            .find(|definition| definition.id == state.definition_id)
            .cloned()
        else {
            return false;
        };
        let child = match &state.position {
            SmartFolderPosition::Root => None,
            SmartFolderPosition::Scoped {
                current,
                current_kind,
                ..
            } => Some((
                SmartChildSource {
                    logical_source: current.clone(),
                    load_path: current.clone(),
                },
                *current_kind,
            )),
            SmartFolderPosition::Container {
                root_entry,
                current,
            } => {
                let extension = root_entry
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let kind = match classify_entry_kind(&extension, true) {
                    Some(SmartFolderEntryKind::Pdf) => SmartChildKind::Pdf,
                    Some(SmartFolderEntryKind::Zip) => SmartChildKind::Zip,
                    Some(SmartFolderEntryKind::Archive) => SmartChildKind::ConvertibleArchive,
                    _ => return false,
                };
                Some((
                    SmartChildSource {
                        logical_source: root_entry.clone(),
                        load_path: current.clone(),
                    },
                    kind,
                ))
            }
        };
        if child.is_none()
            && matches!(intent, SmartTransitionIntent::History(_))
            && self.resident_smart_root_restore_is_exact(&definition)
        {
            let synthetic = smart_folder_synthetic_path(state.definition_id);
            self.set_active_folder_nav_suppress_record_once(true);
            let restored = self.restore_smart_folder_for_synthetic_path(&synthetic);
            self.set_active_folder_nav_suppress_record_once(false);
            if restored {
                if let SmartTransitionIntent::History(peek) = intent {
                    peek.commit(self);
                }
            }
            return restored;
        }
        if child.is_none() {
            let reusable_snapshot = self
                .top_level_grid_view
                .smart_folder_session()
                .filter(|session| session.definition_id == definition.id)
                .and_then(SmartFolderSession::root_snapshot)
                .filter(|snapshot| smart_folder_scan_rules_match(&snapshot.definition, &definition))
                .cloned();
            if let Some(snapshot) = reusable_snapshot {
                return match self.begin_staged_smart_root_reprepare(
                    snapshot,
                    &definition,
                    state,
                    intent,
                ) {
                    Ok(_) => true,
                    Err(message) => {
                        self.show_feedback_toast(message);
                        false
                    }
                };
            }
        }
        let target = match child {
            Some((source, kind)) => SmartFolderTransitionTarget::Child {
                state,
                source,
                kind,
                auto_fullscreen: false,
                effects: SmartPhysicalOpenEffects::default(),
                archive_commit: None,
            },
            None => SmartFolderTransitionTarget::Root(state),
        };
        match self.begin_smart_folder_transition_root(definition, false, intent, target) {
            Ok(_) => true,
            Err(message) => {
                self.show_feedback_toast(message);
                false
            }
        }
    }

    fn adopt_smart_transition_root(
        &mut self,
        request_id: u64,
        prepared: PreparedSmartFolder,
        record_history: bool,
        anchor: Option<PathBuf>,
    ) -> bool {
        let definition_id = prepared.snapshot.definition.id;
        // All fallible work is complete. This is the first point at which the prior viewer may
        // be dismissed or its workers cancelled. In particular, Search and Collection remain
        // live while the new Smart root is scanning and preparing offscreen.
        if self.smart_folder_local_search_reapply.is_some() && self.show_search_bar {
            self.smart_folder_local_search_reapply = Some(self.search_query.clone());
        }
        let prior = self.close_transient_views_before_smart_folder();
        if self.smart_folder_pending.is_some()
            || self.smart_folder_prepare_pending.is_some()
            || self.smart_folder_confirm_pending.is_some()
        {
            self.cancel_smart_folder_pending();
        }
        if !self.items_are_smart_folder_view {
            self.smart_folder_saved_folder = self
                .effective_folder()
                .filter(|path| !is_synthetic_view_path(path));
        }
        self.smart_folder_open_origin = Some(if record_history {
            SmartFolderOpenOrigin::Direct(prior.clone())
        } else {
            SmartFolderOpenOrigin::ReturnReprepare {
                prior: prior.clone(),
                anchor,
            }
        });
        self.smart_folder_generation = self.top_level_grid_view.begin(
            super::top_level_grid_view::TopLevelGridSurface::SmartFolder(
                super::top_level_grid_view::SmartFolderViewState::root(definition_id, Vec::new()),
            ),
            Some(prior),
        );
        let installed = self.install_prepared_smart_folder(prepared);
        if installed && let Some(session) = self.top_level_grid_view.smart_folder_session_mut() {
            session.adopted_request_id = Some(request_id);
        }
        installed
    }

    fn adopt_smart_child_session(
        &mut self,
        request_id: u64,
        root: SmartFolderTransitionRoot,
        mut target: super::top_level_grid_view::SmartFolderViewState,
        source: &SmartChildSource,
    ) -> bool {
        let definition_id = target.definition_id;
        match root {
            SmartFolderTransitionRoot::Offscreen(prepared) => {
                if prepared.snapshot.definition.id != definition_id {
                    self.retire_smart_folder_payloads([prepared as RetiredSmartFolderPayload]);
                    return false;
                }
                let navigation_entries = smart_root_navigation_entries(&prepared.items);
                if !target.refresh_navigation_entries(navigation_entries) {
                    self.retire_smart_folder_payloads([prepared as RetiredSmartFolderPayload]);
                    return false;
                }
                let root_entry = match &target.position {
                    super::top_level_grid_view::SmartFolderPosition::Container {
                        root_entry,
                        ..
                    } => Some(root_entry.as_path()),
                    super::top_level_grid_view::SmartFolderPosition::Scoped {
                        entry_root, ..
                    } => Some(entry_root.as_path()),
                    super::top_level_grid_view::SmartFolderPosition::Root => None,
                };
                if !root_entry.is_some_and(|root_entry| {
                    prepared.items.iter().any(|item| {
                        item.drag_source_path()
                            .is_some_and(|path| crate::folder_tree::path_eq(path, root_entry))
                    })
                }) {
                    self.retire_smart_folder_payloads([prepared as RetiredSmartFolderPayload]);
                    return false;
                }
                let prior = self.close_transient_views_before_smart_folder();
                if self.smart_folder_pending.is_some()
                    || self.smart_folder_prepare_pending.is_some()
                    || self.smart_folder_confirm_pending.is_some()
                {
                    self.cancel_smart_folder_pending();
                }
                self.smart_folder_saved_folder = self
                    .effective_folder()
                    .filter(|path| !is_synthetic_view_path(path));
                self.smart_folder_generation = self.top_level_grid_view.begin(
                    super::top_level_grid_view::TopLevelGridSurface::SmartFolder(target),
                    Some(prior),
                );
                self.top_level_grid_view
                    .install_smart_folder_session(SmartFolderSession {
                        definition_id,
                        snapshot: None,
                        resort_metadata: None,
                        presentation: prepared.presentation.clone(),
                        metadata_revision: prepared.metadata_revision,
                        adopted_request_id: Some(request_id),
                        phase: SmartFolderOpenPhase::Child {
                            logical_path: source.logical_source.clone(),
                            parked_root: SmartFolderRootPayload::Offscreen(prepared),
                        },
                    });
            }
            SmartFolderTransitionRoot::Resident => {
                let Some(mut session) = self.top_level_grid_view.take_smart_folder_session() else {
                    return false;
                };
                if session.definition_id != definition_id {
                    self.top_level_grid_view
                        .install_smart_folder_session(session);
                    return false;
                }
                let previous = std::mem::replace(&mut session.phase, SmartFolderOpenPhase::Root);
                let parked_root = match previous {
                    SmartFolderOpenPhase::Child { parked_root, .. } => parked_root,
                    SmartFolderOpenPhase::Root => {
                        SmartFolderRootPayload::Visible(self.take_visible_smart_folder_grid())
                    }
                };
                self.record_smart_folder_scope_transition(&target);
                self.top_level_grid_view.replace_surface(
                    super::top_level_grid_view::TopLevelGridSurface::SmartFolder(target),
                );
                session.phase = SmartFolderOpenPhase::Child {
                    logical_path: source.logical_source.clone(),
                    parked_root,
                };
                session.adopted_request_id = Some(request_id);
                self.top_level_grid_view
                    .install_smart_folder_session(session);
            }
        }
        self.current_smart_folder_id = Some(definition_id);
        self.items_are_smart_folder_view = false;
        if let Some(top) = self.facet_filter_suppression_stack.last_mut() {
            top.anchor = source.logical_source.clone();
        }
        if let Some((anchor, _)) = self.rating_filter_suppressed_at.as_mut() {
            *anchor = source.logical_source.clone();
        }
        true
    }

    /// Install an already-scanned Smart Folder child. The caller has checked every fallible
    /// admission and published the Smart session, so this tail only retires the old physical
    /// context and materializes the supplied scan once.
    fn install_smart_scanned_folder(
        &mut self,
        path: PathBuf,
        scan: ScannedDir,
        authority: super::VisibleInstallAuthority<'_>,
    ) -> bool {
        let started = std::time::Instant::now();
        let folder_changes = self
            .current_folder
            .as_ref()
            .map(|current| !crate::folder_tree::path_eq(current, &path))
            .unwrap_or(true);
        if folder_changes {
            self.stack_mode_requested = false;
            self.clear_archive_convert_nav_history_rollback();
            crate::thumb_loader::bump_catchup_epoch();
            let _ = crate::pdf_loader::bump_render_context_epoch();
        }
        self.cancel_folder_pane_open();
        self.clear_meta_undo();
        crate::zip_loader::clear_nested_cache();
        self.zip_nav = None;
        self.install_scanned_folder_listing(
            path.clone(),
            scan,
            authority,
            super::FolderListingMetrics {
                seq: self.input_seq,
                started,
                scan_started: started,
                pre_scanned: true,
                path_display: if crate::perf::is_enabled() {
                    path.display().to_string()
                } else {
                    String::new()
                },
            },
        )
    }

    fn adopt_smart_child_ready(
        &mut self,
        request_id: u64,
        root: SmartFolderTransitionRoot,
        target: super::top_level_grid_view::SmartFolderViewState,
        source: SmartChildSource,
        child: SmartPhysicalReady,
        auto_fullscreen: bool,
        effects: SmartPhysicalOpenEffects,
    ) -> bool {
        let definition_id = target.definition_id;
        let path = source.load_path.clone();
        if self.smart_folder_navigation_target_removed(definition_id, &source.logical_source)
            || (matches!(&root, SmartFolderTransitionRoot::Resident)
                && !self.smart_folder_navigation_target_current(&target, &source.logical_source))
        {
            self.retire_smart_folder_payloads([Box::new(child) as RetiredSmartFolderPayload]);
            self.show_feedback_toast(
                "スマートフォルダの並びが変わりました。もう一度操作してください".into(),
            );
            return false;
        }
        // No visible owner may be retired while an admission check can still reject this
        // request. A successful ScannedDir is proof of a Folder target; do not re-enter the
        // ordinary loader's path-type check or its side-effectful pre-scan half.
        if self.sidecar_restore_active()
            || !self.snapshot_scope_allows_open(
                &source.logical_source,
                &super::OpenRequestOwner::Navigation,
            )
        {
            self.retire_smart_folder_payloads([Box::new(child) as RetiredSmartFolderPayload]);
            if !self.sidecar_restore_active() {
                self.show_snapshot_out_of_scope_open_feedback();
            }
            return false;
        }
        // Keep the existing single pin lookup at the visible-adoption boundary, but finish
        // ZIP row construction before retiring the prior owner. The installed side then has
        // no fallible or I/O-bearing materialization step.
        let child = match child {
            SmartPhysicalReady::Zip(enumeration) => SmartPhysicalReady::ZipPrepared(
                self.prepare_zip_grid(path.clone(), enumeration, Some(&source.logical_source)),
            ),
            other => other,
        };
        if !self.adopt_smart_child_session(request_id, root, target, &source) {
            self.retire_smart_folder_payloads([Box::new(child) as RetiredSmartFolderPayload]);
            return false;
        }
        // The old visible PDF/ZIP receivers still belong to the prior view until this exact
        // adoption. Retire them now even when their path equals the new request's load alias;
        // path equality is not an ownership stamp. This also clears the old fullscreen defer
        // before the new request publishes its own reservation below.
        self.pdf_enumerate_pending = None;
        self.zip_enumerate_pending = None;
        self.fs_nav_after_pdf_enumerate = None;
        self.pdf_placeholder_count = None;
        self.note_reading_history_container_open_path(&source.logical_source);
        if effects.suppress_rating_filter
            && self.rating_filter_suppressed_at.is_none()
            && self.rating_filter_active()
        {
            self.rating_filter_suppressed_at =
                Some((source.logical_source.clone(), self.settings.rating_filter));
            self.show_feedback_toast("★フィルタ一時解除中 (親へ戻ると復元)".into());
        }
        if effects.suppress_facet_filter {
            self.maybe_suppress_facet_filter_for_opened_container_path(&source.logical_source);
        }
        if let Some(anchor) = effects.select_after_load {
            self.select_after_load = Some(anchor);
        }
        if auto_fullscreen {
            if matches!(&child, SmartPhysicalReady::Folder(_)) {
                self.pending_auto_fs_open = true;
            } else if !matches!(&child, SmartPhysicalReady::Error(_)) {
                self.fs_nav_after_pdf_enumerate = Some(super::DeferredFsReopen {
                    history_trigger: super::HistoryTrigger::UserChosen,
                    resume_slideshow: false,
                    target: super::DeferredFsTarget::None,
                    resume_to_last_page: self.settings.book_open_resume.resumes(),
                    from_explicit_open: true,
                    preserve_after_password_prompt: true,
                });
            }
        }
        let authority = super::VisibleInstallAuthority::SmartPhysical {
            request_id,
            definition_id,
            logical_source: &source.logical_source,
            load_path: &source.load_path,
        };
        match child {
            SmartPhysicalReady::Folder(scan) => {
                self.install_smart_scanned_folder(path, scan, authority)
            }
            SmartPhysicalReady::PdfPages {
                pages,
                password,
                save_password,
            } => {
                if save_password && let Some(password) = password.as_ref() {
                    self.pdf_password_pending_save = Some((path.clone(), password.clone()));
                }
                let page_count = pages.len() as u32;
                let (items, image_metas, existing_keys) = Self::build_pdf_page_rows(&path, &pages);
                self.start_loading_items_inner(
                    path.clone(),
                    items,
                    image_metas,
                    existing_keys,
                    Vec::new(),
                    None,
                    None,
                    None,
                    None,
                    None,
                    authority,
                );
                // The accepted rows are already final. Reuse the ordinary verification tail
                // for password persistence, page-count cache, and deferred fullscreen without
                // rebuilding the grid or enumerating the PDF again.
                self.pdf_placeholder_count = Some(page_count);
                let handle = crate::pdf_loader::completed_enumerate_handle(&path, Ok(pages));
                self.pdf_enumerate_pending = Some((path, password, handle));
                true
            }
            SmartPhysicalReady::PdfWarm {
                page_count,
                mtime,
                file_size,
                password,
                save_password,
                handle,
            } => {
                if save_password && let Some(password) = password.as_ref() {
                    self.pdf_password_pending_save = Some((path.clone(), password.clone()));
                }
                let (items, image_metas, existing_keys) =
                    Self::build_pdf_meta_placeholder_rows(&path, page_count, mtime, file_size);
                self.start_loading_items_inner(
                    path.clone(),
                    items,
                    image_metas,
                    existing_keys,
                    Vec::new(),
                    None,
                    None,
                    None,
                    None,
                    None,
                    authority,
                );
                self.pdf_placeholder_count = Some(page_count);
                self.pdf_enumerate_pending = Some((path, password, handle));
                true
            }
            SmartPhysicalReady::ZipPrepared(prepared) => {
                self.finalize_prepared_zip_grid(path, self.input_seq, prepared, authority);
                true
            }
            SmartPhysicalReady::Zip(_) => unreachable!("ZIP rows were prepared before adoption"),
            SmartPhysicalReady::Error(reason) => {
                self.start_loading_items_inner(
                    path,
                    Vec::new(),
                    Vec::new(),
                    HashSet::new(),
                    Vec::new(),
                    None,
                    None,
                    None,
                    None,
                    None,
                    authority,
                );
                self.set_empty_items_reason(reason);
                true
            }
        }
    }

    fn spawn_smart_transition_count(
        &self,
        transition: &SmartFolderTransition,
        snapshot: SmartFolderSnapshot,
        refresh: bool,
        tombstones_at_start: HashSet<String>,
    ) -> Result<SmartFolderPreparePending, String> {
        let current_tombstones = self
            .smart_folder_removed_paths
            .get(&snapshot.definition.id)
            .cloned()
            .unwrap_or_default();
        let removed_paths =
            smart_folder_tombstones_after_scan_start(&current_tombstones, &tombstones_at_start);
        spawn_smart_folder_result_count(
            snapshot,
            transition.request_id,
            self.smart_folder_metadata_revision,
            refresh,
            tombstones_at_start,
            removed_paths,
            self.rating_db.is_some(),
            self.tags_db.is_some(),
            self.local_adjust_db.is_some(),
            self.archive_cache_db.clone(),
        )
    }

    fn spawn_smart_transition_rescan(
        &self,
        transition: &SmartFolderTransition,
        definition: crate::settings::SmartFolderDefinition,
        refresh: bool,
    ) -> Result<SmartFolderPending, String> {
        let io_sem = self
            .indexer_manager
            .as_ref()
            .map(|manager| manager.io_sem())
            .unwrap_or_else(|| {
                Arc::new(crate::io_semaphore::GlobalIoSemaphore::new(
                    self.settings.indexer_speed_profile.io_permits().max(1),
                ))
            });
        let tombstones = self
            .smart_folder_removed_paths
            .get(&definition.id)
            .cloned()
            .unwrap_or_default();
        spawn_smart_folder_scan(
            definition,
            transition.request_id,
            refresh,
            tombstones,
            SmartFolderScanOptions::from(&self.settings),
            io_sem,
            Arc::clone(&self.activity_gate),
        )
    }

    fn spawn_smart_transition_prepare(
        &self,
        transition: &SmartFolderTransition,
        snapshot: SmartFolderSnapshot,
        refresh: bool,
        tombstones_at_start: HashSet<String>,
        membership: Option<EvaluatedSmartFolderMembership>,
    ) -> Result<SmartFolderPreparePending, String> {
        let current_tombstones = self
            .smart_folder_removed_paths
            .get(&snapshot.definition.id)
            .cloned()
            .unwrap_or_default();
        let removed_paths =
            smart_folder_tombstones_after_scan_start(&current_tombstones, &tombstones_at_start);
        let presentation = SmartFolderPresentation::for_location(
            self,
            &smart_folder_synthetic_path(snapshot.definition.id),
            snapshot.definition.grouping,
        );
        spawn_smart_folder_prepare(
            snapshot,
            presentation.sort,
            presentation.display,
            transition.request_id,
            self.smart_folder_metadata_revision,
            refresh,
            true,
            tombstones_at_start,
            removed_paths,
            self.rating_db.is_some(),
            self.tags_db.is_some(),
            self.local_adjust_db.is_some(),
            membership,
            None,
            SmartFolderPrepareResources {
                prepare_catalog: true,
                load_adjustments: self.adjustment_db.is_some(),
                load_export_crops: self.export_crop_db.is_some(),
                load_view_trims: self.view_trim_db.is_some(),
                load_masks: self.mask_db.is_some(),
                load_conceals: self.conceal_db.is_some(),
                load_comics: self.comic_db.is_some(),
                load_video_pins: self.video_pin_db.is_some(),
                folder_thumb_sort: self.settings.folder_thumb_sort,
                folder_thumb_depth: self.settings.folder_thumb_depth,
                folder_pin_db: self.folder_thumb_pin_db.clone(),
                archive_cache_db: self.archive_cache_db.clone(),
                reused_catalog_db: None,
                reused_catalog_entries: None,
            },
        )
    }

    fn begin_smart_child_preflight(
        &mut self,
        source: &SmartChildSource,
        kind: SmartChildKind,
    ) -> Result<SmartPhysicalPreflight, String> {
        let path = source.load_path.clone();
        match kind {
            SmartChildKind::Folder => {
                let cancel = Arc::new(AtomicBool::new(false));
                let worker_cancel = Arc::clone(&cancel);
                let worker_path = path.clone();
                let include_convertible =
                    !self.settings.archive_file_handling_ignores_convertible();
                let show_hidden = self.settings.show_hidden_files;
                let (tx, rx) = mpsc::channel();
                std::thread::Builder::new()
                    .name("smart-folder-child-scan".into())
                    .spawn(move || {
                        let result =
                            super::folder_scan::scan_directory_with_convertible_archives_cancel(
                                &worker_path,
                                include_convertible,
                                show_hidden,
                                Some(&worker_cancel),
                            );
                        if !worker_cancel.load(Ordering::Relaxed) {
                            let _ = tx.send(result);
                        }
                    })
                    .map_err(|error| {
                        format!("フォルダの読み取りを開始できませんでした: {error}")
                    })?;
                Ok(SmartPhysicalPreflight::Folder { path, cancel, rx })
            }
            SmartChildKind::Pdf => {
                let saved_password = self.pdf_passwords.get(&path);
                let password = self.pdf_open_password(&path);
                let warm = self.peek_pdf_meta_cache(&path, saved_password.is_some());
                let handle = crate::pdf_loader::enumerate_pages_async(&path, password.as_deref());
                Ok(SmartPhysicalPreflight::Pdf {
                    path,
                    password,
                    save_password: false,
                    handle,
                    warm,
                })
            }
            SmartChildKind::Zip => {
                let cancel = Arc::new(AtomicBool::new(false));
                let worker_cancel = Arc::clone(&cancel);
                let worker_path = path.clone();
                let input_seq = self.input_seq;
                let (tx, rx) = mpsc::channel();
                std::thread::Builder::new()
                    .name("smart-folder-zip-enumerate".into())
                    .spawn(move || {
                        if worker_cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let result = if crate::rar_loader::is_rar_path(&worker_path) {
                            crate::rar_loader::enumerate_image_entries_detailed_traced(
                                &worker_path,
                                input_seq,
                            )
                        } else {
                            crate::zip_loader::enumerate_image_entries_detailed(&worker_path)
                        }
                        .map_err(|error| error.to_string());
                        if let Ok(enumeration) = &result {
                            // Existing ZIP-key maintenance belongs to the worker and must finish
                            // before a visible result reads the migrated page metadata.
                            crate::zip_key_migration::migrate_if_needed(
                                &worker_path,
                                &enumeration.legacy_renames,
                            );
                        }
                        if !worker_cancel.load(Ordering::Relaxed) {
                            let _ = tx.send(result);
                        }
                    })
                    .map_err(|error| format!("ZIP の読み取りを開始できませんでした: {error}"))?;
                Ok(SmartPhysicalPreflight::Zip { path, cancel, rx })
            }
            SmartChildKind::ConvertibleArchive => Ok(SmartPhysicalPreflight::ArchiveConvert),
        }
    }

    fn smart_transition_root_is_current(&self, root: &SmartFolderTransitionRoot) -> bool {
        match root {
            SmartFolderTransitionRoot::Offscreen(prepared) => {
                let Some(definition) = self
                    .settings
                    .smart_folders
                    .iter()
                    .find(|definition| definition.id == prepared.snapshot.definition.id)
                else {
                    return false;
                };
                let removed_since_scan = self
                    .smart_folder_removed_paths
                    .get(&definition.id)
                    .map(|removed| {
                        smart_folder_tombstones_after_scan_start(
                            removed,
                            &prepared.authoritative_ignored_tombstones,
                        )
                    })
                    .unwrap_or_default();
                smart_folder_scan_rules_match(&prepared.snapshot.definition, definition)
                    && prepared.presentation
                        == SmartFolderPresentation::for_location(
                            self,
                            &smart_folder_synthetic_path(definition.id),
                            definition.grouping,
                        )
                    && prepared.metadata_revision == self.smart_folder_metadata_revision
                    && removed_since_scan.is_subset(&prepared.applied_tombstones)
            }
            SmartFolderTransitionRoot::Resident => {
                let Some(session) = self.top_level_grid_view.smart_folder_session() else {
                    return false;
                };
                let Some(definition) = self
                    .settings
                    .smart_folders
                    .iter()
                    .find(|definition| definition.id == session.definition_id)
                else {
                    return false;
                };
                // Presentation edits are deferred while this physical open is in flight. The
                // old root remains a valid source for the chosen child; its frozen stamp is
                // compared on return. Rule/source changes invalidate that source instead.
                session.root_snapshot().is_some_and(|snapshot| {
                    smart_folder_scan_rules_match(&snapshot.definition, definition)
                })
            }
        }
    }

    fn poll_smart_folder_transition_root(&mut self, ctx: &egui::Context) {
        let Some(mut transition) = self.smart_folder_transition.take() else {
            return;
        };
        if self.smart_folder_source_lease().as_ref() != Some(&transition.source) {
            // An independent navigation won. Drop cancels only this request's workers and retires
            // its unshown payload; it does not touch the still-visible source.
            if matches!(
                &transition.phase,
                SmartFolderTransitionPhase::ChildPreflight {
                    child: SmartPhysicalPreflight::PdfPassword { .. },
                    ..
                }
            ) {
                self.clear_smart_pdf_dialog_if_unclaimed();
            }
            self.release_staged_smart_nav_lock(transition.request_id, &transition.intent);
            return;
        }
        let mut adopted_folder_nav = false;
        let mut launch_archive_conversion = false;
        if let SmartFolderTransitionPhase::ChildPreflight {
            child:
                SmartPhysicalPreflight::PdfPassword {
                    invalid_password, ..
                },
            ..
        } = &transition.phase
            && !self.pdf_password_request_pending_in_any_context()
            && !self.show_pdf_password_dialog
        {
            self.pdf_password_input.clear();
            self.pdf_password_error =
                (*invalid_password).then(|| "パスワードが正しくありません".to_owned());
            self.pdf_password_save = false;
            self.show_pdf_password_dialog = true;
        }
        if self.sidecar_restore_active() {
            self.smart_folder_transition = Some(transition);
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(50));
        let phase = std::mem::replace(&mut transition.phase, SmartFolderTransitionPhase::Retired);
        let was_password_prompt = matches!(
            &phase,
            SmartFolderTransitionPhase::ChildPreflight {
                child: SmartPhysicalPreflight::PdfPassword { .. },
                ..
            }
        );
        let next = match phase {
            SmartFolderTransitionPhase::RootScan(pending) => match pending.rx.try_recv() {
                Ok(SmartFolderScanEvent::Progress(progress)) => {
                    transition.progress = progress;
                    Some(SmartFolderTransitionPhase::RootScan(pending))
                }
                Ok(SmartFolderScanEvent::Done(mut result)) => {
                    let current = self.settings.smart_folders.iter().find(|definition| {
                        definition.id == pending.definition_id
                            && smart_folder_scan_rules_match(
                                definition,
                                &result.snapshot.definition,
                            )
                    });
                    if pending.generation != transition.request_id || current.is_none() {
                        self.retire_smart_folder_payloads([
                            Box::new(result) as RetiredSmartFolderPayload
                        ]);
                        None
                    } else {
                        adopt_smart_folder_presentation(
                            &mut result.snapshot.definition,
                            current.expect("checked above"),
                        );
                        if result.snapshot.diag.source_failures
                            == active_rules(&result.snapshot.definition).len()
                        {
                            self.show_feedback_toast(
                                "スマートフォルダの検索元を1件も読み込めませんでした".into(),
                            );
                            self.retire_smart_folder_payloads([
                                Box::new(result) as RetiredSmartFolderPayload
                            ]);
                            None
                        } else if result.snapshot.entries.len() >= SMART_FOLDER_CONFIRM_THRESHOLD
                            && smart_folder_definition_needs_result_count(
                                &result.snapshot.definition,
                            )
                        {
                            match self.spawn_smart_transition_count(
                                &transition,
                                result.snapshot,
                                pending.refresh,
                                pending.tombstones_at_start.clone(),
                            ) {
                                Ok(count) => Some(SmartFolderTransitionPhase::RootCount(count)),
                                Err(message) => {
                                    self.show_feedback_toast(message);
                                    None
                                }
                            }
                        } else if result.snapshot.entries.len() >= SMART_FOLDER_CONFIRM_THRESHOLD {
                            let result_count = result.snapshot.entries.len();
                            Some(SmartFolderTransitionPhase::RootConfirm(
                                SmartFolderConfirmPending {
                                    snapshot: result.snapshot,
                                    result_count,
                                    membership: None,
                                    membership_revision: None,
                                    generation: transition.request_id,
                                    refresh: pending.refresh,
                                    tombstones_at_start: pending.tombstones_at_start.clone(),
                                },
                            ))
                        } else {
                            match self.spawn_smart_transition_prepare(
                                &transition,
                                result.snapshot,
                                pending.refresh,
                                pending.tombstones_at_start.clone(),
                                None,
                            ) {
                                Ok(prepare) => {
                                    Some(SmartFolderTransitionPhase::RootPrepare(prepare))
                                }
                                Err(message) => {
                                    self.show_feedback_toast(message);
                                    None
                                }
                            }
                        }
                    }
                }
                Ok(SmartFolderScanEvent::Cancelled) => None,
                Err(mpsc::TryRecvError::Empty) => {
                    Some(SmartFolderTransitionPhase::RootScan(pending))
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.show_feedback_toast("スマートフォルダ走査が中断されました".into());
                    None
                }
            },
            SmartFolderTransitionPhase::RootCount(pending) => match pending.rx.try_recv() {
                Ok(SmartFolderPrepareEvent::Progress(progress)) => {
                    transition.progress = progress;
                    Some(SmartFolderTransitionPhase::RootCount(pending))
                }
                Ok(SmartFolderPrepareEvent::Counted(mut counted)) => {
                    let current = self.settings.smart_folders.iter().find(|definition| {
                        definition.id == pending.definition_id
                            && smart_folder_scan_rules_match(
                                definition,
                                &counted.snapshot.definition,
                            )
                    });
                    if pending.generation != transition.request_id || current.is_none() {
                        self.retire_smart_folder_payloads([counted as RetiredSmartFolderPayload]);
                        None
                    } else {
                        adopt_smart_folder_presentation(
                            &mut counted.snapshot.definition,
                            current.expect("checked above"),
                        );
                        if counted.metadata_revision != self.smart_folder_metadata_revision {
                            match self.spawn_smart_transition_count(
                                &transition,
                                counted.snapshot,
                                counted.refresh,
                                counted.tombstones_at_start.clone(),
                            ) {
                                Ok(count) => Some(SmartFolderTransitionPhase::RootCount(count)),
                                Err(message) => {
                                    self.show_feedback_toast(message);
                                    None
                                }
                            }
                        } else if counted.result_count >= SMART_FOLDER_CONFIRM_THRESHOLD {
                            Some(SmartFolderTransitionPhase::RootConfirm(
                                SmartFolderConfirmPending {
                                    snapshot: counted.snapshot,
                                    result_count: counted.result_count,
                                    membership: Some(counted.membership),
                                    membership_revision: Some(counted.metadata_revision),
                                    generation: transition.request_id,
                                    refresh: counted.refresh,
                                    tombstones_at_start: counted.tombstones_at_start.clone(),
                                },
                            ))
                        } else {
                            match self.spawn_smart_transition_prepare(
                                &transition,
                                counted.snapshot,
                                counted.refresh,
                                counted.tombstones_at_start.clone(),
                                Some(counted.membership),
                            ) {
                                Ok(prepare) => {
                                    Some(SmartFolderTransitionPhase::RootPrepare(prepare))
                                }
                                Err(message) => {
                                    self.show_feedback_toast(message);
                                    None
                                }
                            }
                        }
                    }
                }
                Ok(SmartFolderPrepareEvent::Error(message)) => {
                    self.show_feedback_toast(message);
                    None
                }
                Ok(SmartFolderPrepareEvent::Cancelled) => None,
                Ok(SmartFolderPrepareEvent::Done(prepared)) => {
                    self.retire_smart_folder_payloads([prepared as RetiredSmartFolderPayload]);
                    None
                }
                Err(mpsc::TryRecvError::Empty) => {
                    Some(SmartFolderTransitionPhase::RootCount(pending))
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.show_feedback_toast("スマートフォルダの件数確認が中断されました".into());
                    None
                }
            },
            SmartFolderTransitionPhase::RootPrepare(pending) => match pending.rx.try_recv() {
                Ok(SmartFolderPrepareEvent::Progress(progress)) => {
                    transition.progress = progress;
                    Some(SmartFolderTransitionPhase::RootPrepare(pending))
                }
                Ok(SmartFolderPrepareEvent::Done(prepared)) => {
                    if pending.generation != transition.request_id {
                        self.retire_smart_folder_payloads([prepared as RetiredSmartFolderPayload]);
                        None
                    } else {
                        Some(SmartFolderTransitionPhase::RootReady(prepared))
                    }
                }
                Ok(SmartFolderPrepareEvent::Error(message)) => {
                    self.show_feedback_toast(message);
                    None
                }
                Ok(SmartFolderPrepareEvent::Cancelled) => None,
                Ok(SmartFolderPrepareEvent::Counted(counted)) => {
                    self.retire_smart_folder_payloads([counted as RetiredSmartFolderPayload]);
                    None
                }
                Err(mpsc::TryRecvError::Empty) => {
                    Some(SmartFolderTransitionPhase::RootPrepare(pending))
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.show_feedback_toast("スマートフォルダの表示準備が中断されました".into());
                    None
                }
            },
            SmartFolderTransitionPhase::RootReady(mut prepared) => {
                let current_definition = self
                    .settings
                    .smart_folders
                    .iter()
                    .find(|definition| definition.id == prepared.snapshot.definition.id)
                    .cloned();
                if let Some(current_definition) = current_definition {
                    if !smart_folder_scan_rules_match(
                        &prepared.snapshot.definition,
                        &current_definition,
                    ) {
                        // A changed rule or source invalidates the scanned facts. The same
                        // navigation authority starts a fresh scan; the old display is untouched.
                        match self.spawn_smart_transition_rescan(
                            &transition,
                            current_definition,
                            prepared.refresh,
                        ) {
                            Ok(pending) => Some(SmartFolderTransitionPhase::RootScan(pending)),
                            Err(message) => {
                                self.show_feedback_toast(message);
                                None
                            }
                        }
                    } else {
                        let latest_presentation = SmartFolderPresentation::for_location(
                            self,
                            &smart_folder_synthetic_path(current_definition.id),
                            current_definition.grouping,
                        );
                        let removed_since_scan = self
                            .smart_folder_removed_paths
                            .get(&current_definition.id)
                            .map(|removed| {
                                smart_folder_tombstones_after_scan_start(
                                    removed,
                                    &prepared.authoritative_ignored_tombstones,
                                )
                            })
                            .unwrap_or_default();
                        let tombstones_stale =
                            !removed_since_scan.is_subset(&prepared.applied_tombstones);
                        if prepared.metadata_revision != self.smart_folder_metadata_revision
                            || prepared.presentation != latest_presentation
                            || tombstones_stale
                        {
                            let PreparedSmartFolder {
                                mut snapshot,
                                items,
                                image_metas,
                                video_items,
                                metadata,
                                resort_metadata,
                                refresh,
                                authoritative_ignored_tombstones,
                                applied_tombstones,
                                ..
                            } = *prepared;
                            self.retire_prepared_smart_folder_install_payload(
                                items,
                                image_metas,
                                video_items,
                                metadata,
                                resort_metadata,
                                applied_tombstones,
                            );
                            adopt_smart_folder_presentation(
                                &mut snapshot.definition,
                                &current_definition,
                            );
                            match self.spawn_smart_transition_prepare(
                                &transition,
                                snapshot,
                                refresh,
                                authoritative_ignored_tombstones,
                                None,
                            ) {
                                Ok(pending) => {
                                    Some(SmartFolderTransitionPhase::RootPrepare(pending))
                                }
                                Err(message) => {
                                    self.show_feedback_toast(message);
                                    None
                                }
                            }
                        } else {
                            match &transition.target {
                                SmartFolderTransitionTarget::Root(_) => {
                                    let history_ready = match &transition.intent {
                                        SmartTransitionIntent::History(peek) => {
                                            peek.is_current(self)
                                        }
                                        SmartTransitionIntent::Direct(_)
                                        | SmartTransitionIntent::Refresh
                                        | SmartTransitionIntent::FolderNav(_)
                                        | SmartTransitionIntent::Return { .. } => true,
                                    };
                                    if history_ready {
                                        let record_history = matches!(
                                            transition.intent,
                                            SmartTransitionIntent::Direct(_)
                                        );
                                        if self.adopt_smart_transition_root(
                                            transition.request_id,
                                            *prepared,
                                            record_history,
                                            match &transition.intent {
                                                SmartTransitionIntent::Return { anchor } => {
                                                    anchor.clone()
                                                }
                                                _ => None,
                                            },
                                        ) {
                                            if let SmartTransitionIntent::History(peek) =
                                                &transition.intent
                                            {
                                                peek.clone().commit(self);
                                            }
                                            self.reapply_local_search_after_smart_folder_prepare(
                                                ctx,
                                            );
                                        }
                                    }
                                    None
                                }
                                SmartFolderTransitionTarget::Child { source, kind, .. } => {
                                    match self.begin_smart_child_preflight(source, *kind) {
                                        Ok(child) => {
                                            launch_archive_conversion =
                                                *kind == SmartChildKind::ConvertibleArchive;
                                            Some(SmartFolderTransitionPhase::ChildPreflight {
                                                root: SmartFolderTransitionRoot::Offscreen(
                                                    prepared,
                                                ),
                                                child,
                                            })
                                        }
                                        Err(message) => {
                                            self.show_feedback_toast(message);
                                            None
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    self.show_feedback_toast("スマートフォルダが見つかりません".into());
                    None
                }
            }
            SmartFolderTransitionPhase::ChildPreflight { root, child } => match child.poll() {
                SmartPhysicalPoll::Waiting(child) => {
                    Some(SmartFolderTransitionPhase::ChildPreflight { root, child })
                }
                SmartPhysicalPoll::Ready(child) => {
                    Some(SmartFolderTransitionPhase::ChildReady { root, child })
                }
                SmartPhysicalPoll::PasswordRequired {
                    path,
                    invalid_password,
                } => Some(SmartFolderTransitionPhase::ChildPreflight {
                    root,
                    child: SmartPhysicalPreflight::PdfPassword {
                        path,
                        invalid_password,
                    },
                }),
                SmartPhysicalPoll::Cancelled => None,
                SmartPhysicalPoll::Failed(message) => {
                    self.show_feedback_toast(message);
                    None
                }
            },
            SmartFolderTransitionPhase::ChildReady { root, child } => {
                if !self.smart_transition_root_is_current(&root) {
                    match root {
                        SmartFolderTransitionRoot::Offscreen(prepared) => {
                            self.retire_smart_folder_payloads([
                                Box::new(child) as RetiredSmartFolderPayload
                            ]);
                            Some(SmartFolderTransitionPhase::RootReady(prepared))
                        }
                        SmartFolderTransitionRoot::Resident => {
                            self.retire_smart_folder_payloads([
                                Box::new(child) as RetiredSmartFolderPayload
                            ]);
                            None
                        }
                    }
                } else if let SmartFolderTransitionTarget::Child {
                    state,
                    source,
                    auto_fullscreen,
                    effects,
                    archive_commit,
                    ..
                } = &transition.target
                {
                    let history_ready = match &transition.intent {
                        SmartTransitionIntent::History(peek) => peek.is_current(self),
                        SmartTransitionIntent::Direct(_)
                        | SmartTransitionIntent::Refresh
                        | SmartTransitionIntent::FolderNav(_)
                        | SmartTransitionIntent::Return { .. } => true,
                    };
                    if history_ready {
                        let adopted = self.adopt_smart_child_ready(
                            transition.request_id,
                            root,
                            state.clone(),
                            source.clone(),
                            child,
                            *auto_fullscreen,
                            effects.clone(),
                        );
                        if adopted && let Some(commit) = archive_commit.clone() {
                            self.commit_smart_folder_archive_source(commit);
                        }
                        if adopted && let SmartTransitionIntent::History(peek) = &transition.intent
                        {
                            peek.clone().commit(self);
                        }
                        if adopted && let SmartTransitionIntent::FolderNav(nav) = &transition.intent
                        {
                            if matches!(
                                &nav.mode,
                                super::FolderNavMode::SmartFolder {
                                    fullscreen: true,
                                    ..
                                }
                            ) {
                                self.adopt_smart_folder_navigation_sequence(transition.request_id);
                                let reason = self.reopen_fullscreen_after_folder_nav_load(
                                    ctx,
                                    nav.restore_video_tile,
                                    false,
                                    nav.history_trigger,
                                );
                                if reason == "smart_transition_defer" {
                                    adopted_folder_nav =
                                        self.handoff_staged_smart_folder_nav(nav.clone());
                                }
                            }
                            if !adopted_folder_nav {
                                self.chain_folder_nav_if_pending(
                                    nav.queued_steps,
                                    nav.mode.clone(),
                                );
                                adopted_folder_nav = true;
                            }
                        }
                        None
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            ready @ SmartFolderTransitionPhase::RootConfirm(_) => Some(ready),
            SmartFolderTransitionPhase::Retired => None,
        };
        if let Some(next) = next {
            transition.phase = next;
            self.smart_folder_transition = Some(transition);
            if launch_archive_conversion {
                self.start_smart_archive_conversion_current();
            }
        } else {
            if !adopted_folder_nav {
                self.release_staged_smart_nav_lock(transition.request_id, &transition.intent);
            }
            if was_password_prompt {
                self.clear_smart_pdf_dialog_if_unclaimed();
            }
        }
        self.reprepare_visible_smart_root_after_staged_terminal();
    }
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
    file_size: Option<i64>,
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
    file_size: Option<i64>,
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
        let Some(file_size) = file_size.filter(|size| *size > 0) else {
            return false;
        };
        let (min, max) = preset.range_bytes();
        let size = file_size as u64;
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
            Some(super::folder_scan::ScannedMediaEntry {
                path: candidate.path.clone(),
                kind,
                mtime: candidate.mtime,
                file_size: candidate.file_size.unwrap_or(0),
                sort_meta: crate::settings::ListingSortMetadata::new(
                    candidate.mtime,
                    candidate.file_size,
                ),
            })
        })
        .collect::<Vec<_>>();
    super::folder_scan::filter_upscaled_video_pairs_fast(&mut media, entry_file_names_ci);
    let mut video_thumb_overrides = HashMap::new();
    if options.skip_image_if_video_exists {
        let filtered = super::folder_scan::filter_video_image_duplicates(
            &mut media,
            options.video_thumb_use_sidecar_image,
        );
        for (video, image) in filtered.sidecars {
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
            container_metas.push(Some((candidate.mtime, candidate.file_size.unwrap_or(0))));
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
        .map(|entry| crate::path_key::normalize_keep_drive(&entry.path))
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
        if crate::fs_entry::is_internal_app_entry_name(&entry.file_name()) {
            continue;
        }
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
        let file_size = (kind != SmartFolderEntryKind::Folder)
            .then(|| metadata.as_ref().map(|metadata| metadata.len() as i64))
            .flatten();
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
    bookmarks: &crate::bookmark_browser::BookmarkPresence,
    converted_archive_paths: &HashMap<String, ConvertedArchiveSourceState>,
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
            FacetEditFlag::Bookmarked => bookmark_matches_for_entry(bookmarks, entry),
            FacetEditFlag::Unbookmarked => !bookmark_matches_for_entry(bookmarks, entry),
        };
        if !matched {
            return false;
        }
    }
    true
}

fn bookmark_matches_for_entry(
    bookmarks: &crate::bookmark_browser::BookmarkPresence,
    entry: &SmartFolderEntry,
) -> bool {
    match entry.kind {
        SmartFolderEntryKind::Video | SmartFolderEntryKind::Audio => {
            bookmarks.has_media_path(&entry.path)
        }
        SmartFolderEntryKind::Folder
        | SmartFolderEntryKind::Zip
        | SmartFolderEntryKind::Pdf
        | SmartFolderEntryKind::Archive => bookmarks.has_book_container(&entry.path),
        SmartFolderEntryKind::Image => false,
    }
}

fn edit_key_matches_for_entry(
    keys: &std::collections::BTreeSet<String>,
    entry: &SmartFolderEntry,
    key: &str,
    include_descendants: bool,
    converted_archive_paths: &HashMap<String, ConvertedArchiveSourceState>,
) -> bool {
    if edit_key_matches(keys, entry, key, include_descendants) {
        return true;
    }
    if entry.kind != SmartFolderEntryKind::Archive {
        return false;
    }
    converted_archive_paths
        .get(&crate::path_key::normalize_keep_drive(&entry.path))
        .and_then(ConvertedArchiveSourceState::load_path)
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

fn load_edit_key_sets_readonly(
    wanted: &std::collections::BTreeSet<crate::settings::FacetEditFlag>,
    local_adjust: std::collections::BTreeSet<String>,
) -> Result<SmartEditKeySets, String> {
    use crate::settings::FacetEditFlag;
    let mut result = SmartEditKeySets {
        local_adjust,
        ..SmartEditKeySets::default()
    };
    if (wanted.contains(&FacetEditFlag::Adjustment)
        || wanted.contains(&FacetEditFlag::AiAdjustment))
        && crate::adjustment_db::AdjustmentDb::db_path()
            .try_exists()
            .unwrap_or(false)
    {
        result.adjustment = crate::adjustment_db::AdjustmentDb::open_readonly()
            .map_err(|error| format!("補正 DB を読み込めませんでした: {error}"))?
            .load_page_param_keys();
    }
    if wanted.contains(&FacetEditFlag::Mask)
        && crate::mask_db::MaskDb::db_path()
            .try_exists()
            .unwrap_or(false)
    {
        result.mask = crate::mask_db::MaskDb::open_readonly()
            .map_err(|error| format!("消しゴム DB を読み込めませんでした: {error}"))?
            .load_all_mask_keys();
    }
    if wanted.contains(&FacetEditFlag::Conceal)
        && crate::conceal_db::ConcealDb::db_path()
            .try_exists()
            .unwrap_or(false)
    {
        result.conceal =
            crate::conceal_db::ConcealDb::open_readonly(&crate::conceal_db::ConcealDb::db_path())
                .map_err(|error| format!("隠蔽加工 DB を読み込めませんでした: {error}"))?
                .load_all_conceal_keys();
    }
    if wanted.contains(&FacetEditFlag::Annotation)
        && crate::comic_db::ComicDb::db_path()
            .try_exists()
            .unwrap_or(false)
    {
        result.annotation = crate::comic_db::ComicDb::open_readonly()
            .map_err(|error| format!("注釈 DB を読み込めませんでした: {error}"))?
            .load_all_comic_keys();
    }
    if wanted.contains(&FacetEditFlag::Rotation)
        && crate::rotation_db::RotationDb::db_path()
            .try_exists()
            .unwrap_or(false)
    {
        result.rotation = crate::rotation_db::RotationDb::open_readonly()
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct SmartFolderPresentation {
    sort: crate::settings::SortOrder,
    display: crate::settings::GridDisplayOrder,
    grouping: crate::settings::SubfolderExpansionOrder,
}

impl SmartFolderPresentation {
    fn current(app: &App, grouping: crate::settings::SubfolderExpansionOrder) -> Self {
        Self {
            sort: app.settings.sort_order,
            display: app.settings.grid_display_order.normalized(),
            grouping,
        }
    }

    /// Compare a parked root with the settings that will own its destination, without applying
    /// that destination's favorite overlay to the still-visible physical child.
    fn for_location(
        app: &App,
        path: &Path,
        grouping: crate::settings::SubfolderExpansionOrder,
    ) -> Self {
        let from_state = |state: &crate::settings::FavoriteViewState| Self {
            sort: state.sort_order,
            display: state.grid_display_order.normalized(),
            grouping,
        };
        let current = Self::current(app, grouping);
        let common = app
            .settings
            .favorite_view_overlay
            .as_ref()
            .map(|overlay| from_state(&overlay.common))
            .unwrap_or_else(|| current.clone());
        // The real transition clears an old overlay even after remember was turned off.
        if !app.settings.remember_favorite_view_state {
            return common;
        }
        match app.favorite_view_owner_for_path(path) {
            Some(id) if app.settings.active_favorite_view_id() == Some(id) => current,
            Some(id) => app
                .favorite_view_states
                .get(&id)
                .map(from_state)
                // A newly assigned favorite inherits the currently effective state.
                .unwrap_or(current),
            None => common,
        }
    }
}

fn compare_smart_entries_within_group(
    sort: crate::settings::SortOrder,
    a: &SmartFolderEntry,
    ak: &SmartEntrySortKey,
    b: &SmartFolderEntry,
    bk: &SmartEntrySortKey,
) -> std::cmp::Ordering {
    sort.compare_listing_keys(
        &ak.name,
        crate::settings::ListingSortMetadata::new(a.mtime, a.file_size),
        &bk.name,
        crate::settings::ListingSortMetadata::new(b.mtime, b.file_size),
    )
    .then_with(|| a.path.cmp(&b.path))
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
    db: Option<&crate::archive_cache::ArchiveCacheDb>,
) -> ConvertedArchiveSourceState {
    let mut source = path.to_path_buf();
    if crate::rar_loader::is_rar_path(path) {
        match crate::rar_loader::inspect_for_direct_read(path) {
            Ok(inspection)
                if inspection.decision == crate::rar_loader::RarDirectReadDecision::Direct =>
            {
                return ConvertedArchiveSourceState::Direct(inspection.resolved_path);
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
    db.and_then(|db| db.peek(&source, mtime, size))
        .map(ConvertedArchiveSourceState::CachedZip)
        .unwrap_or(ConvertedArchiveSourceState::Unavailable)
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

struct EvaluatedSmartFolderMembership {
    keys: Arc<Vec<String>>,
    included: Vec<usize>,
    ratings: HashMap<String, u8>,
    tags: HashMap<String, Vec<String>>,
    local_adjust: HashSet<String>,
    converted_archive_paths: HashMap<String, ConvertedArchiveSourceState>,
}

#[allow(clippy::too_many_arguments)]
fn evaluate_smart_folder_membership(
    snapshot: &SmartFolderSnapshot,
    removed_paths: &HashSet<String>,
    load_ratings: bool,
    load_tags: bool,
    load_local_adjust: bool,
    reused_metadata: Option<&ReusedSmartFolderMetadata>,
    archive_cache_db: Option<&crate::archive_cache::ArchiveCacheDb>,
    read_only: bool,
    cancel: &AtomicBool,
    tx: &mpsc::Sender<SmartFolderPrepareEvent>,
) -> Result<Option<EvaluatedSmartFolderMembership>, String> {
    let reuse_metadata = reused_metadata.is_some();
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
    if !reuse_metadata
        && load_ratings
        && (!read_only
            || crate::rating_db::RatingDb::db_path()
                .try_exists()
                .unwrap_or(false))
    {
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
    if !reuse_metadata
        && load_tags
        && (!read_only
            || crate::tags_db::TagsDb::db_path()
                .try_exists()
                .unwrap_or(false))
    {
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
    if !reuse_metadata
        && load_local_adjust
        && (!read_only
            || crate::local_adjust_db::LocalAdjustDb::db_path()
                .try_exists()
                .unwrap_or(false))
    {
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
    } else if read_only {
        load_edit_key_sets_readonly(&wanted_edit_flags, local_adjust.iter().cloned().collect())?
    } else {
        load_edit_key_sets(&wanted_edit_flags, local_adjust.iter().cloned().collect())?
    };
    let bookmark_presence = if reuse_metadata
        || !wanted_edit_flags.iter().any(|flag| {
            matches!(
                flag,
                crate::settings::FacetEditFlag::Bookmarked
                    | crate::settings::FacetEditFlag::Unbookmarked
            )
        }) {
        crate::bookmark_browser::BookmarkPresence::default()
    } else {
        report(SmartFolderPhase::Bookmarks, 0);
        if read_only {
            crate::bookmark_browser::load_presence_readonly()?
        } else {
            crate::bookmark_browser::load_presence()?
        }
    };
    let mut converted_archive_paths = reused_metadata
        .map(|metadata| metadata.converted_archive_cache_paths.clone())
        .unwrap_or_default();
    if !reuse_metadata {
        for (index, entry) in snapshot.entries.iter().enumerate() {
            if index.is_multiple_of(METADATA_CHUNK_SIZE) && cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if entry.kind != SmartFolderEntryKind::Archive {
                continue;
            }
            let state = prepared_converted_archive_path(
                &entry.path,
                entry.mtime,
                entry.file_size.unwrap_or(0),
                archive_cache_db,
            );
            converted_archive_paths
                .insert(crate::path_key::normalize_keep_drive(&entry.path), state);
        }
    }

    report(SmartFolderPhase::Filtering, 0);
    let mut included = Vec::with_capacity(
        reused_metadata
            .map(|metadata| metadata.included_entry_indices.len())
            .unwrap_or(total),
    );
    if let Some(metadata) = reused_metadata {
        for (position, &entry_index) in metadata.included_entry_indices.iter().enumerate() {
            if position.is_multiple_of(METADATA_CHUNK_SIZE) {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                report(SmartFolderPhase::Filtering, position);
            }
            let Some(entry) = snapshot.entries.get(entry_index) else {
                return Err("スマートフォルダの再ソート用キャッシュが一致しません".into());
            };
            if !removed_paths.is_empty()
                && smart_folder_path_is_removed(
                    &crate::path_key::normalize_keep_drive(&entry.path),
                    removed_paths,
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
                    removed_paths,
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
                            &bookmark_presence,
                            &converted_archive_paths,
                        )
                    })
            });
            if included_by_rule {
                included.push(index);
            }
        }
    }

    Ok(Some(EvaluatedSmartFolderMembership {
        keys,
        included,
        ratings,
        tags,
        local_adjust,
        converted_archive_paths,
    }))
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
    precounted_membership: Option<EvaluatedSmartFolderMembership>,
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
    let membership = if let Some(mut membership) = precounted_membership {
        if !removed_paths.is_empty() {
            membership.included.retain(|&entry_index| {
                snapshot.entries.get(entry_index).is_some_and(|entry| {
                    !smart_folder_path_is_removed(
                        &crate::path_key::normalize_keep_drive(&entry.path),
                        &removed_paths,
                    )
                })
            });
        }
        membership
    } else {
        let Some(membership) = evaluate_smart_folder_membership(
            &snapshot,
            &removed_paths,
            load_ratings,
            load_tags,
            load_local_adjust,
            reused_metadata.as_deref(),
            resources.archive_cache_db.as_deref(),
            false,
            cancel,
            tx,
        )?
        else {
            return Ok(None);
        };
        membership
    };
    let EvaluatedSmartFolderMembership {
        keys,
        included,
        ratings,
        tags,
        local_adjust,
        converted_archive_paths: converted_archive_paths_for_filter,
    } = membership;

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
            let within = || compare_smart_entries_within_group(sort, a, ak, b, bk);
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
        image_metas.push(Some((entry.mtime, entry.file_size.unwrap_or(0))));
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
    let presentation = SmartFolderPresentation {
        sort,
        display: display_order.normalized(),
        grouping: snapshot.definition.grouping,
    };
    Ok(Some(PreparedSmartFolder {
        snapshot,
        presentation,
        items,
        image_metas,
        video_items,
        metadata: super::subfolder_expansion::PreparedSubfolderMetadata {
            rating_cache,
            tags_cache,
            local_adjust_pages,
            page_edits: None,
            video_pin_blobs,
            folder_pin_map: None,
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

#[allow(clippy::too_many_arguments)]
fn count_smart_folder_results(
    snapshot: SmartFolderSnapshot,
    metadata_revision: u64,
    refresh: bool,
    tombstones_at_start: HashSet<String>,
    removed_paths: HashSet<String>,
    load_ratings: bool,
    load_tags: bool,
    load_local_adjust: bool,
    archive_cache_db: Option<Arc<crate::archive_cache::ArchiveCacheDb>>,
    cancel: &AtomicBool,
    tx: &mpsc::Sender<SmartFolderPrepareEvent>,
) -> Result<Option<CountedSmartFolder>, String> {
    let Some(membership) = evaluate_smart_folder_membership(
        &snapshot,
        &removed_paths,
        load_ratings,
        load_tags,
        load_local_adjust,
        None,
        archive_cache_db.as_deref(),
        false,
        cancel,
        tx,
    )?
    else {
        return Ok(None);
    };
    let result_count = membership.included.len();
    Ok(Some(CountedSmartFolder {
        snapshot,
        result_count,
        membership,
        metadata_revision,
        refresh,
        tombstones_at_start,
    }))
}

#[allow(clippy::too_many_arguments)]
fn spawn_smart_folder_result_count(
    snapshot: SmartFolderSnapshot,
    generation: u64,
    metadata_revision: u64,
    refresh: bool,
    tombstones_at_start: HashSet<String>,
    removed_paths: HashSet<String>,
    load_ratings: bool,
    load_tags: bool,
    load_local_adjust: bool,
    archive_cache_db: Option<Arc<crate::archive_cache::ArchiveCacheDb>>,
) -> Result<SmartFolderPreparePending, String> {
    let definition_id = snapshot.definition.id;
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("smart-folder-result-count".into())
        .spawn(move || {
            let started = Instant::now();
            let event = match count_smart_folder_results(
                snapshot,
                metadata_revision,
                refresh,
                tombstones_at_start,
                removed_paths,
                load_ratings,
                load_tags,
                load_local_adjust,
                archive_cache_db,
                &cancel_worker,
                &tx,
            ) {
                Ok(Some(counted)) if !cancel_worker.load(Ordering::Relaxed) => {
                    SmartFolderPrepareEvent::Counted(Box::new(counted))
                }
                Ok(_) => SmartFolderPrepareEvent::Cancelled,
                Err(message) => SmartFolderPrepareEvent::Error(message),
            };
            if crate::perf::is_enabled() {
                let (status, items) = match &event {
                    SmartFolderPrepareEvent::Counted(counted) => ("done", counted.result_count),
                    SmartFolderPrepareEvent::Cancelled => ("cancelled", 0),
                    SmartFolderPrepareEvent::Error(_) => ("error", 0),
                    SmartFolderPrepareEvent::Progress(_) => ("progress", 0),
                    SmartFolderPrepareEvent::Done(prepared) => ("prepared", prepared.items.len()),
                };
                crate::perf::event(
                    "smart_folder",
                    "result_count_end",
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
        .map_err(|error| format!("スマートフォルダの件数確認を開始できませんでした: {error}"))?;
    Ok(SmartFolderPreparePending {
        definition_id,
        generation,
        cancel,
        rx,
    })
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
    precounted_membership: Option<EvaluatedSmartFolderMembership>,
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
                precounted_membership,
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
                    SmartFolderPrepareEvent::Counted(counted) => ("counted", counted.result_count),
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

fn smart_folder_prepared_definition_matches(
    left: &crate::settings::SmartFolderDefinition,
    right: &crate::settings::SmartFolderDefinition,
) -> bool {
    left.id == right.id && left.rules == right.rules && left.grouping == right.grouping
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
    Bookmarks,
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
        SmartFolderMetadataDependency::Bookmarks => rule.filter.edits.iter().any(|flag| {
            matches!(
                flag,
                crate::settings::FacetEditFlag::Bookmarked
                    | crate::settings::FacetEditFlag::Unbookmarked
            )
        }),
    })
}

fn smart_folder_definition_needs_result_count(
    definition: &crate::settings::SmartFolderDefinition,
) -> bool {
    definition.rules.iter().any(|rule| {
        rule.enabled
            && (rule.filter.ratings != [true; 6]
                || !rule.filter.tags.is_empty()
                || rule.filter.include_untagged
                || !rule.filter.edits.is_empty())
    })
}

/// remote IPC がスマートフォルダの既存 scan / metadata filter / sort を再利用するための
/// 読み出し専用 read model。絶対 path はこの crate 内だけで保持し、IPC 化する前に
/// `remote_ipc::path_guard` のお気に入り境界へ写像する。
#[derive(Clone, Debug)]
pub(crate) struct RemoteSmartFolderEntry {
    pub(crate) path: PathBuf,
    pub(crate) kind: SmartFolderEntryKind,
    pub(crate) rating: Option<u8>,
}

/// UI の `open_smart_folder` と同じ候補走査・保存条件評価・表示順を、IPC worker 上で
/// 同期的に最後まで構築する。UI thread からは呼ばない。
pub(crate) fn build_remote_smart_folder_entries(
    settings: &crate::settings::Settings,
    definition: crate::settings::SmartFolderDefinition,
) -> Result<Vec<RemoteSmartFolderEntry>, String> {
    if active_rules(&definition).is_empty() {
        return Ok(Vec::new());
    }
    let cancel = AtomicBool::new(false);
    let io_sem = crate::io_semaphore::GlobalIoSemaphore::new(
        settings.indexer_speed_profile.io_permits().max(1),
    );
    let activity_gate = crate::activity_gate::ActivityGate::new(0);
    let (scan_tx, _scan_rx) = mpsc::channel();
    let scanned = scan_smart_folder(
        definition,
        SmartFolderScanOptions::from(settings),
        &cancel,
        &io_sem,
        &activity_gate,
        &scan_tx,
    )
    .ok_or_else(|| "スマートフォルダの走査が中断されました".to_owned())?;
    let snapshot = scanned.snapshot;
    let load_ratings = smart_folder_definition_uses_metadata(
        &snapshot.definition,
        SmartFolderMetadataDependency::Rating,
    );
    let load_tags = smart_folder_definition_uses_metadata(
        &snapshot.definition,
        SmartFolderMetadataDependency::Tags,
    );
    let load_local_adjust = smart_folder_definition_uses_metadata(
        &snapshot.definition,
        SmartFolderMetadataDependency::Edits,
    );
    let (prepare_tx, _prepare_rx) = mpsc::channel();
    let archive_cache = if crate::archive_cache::db_path()
        .try_exists()
        .unwrap_or(false)
    {
        crate::archive_cache::ArchiveCacheDb::open_readonly().ok()
    } else {
        None
    };
    let membership = evaluate_smart_folder_membership(
        &snapshot,
        &HashSet::new(),
        load_ratings,
        load_tags,
        load_local_adjust,
        None,
        archive_cache.as_ref(),
        true,
        &cancel,
        &prepare_tx,
    )?
    .ok_or_else(|| "スマートフォルダの評価が中断されました".to_owned())?;

    let sort = settings.sort_order;
    let display_order = settings.grid_display_order.normalized();
    let grouping = snapshot.definition.grouping;
    let sort_keys = build_smart_entry_sort_keys(
        &snapshot.entries,
        &membership.included,
        sort,
        &display_order,
    );
    let sorted_positions = super::recursive_snapshot_scan::cancelable_sorted_indices(
        membership.included.len(),
        &cancel,
        |a_position, b_position| {
            let a = &snapshot.entries[membership.included[a_position]];
            let b = &snapshot.entries[membership.included[b_position]];
            let ak = &sort_keys[a_position];
            let bk = &sort_keys[b_position];
            let within = || compare_smart_entries_within_group(sort, a, ak, b, bk);
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
        |_| {},
    )
    .ok_or_else(|| "スマートフォルダの並べ替えが中断されました".to_owned())?;

    Ok(sorted_positions
        .into_iter()
        .map(|position| {
            let entry_index = membership.included[position];
            let entry = &snapshot.entries[entry_index];
            RemoteSmartFolderEntry {
                path: entry.path.clone(),
                kind: entry.kind,
                rating: membership
                    .ratings
                    .get(&membership.keys[entry_index])
                    .copied(),
            }
        })
        .collect())
}

impl App {
    /// Move final drops for million-item smart-folder data away from the UI thread.
    ///
    /// The channel handoff is deliberate: if thread creation fails, the closure (and everything
    /// it captured) is destroyed by `Builder::spawn` on the caller.  Keeping the heavy values out
    /// of the closure until spawn succeeds prevents that failure path from recreating the stall.
    pub(crate) fn retire_smart_folder_payloads(
        &mut self,
        values: impl IntoIterator<Item = RetiredSmartFolderPayload>,
    ) {
        let mut retired = std::mem::take(&mut self.smart_folder_retired_payloads);
        retired.extend(values);
        if retired.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel::<Vec<RetiredSmartFolderPayload>>();
        match std::thread::Builder::new()
            .name("smart-folder-payload-drop".into())
            .spawn(move || {
                if let Ok(retired) = rx.recv() {
                    drop(retired);
                }
            }) {
            Ok(_thread) => {
                if let Err(error) = tx.send(retired) {
                    self.smart_folder_retired_payloads = error.0;
                }
            }
            Err(error) => {
                crate::logger::log(format!(
                    "smart_folder: failed to spawn payload drop worker; defer until next boundary: {error}"
                ));
                self.smart_folder_retired_payloads = retired;
            }
        }
    }

    fn retire_prepared_smart_folder(&mut self, prepared: PreparedSmartFolder) {
        self.retire_smart_folder_payloads(std::iter::once(
            Box::new(prepared) as RetiredSmartFolderPayload
        ));
    }

    #[allow(clippy::too_many_arguments)]
    fn retire_prepared_smart_folder_install_payload(
        &mut self,
        items: Vec<GridItem>,
        image_metas: Vec<Option<(i64, i64)>>,
        video_items: Vec<(usize, PathBuf, u64)>,
        metadata: super::subfolder_expansion::PreparedSubfolderMetadata,
        resort_metadata: Arc<ReusedSmartFolderMetadata>,
        applied_tombstones: HashSet<String>,
    ) {
        // These fields may collectively own several million rows. The tuple is intentionally
        // opaque: after a retry decision only its destructor is meaningful.
        let payload = Box::new((
            items,
            image_metas,
            video_items,
            metadata,
            resort_metadata,
            applied_tombstones,
        )) as RetiredSmartFolderPayload;
        self.retire_smart_folder_payloads(std::iter::once(payload));
    }

    /// Reject an in-flight prepare after a path-keyed metadata write. An installed session is a
    /// deliberately frozen result: direct cell state may still update, but membership/order and
    /// its reusable metadata remain unchanged until an explicit reopen.
    pub(crate) fn invalidate_smart_folder_resort_metadata(&mut self) {
        self.page_edit_revision = self.page_edit_revision.wrapping_add(1);
        if self.top_level_grid_view.smart_folder_session().is_some() {
            return;
        }
        self.smart_folder_metadata_revision = self.smart_folder_metadata_revision.wrapping_add(1);
    }

    /// A smart-folder scan/prepare owns the top-level grid.  Search modes and Snapshot Lock own
    /// the same surface, so they must be retired before a smart-folder generation is started.
    ///
    /// Search close is intentionally allowed to restore its saved origin first.  That restored
    /// real/synthetic location becomes the smart folder's history/return origin; the later smart
    /// install remains the only operation that replaces the grid.
    fn close_transient_views_before_smart_folder(
        &mut self,
    ) -> super::top_level_grid_view::TopLevelGridRestore {
        let mut return_context = self
            .current_top_level_restore_snapshot()
            .unwrap_or(super::top_level_grid_view::TopLevelGridRestore::Unavailable);
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
    ) -> Option<super::top_level_grid_view::TopLevelGridRestore> {
        let staged_search_return = (self.projected_viewer_context_id()
            == self.viewer_context_main())
        .then(|| self.smart_folder_transition.as_ref())
        .flatten()
        .filter(|transition| {
            matches!(transition.intent, SmartTransitionIntent::Return { .. })
                && match &transition.source.source {
                    SmartFolderSourceIdentity::Search { .. } => true,
                    // Local metadata search first dismisses its flat search overlay, so its
                    // pending Smart return is leased from the search-results marker in a
                    // Folder surface until adoption.
                    SmartFolderSourceIdentity::Folder(Some(path)) => {
                        crate::folder_tree::path_eq(path, &super::search_results_synthetic_path())
                    }
                    _ => false,
                }
        })
        .map(|transition| match &transition.target {
            SmartFolderTransitionTarget::Root(state)
            | SmartFolderTransitionTarget::Child { state, .. } => {
                super::top_level_grid_view::TopLevelGridRestore::SmartFolder(state.clone())
            }
        });
        let origin = self.smart_folder_open_origin.take();
        // A search-to-Smart return is still offscreen here. The next search inherits that typed
        // destination, while the abandoned request itself must not install over the new results.
        self.retire_staged_smart_navigation_for_independent_intent();
        self.cancel_smart_folder_pending();
        staged_search_return.or_else(|| origin.map(SmartFolderOpenOrigin::into_prior))
    }

    pub(crate) fn schedule_current_smart_folder_metadata_refresh(
        &mut self,
        dependency: SmartFolderMetadataDependency,
    ) {
        // Even when this metadata kind is not part of the definition's filter, the prepared grid
        // owns badges/adjustments/crops/pins that a later sort would otherwise roll back.
        self.invalidate_smart_folder_resort_metadata();
        if self.top_level_grid_view.smart_folder_session().is_some() {
            self.smart_folder_metadata_refresh_due = None;
            return;
        }
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
        if self.top_level_grid_view.smart_folder_session().is_some() {
            self.smart_folder_metadata_refresh_due = None;
            return;
        }
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
    }

    #[cfg(test)]
    pub(crate) fn open_smart_folder(&mut self, definition_id: uuid::Uuid, refresh: bool) {
        self.open_smart_folder_staged(definition_id, refresh);
    }

    /// Start a user-selected root open while retaining the currently visible owner. The scan,
    /// confirmation, and prepare run in the transition; only RootReady may replace the grid.
    pub(crate) fn open_smart_folder_staged(&mut self, definition_id: uuid::Uuid, refresh: bool) {
        let _ = self.begin_smart_folder_staged(definition_id, refresh);
    }

    pub(crate) fn begin_smart_folder_staged(
        &mut self,
        definition_id: uuid::Uuid,
        refresh: bool,
    ) -> Option<u64> {
        self.open_smart_folder_staged_with_intent(
            definition_id,
            refresh,
            SmartTransitionIntent::Direct(
                self.current_top_level_restore_snapshot()
                    .unwrap_or(super::top_level_grid_view::TopLevelGridRestore::Unavailable),
            ),
        )
    }

    pub(crate) fn refresh_smart_folder_staged(&mut self, definition_id: uuid::Uuid) -> Option<u64> {
        self.open_smart_folder_staged_with_intent(
            definition_id,
            true,
            SmartTransitionIntent::Refresh,
        )
    }

    fn open_smart_folder_staged_with_intent(
        &mut self,
        definition_id: uuid::Uuid,
        refresh: bool,
        intent: SmartTransitionIntent,
    ) -> Option<u64> {
        let Some(definition) = self
            .settings
            .smart_folders
            .iter()
            .find(|definition| definition.id == definition_id)
            .cloned()
        else {
            self.show_feedback_toast("スマートフォルダが見つかりません".into());
            return None;
        };
        if active_rules(&definition).is_empty() {
            self.show_feedback_toast("表示条件を追加してください".into());
            self.open_smart_folder_manager(Some(definition_id));
            return None;
        }
        match self.begin_smart_folder_transition_root(
            definition.clone(),
            refresh,
            intent,
            SmartFolderTransitionTarget::Root(
                super::top_level_grid_view::SmartFolderViewState::root(definition_id, Vec::new()),
            ),
        ) {
            Ok(request_id) => {
                self.show_feedback_toast(format!(
                    "スマートフォルダ「{}」を走査中",
                    definition.name
                ));
                Some(request_id)
            }
            Err(message) => {
                self.show_feedback_toast(message);
                None
            }
        }
    }

    pub(crate) fn smart_folder_busy(&self) -> bool {
        self.smart_folder_transition.is_some()
            || self.smart_folder_pending.is_some()
            || self.smart_folder_prepare_pending.is_some()
            || self.smart_folder_confirm_pending.is_some()
    }

    /// テスト専用: snapshot から prepare 済み状態を組み立てるセットアップ helper。
    ///
    /// production はセッション復帰で prepare をやり直さなくなったため、この既定引数の
    /// 入口を呼ぶ経路は残っていない (`start_smart_folder_prepare_inner` を各文脈が
    /// 直接呼ぶ)。`_inner` は private なので tests から直接は呼べず、この薄い wrapper を
    /// `#[cfg(test)]` で残す。**production から呼びたくなったら、それは
    /// 「セッションを無視して prepare し直している」合図なので、まず §2.3 の方針を確認する。**
    #[cfg(test)]
    pub(crate) fn start_smart_folder_prepare(
        &mut self,
        snapshot: SmartFolderSnapshot,
        refresh: bool,
    ) {
        self.start_smart_folder_prepare_inner(snapshot, refresh, false, HashSet::new(), None, None);
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
        self.start_smart_folder_prepare_inner(
            snapshot,
            refresh,
            true,
            tombstones_at_start,
            None,
            None,
        );
    }

    fn start_smart_folder_prepare_after_count(
        &mut self,
        snapshot: SmartFolderSnapshot,
        refresh: bool,
        tombstones_at_start: HashSet<String>,
        membership: EvaluatedSmartFolderMembership,
    ) {
        self.start_smart_folder_prepare_inner(
            snapshot,
            refresh,
            true,
            tombstones_at_start,
            Some(membership),
            None,
        );
    }

    fn start_smart_folder_result_count_after_scan(
        &mut self,
        snapshot: SmartFolderSnapshot,
        refresh: bool,
        tombstones_at_start: HashSet<String>,
    ) {
        let retired =
            cancelled_smart_folder_payloads(None, self.smart_folder_prepare_pending.take(), None);
        self.retire_smart_folder_payloads(retired);
        let generation = self.smart_folder_generation;
        let definition_id = snapshot.definition.id;
        let current_tombstones = self
            .smart_folder_removed_paths
            .get(&definition_id)
            .cloned()
            .unwrap_or_default();
        let removed_paths =
            smart_folder_tombstones_after_scan_start(&current_tombstones, &tombstones_at_start);
        match spawn_smart_folder_result_count(
            snapshot,
            generation,
            self.smart_folder_metadata_revision,
            refresh,
            tombstones_at_start,
            removed_paths,
            self.rating_db.is_some(),
            self.tags_db.is_some(),
            self.local_adjust_db.is_some(),
            self.archive_cache_db.clone(),
        ) {
            Ok(pending) => {
                self.smart_folder_progress = Some(SmartFolderProgress {
                    phase: SmartFolderPhase::Filtering,
                    ..SmartFolderProgress::default()
                });
                self.smart_folder_prepare_pending = Some(pending);
            }
            Err(message) => {
                self.show_feedback_toast(message);
                self.restore_pending_smart_folder_origin();
            }
        }
    }

    fn start_smart_folder_prepare_inner(
        &mut self,
        snapshot: SmartFolderSnapshot,
        refresh: bool,
        authoritative_rescan: bool,
        authoritative_ignored_tombstones: HashSet<String>,
        precounted_membership: Option<EvaluatedSmartFolderMembership>,
        reused_metadata: Option<Arc<ReusedSmartFolderMetadata>>,
    ) {
        if self.items_are_smart_folder_view && self.show_search_bar {
            self.smart_folder_local_search_reapply = Some(self.search_query.clone());
            self.cancel_search_pending();
        }
        let retired =
            cancelled_smart_folder_payloads(None, self.smart_folder_prepare_pending.take(), None);
        self.retire_smart_folder_payloads(retired);
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
            precounted_membership,
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
        let retired = cancelled_smart_folder_payloads(
            self.smart_folder_pending.take(),
            self.smart_folder_prepare_pending.take(),
            self.smart_folder_confirm_pending.take(),
        );
        self.retire_smart_folder_payloads(retired);
        self.smart_folder_progress = None;
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

    pub(crate) fn cancel_smart_folder_pending_and_restore_origin(&mut self) {
        // A staged request has not changed the source surface, so cancellation retires only its
        // offscreen workers and leaves the visible owner and history untouched.
        self.retire_smart_folder_transition();
        let origin = self.smart_folder_open_origin.take();
        self.cancel_smart_folder_pending();
        self.restore_cancelled_smart_folder_origin(origin);
        self.reprepare_visible_smart_root_after_staged_terminal();
    }

    fn restore_pending_smart_folder_origin(&mut self) {
        let origin = self.smart_folder_open_origin.take();
        self.restore_cancelled_smart_folder_origin(origin);
    }

    fn restore_cancelled_smart_folder_origin(&mut self, origin: Option<SmartFolderOpenOrigin>) {
        let Some(origin) = origin else {
            return;
        };
        self.restore_cancelled_smart_folder_prior(origin.into_prior());
    }

    fn restore_cancelled_smart_folder_prior(
        &mut self,
        origin: super::top_level_grid_view::TopLevelGridRestore,
    ) {
        if let super::top_level_grid_view::TopLevelGridRestore::SmartFolder(state) = &origin {
            // Direct switching and refresh leave the previous installed grid in memory until the
            // replacement succeeds. Restore that owner without starting another prepare/scan.
            if self.items_are_smart_folder_view
                && self.current_smart_folder_id == Some(state.definition_id)
            {
                self.top_level_grid_view.replace_surface(
                    super::top_level_grid_view::TopLevelGridSurface::SmartFolder(state.clone()),
                );
                return;
            }

            // A history/search restore can set the target SmartFolder surface before a cache-miss
            // scan starts. In that state the target itself can accidentally become its own cancel
            // origin. Calling the generic restore path would immediately start the same scan
            // again, making the modal impossible to escape.
            let can_restore_without_scan = self
                .settings
                .smart_folders
                .iter()
                .find(|definition| definition.id == state.definition_id)
                .and_then(|definition| {
                    self.top_level_grid_view
                        .smart_folder_session()
                        .and_then(SmartFolderSession::root_snapshot)
                        .filter(|snapshot| {
                            smart_folder_prepared_definition_matches(
                                &snapshot.definition,
                                definition,
                            )
                        })
                })
                .is_some();
            if !can_restore_without_scan {
                let fallback = self
                    .smart_folder_saved_folder
                    .clone()
                    .or_else(|| self.effective_folder())
                    .filter(|path| !is_synthetic_view_path(path));
                self.clear_smart_folder_view_state();
                if let Some(path) = fallback {
                    self.load_folder(path);
                } else {
                    self.enter_drive_list(None);
                }
                return;
            }
        }
        self.restore_view_return_context(origin);
    }

    /// A definition edit retires unadopted work for that definition. The visible session and
    /// delete tombstones remain owned by the old display until a refreshed result is adopted.
    fn invalidate_smart_folder_definition_state(&mut self, definition_id: uuid::Uuid) {
        if self
            .smart_folder_transition
            .as_ref()
            .is_some_and(|transition| match &transition.target {
                SmartFolderTransitionTarget::Root(state) => state.definition_id == definition_id,
                SmartFolderTransitionTarget::Child { state, .. } => {
                    state.definition_id == definition_id
                }
            })
        {
            self.retire_smart_folder_transition();
        }
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
            let _ = self.refresh_smart_folder_staged(definition_id);
        }
    }

    /// 表示中の定義が削除された場合は orphan synthetic view を残さず元の場所へ戻す。
    pub(crate) fn forget_smart_folder_definition(&mut self, definition_id: uuid::Uuid) {
        self.invalidate_smart_folder_definition(definition_id);
        self.smart_folder_removed_paths.remove(&definition_id);
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
        if self.top_level_grid_view.smart_folder().is_some() {
            self.top_level_grid_view
                .replace_surface(super::top_level_grid_view::TopLevelGridSurface::Folder);
        }
    }

    /// Consume the one typed authorization created by an in-session grid/parent/Ctrl-navigation
    /// action. An ordinary address/favourite/folder load has no authorization and therefore lets
    /// the `TopLevelGridView` owner discard the session at the common load boundary.
    pub(crate) fn smart_folder_session_owns_load(&self, path: &Path) -> bool {
        self.top_level_grid_view
            .smart_folder_session()
            .is_some_and(|session| session.owns_load(path, self.archive_source_override.as_deref()))
    }

    pub(crate) fn smart_folder_opening_pending(&self) -> bool {
        self.smart_folder_transition.is_some()
    }

    /// An explicit presentation edit may happen while a staged child leaves the old root on
    /// screen. If that request ends without adopting a child, bring that still-visible root up
    /// to date once the transition no longer owns its presentation.
    fn reprepare_visible_smart_root_after_staged_terminal(&mut self) {
        if self.smart_folder_transition.is_some() || !self.items_are_smart_folder_view {
            return;
        }
        let stale = self
            .top_level_grid_view
            .smart_folder_session()
            .filter(|session| matches!(session.phase, SmartFolderOpenPhase::Root))
            .and_then(|session| {
                self.settings
                    .smart_folders
                    .iter()
                    .find(|definition| definition.id == session.definition_id)
                    .map(|definition| {
                        session.presentation
                            != SmartFolderPresentation::current(self, definition.grouping)
                    })
            })
            .unwrap_or(false);
        if stale {
            self.reprepare_current_smart_folder_for_sort();
        }
    }

    pub(crate) fn abort_smart_archive_open_for_owner(
        &mut self,
        owner: &super::OpenRequestOwner,
    ) -> bool {
        let super::OpenRequestOwner::MainGridArchive(intent) = owner else {
            return false;
        };
        let super::SmartGridArchiveOwner::Transition(request_id) = intent.smart_folder_owner else {
            return false;
        };
        let staged = self.smart_folder_transition.as_ref().is_some_and(|transition| {
            transition.request_id == request_id
                &&
            matches!(&transition.target, SmartFolderTransitionTarget::Child { source, .. }
                if crate::folder_tree::path_eq(&source.logical_source, &intent.source_path))
        });
        if staged {
            self.retire_smart_folder_transition();
            self.reprepare_visible_smart_root_after_staged_terminal();
        }
        staged
    }

    /// A same-child refresh keeps the settled Smart session. New child navigation enters through
    /// `SmartFolderTransition` and receives explicit visible-install authority at adoption.
    pub(crate) fn preserve_smart_folder_session_for_load(&self, path: &Path) -> bool {
        self.smart_folder_session_owns_load(path)
    }

    /// Move the currently displayed grid into one owner before replacing its item generation.
    /// The same payload is used for the resident Smart root and for a no-resident history
    /// continuation's prior view; neither path clones the potentially very large item list.
    fn take_visible_smart_folder_grid(&mut self) -> SmartFolderPreparedGrid {
        self.persist_pending_view_trim_state();
        let anchor_index = self
            .grid_click_selection_anchor
            .and_then(|anchor| anchor.index_for_generation(self.items_generation));
        let grid = SmartFolderPreparedGrid {
            items: std::mem::take(&mut self.items),
            thumbnails: std::mem::take(&mut self.thumbnails),
            image_metas: std::mem::take(&mut self.image_metas),
            video_thumb_overrides: std::mem::take(&mut self.video_thumb_overrides),
            selected: self.selected.take(),
            grid_click_selection_anchor_index: anchor_index,
            layout: SmartFolderPreparedGridLayout {
                scroll_offset_y: std::mem::take(&mut self.scroll_offset_y),
                auto_aspect: std::mem::take(&mut self.auto_aspect),
                mode: SmartFolderPreparedGridMode::current(&self.settings),
                window_inner_size: self.last_inner_size,
            },
            scroll_to_selected: std::mem::take(&mut self.scroll_to_selected),
            pending_grid_scroll: self.pending_grid_scroll.take(),
            checked: std::mem::take(&mut self.checked),
            visible_indices: std::mem::take(&mut self.visible_indices),
            details_order: std::mem::take(&mut self.details_order),
            show_search_bar: std::mem::take(&mut self.show_search_bar),
            search_query: std::mem::take(&mut self.search_query),
            search_filter: self.search_filter.take(),
            search_filter_origin_folder: self.search_filter_origin_folder.take(),
            rating_cache: std::mem::take(&mut self.rating_cache),
            tags_cache: std::mem::take(&mut self.tags_cache),
            local_adjust_pages: std::mem::take(&mut self.local_adjust_pages),
            adjustment_page_params: std::mem::take(&mut self.adjustment_page_params),
            export_crop_page_settings: std::mem::take(&mut self.export_crop_page_settings),
            view_trim_page_overrides: std::mem::take(&mut self.view_trim_page_overrides),
            mask_pages: std::mem::take(&mut self.mask_pages),
            conceal_pages: std::mem::take(&mut self.conceal_pages),
            comic_pages: std::mem::take(&mut self.comic_pages),
            folder_pin_map: std::mem::take(&mut self.folder_pin_map),
            converted_archive_cache_paths: std::mem::take(&mut self.converted_archive_cache_paths),
            color_cache_map: self.current_color_cache_map.take(),
            color_catalog: self.current_color_catalog.take(),
        };
        self.invalidate_facet_name_cache();
        grid
    }

    fn restore_prepared_smart_folder_session(
        &mut self,
        definition_id: uuid::Uuid,
        returned_root_entry: Option<&Path>,
    ) -> bool {
        let Some(mut session) = self.top_level_grid_view.take_smart_folder_session() else {
            return false;
        };
        if session.definition_id != definition_id {
            self.top_level_grid_view
                .install_smart_folder_session(session);
            return false;
        }
        let Some(root) =
            std::mem::replace(&mut session.phase, SmartFolderOpenPhase::Root).into_parked_root()
        else {
            self.top_level_grid_view
                .install_smart_folder_session(session);
            return false;
        };
        let grid = match root {
            SmartFolderRootPayload::Visible(grid) => grid,
            SmartFolderRootPayload::Offscreen(prepared) => {
                // No root grid was ever published on this history continuation. Materialize the
                // saved worker result once now, with the entered root row as the stable anchor.
                // `SmartFolderSession::Drop` retires its snapshot off the UI thread.
                drop(session);
                self.smart_folder_open_origin = Some(SmartFolderOpenOrigin::ReturnReprepare {
                    prior: super::top_level_grid_view::TopLevelGridRestore::SmartFolder(
                        super::top_level_grid_view::SmartFolderViewState::root(
                            definition_id,
                            Vec::new(),
                        ),
                    ),
                    anchor: returned_root_entry.map(Path::to_path_buf),
                });
                return self.install_prepared_smart_folder(*prepared);
            }
        };

        self.cancel_media_navigation_pending_for_current_context(
            "restore_prepared_smart_folder_session",
        );
        self.close_fullscreen();
        self.bump_full_context_for_load();
        self.cancel_pending_folder_nav();
        self.pdf_enumerate_pending = None;
        self.zip_enumerate_pending = None;
        self.fs_nav_after_pdf_enumerate = None;
        self.pdf_placeholder_count = None;
        self.virtual_folder_writeback = None;
        self.pdf_prefetch_grace_until = None;
        self.zip_nav = None;
        self.archive_source_override = None;
        crate::zip_loader::clear_nested_cache();

        let SmartFolderPreparedGrid {
            items,
            thumbnails,
            image_metas,
            video_thumb_overrides,
            selected,
            grid_click_selection_anchor_index,
            layout,
            scroll_to_selected,
            pending_grid_scroll,
            checked,
            visible_indices,
            details_order,
            show_search_bar,
            search_query,
            search_filter,
            search_filter_origin_folder,
            rating_cache,
            tags_cache,
            local_adjust_pages,
            adjustment_page_params,
            export_crop_page_settings,
            view_trim_page_overrides,
            mask_pages,
            conceal_pages,
            comic_pages,
            folder_pin_map,
            converted_archive_cache_paths,
            color_cache_map,
            color_catalog,
        } = grid;
        let layout_changed = !layout.matches_current_layout(self);
        let SmartFolderPreparedGridLayout {
            scroll_offset_y,
            mut auto_aspect,
            ..
        } = layout;
        let synthetic = smart_folder_synthetic_path(definition_id);
        self.current_folder = Some(synthetic.clone());
        self.current_folder_last_mtime = None;
        self.current_folder_signature = None;
        self.items = items;
        self.thumbnails = thumbnails;
        self.image_metas = image_metas;
        self.bump_items_generation();
        self.invalidate_idx_state_and_queues();
        // The restored samples still describe these moved items, but their installed App
        // generation is new so stale child work cannot publish into the root.
        auto_aspect.items_generation = self.items_generation;
        self.auto_aspect = auto_aspect;

        self.video_thumb_overrides = video_thumb_overrides;
        self.selected = selected;
        self.grid_click_selection_anchor = grid_click_selection_anchor_index
            .map(|index| super::GridClickSelectionAnchor::new(index, self.items_generation));
        self.scroll_offset_y = scroll_offset_y;
        self.scroll_to_selected = scroll_to_selected || layout_changed;
        self.pending_grid_scroll = pending_grid_scroll;
        self.checked = checked;
        self.visible_indices = visible_indices;
        self.details_order = details_order;
        self.show_search_bar = show_search_bar;
        self.search_query = search_query;
        self.search_filter = search_filter;
        self.search_filter_origin_folder = search_filter_origin_folder;
        self.rating_cache = rating_cache;
        self.replace_tags_cache(tags_cache);
        self.local_adjust_pages = local_adjust_pages;
        self.adjustment_page_params = adjustment_page_params;
        self.export_crop_page_settings = export_crop_page_settings;
        self.export_crop_pages = self.export_crop_page_settings.keys().copied().collect();
        self.view_trim_page_overrides = view_trim_page_overrides;
        self.mask_pages = mask_pages;
        self.conceal_pages = conceal_pages;
        self.comic_pages = comic_pages;
        self.folder_pin_map = folder_pin_map;
        self.converted_archive_cache_paths = converted_archive_cache_paths;
        self.current_color_cache_map = color_cache_map;
        self.current_color_catalog = color_catalog;

        let (tx, rx) = mpsc::channel();
        self.tx = tx.clone();
        self.rx = rx;
        if let Some(cache_map) = self.current_color_cache_map.clone() {
            let reload_queue: Arc<NotifyQueue> =
                Arc::new((Mutex::new(Vec::new()), std::sync::Condvar::new()));
            let heavy_io_queue: Arc<NotifyQueue> =
                Arc::new((Mutex::new(Vec::new()), std::sync::Condvar::new()));
            self.reload_queue = Some(Arc::clone(&reload_queue));
            self.heavy_io_queue = Some(Arc::clone(&heavy_io_queue));
            self.spawn_thumbnail_workers(
                &tx,
                Arc::clone(&self.cancel_token),
                reload_queue,
                heavy_io_queue,
                cache_map,
                self.current_color_catalog.clone(),
                self.folder_thumb_pin_db.clone(),
            );
        } else {
            self.reload_queue = None;
            self.heavy_io_queue = None;
        }
        let video_items =
            crate::filename_stack_ui::stack_video_items(&self.items, &self.image_metas);
        if !video_items.is_empty() {
            self.spawn_video_thread(
                tx,
                Arc::clone(&self.cancel_token),
                video_items,
                self.video_thumb_overrides.clone(),
                Arc::new(HashMap::new()),
            );
        }
        self.items_are_global_search_view = false;
        self.items_are_tag_view = false;
        self.items_are_reading_history_view = false;
        self.items_are_bookmark_view = false;
        self.items_are_rating_view = false;
        self.items_are_subfolder_expansion_view = false;
        self.items_are_drive_list = false;
        self.items_are_smart_folder_view = true;
        // prepared grid の復元は start_loading_items を通らないため、Rating 一覧だけで
        // 有効な ★設定時刻ソートをこの所有境界で解放して Toolbar 順を再構築する。
        self.reset_details_sort_if_hidden();
        self.current_smart_folder_id = Some(definition_id);
        self.smart_folder_progress = None;

        let definition_name = self
            .settings
            .smart_folders
            .iter()
            .find(|definition| definition.id == definition_id)
            .map(|definition| definition.name.as_str())
            .unwrap_or("?");
        self.address = format!("スマートフォルダ: {definition_name}");
        self.last_scroll_offset_y_tracked = self.scroll_offset_y;
        if let Some(returned_root_entry) = returned_root_entry {
            let returned_index = self.items.iter().position(|item| {
                item.drag_source_path()
                    .is_some_and(|path| crate::folder_tree::path_eq(path, returned_root_entry))
            });
            match returned_index {
                Some(index)
                    if self.selected != Some(index)
                        || self.visible_indices.binary_search(&index).is_err() =>
                {
                    self.select_item_after_load(index);
                }
                Some(_) => {}
                None => {
                    // The active entry was deleted or ceased to belong to the frozen root.
                    // Match ordinary select-after-load's miss behavior: keep the prepared
                    // selection, repairing it only if that row is no longer visible.
                    self.redirect_selected_to_visible();
                }
            }
        }
        session.returned_to_root();
        self.top_level_grid_view
            .install_smart_folder_session(session);
        true
    }

    pub(crate) fn start_smart_folder_scope_nav(&mut self, forward: bool, fullscreen: bool) -> bool {
        let Some(state) = self.top_level_grid_view.smart_folder().cloned() else {
            return false;
        };
        let current = state
            .scoped_current()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| smart_folder_synthetic_path(state.definition_id));
        if fullscreen {
            self.mark_smart_folder_navigation_sequence();
        }
        self.start_folder_nav(
            current,
            forward,
            super::FolderNavMode::SmartFolder { state, fullscreen },
        );
        true
    }

    pub(crate) fn smart_folder_parent_nav_target(&self) -> Option<crate::ui_main::AddressBarNav> {
        let state = self.top_level_grid_view.smart_folder()?;
        let target = state.parent_target();
        if target.is_none()
            && self
                .top_level_grid_view
                .smart_folder_session()
                .is_some_and(SmartFolderSession::root_parent_target)
        {
            return Some(crate::ui_main::AddressBarNav::Direct(
                smart_folder_synthetic_path(state.definition_id),
            ));
        }
        match target? {
            super::top_level_grid_view::SmartFolderParentTarget::Root => {
                Some(crate::ui_main::AddressBarNav::Direct(
                    smart_folder_synthetic_path(state.definition_id),
                ))
            }
            super::top_level_grid_view::SmartFolderParentTarget::Folder(path) => {
                Some(crate::ui_main::AddressBarNav::Direct(path))
            }
        }
    }

    pub(crate) fn restore_smart_folder_view_state(
        &mut self,
        state: super::top_level_grid_view::SmartFolderViewState,
    ) {
        let _ = self.restore_smart_folder_view_state_with_history(state, &mut None);
    }

    pub(crate) fn restore_smart_folder_view_state_with_history(
        &mut self,
        state: super::top_level_grid_view::SmartFolderViewState,
        history_rollback: &mut Option<super::FolderNavHistorySnapshot>,
    ) -> bool {
        if let Some(snapshot) = history_rollback.take() {
            // An older programmatic history dispatcher already popped this target. Restore its
            // pre-pop stack and let the same typed peek owner commit only after visible adoption.
            self.restore_folder_nav_history(snapshot);
            let target = super::FolderNavHistoryTarget::SmartFolder(state.clone());
            let direction = if self
                .folder_history_back_target()
                .is_some_and(|candidate| Self::folder_nav_targets_eq(candidate, &target))
            {
                Some(SmartHistoryDirection::Back)
            } else if self
                .folder_history_forward_target()
                .is_some_and(|candidate| Self::folder_nav_targets_eq(candidate, &target))
            {
                Some(SmartHistoryDirection::Forward)
            } else {
                None
            };
            return direction
                .is_some_and(|direction| self.begin_smart_history_navigation(state, direction));
        }
        self.begin_staged_smart_target_navigation(
            state,
            SmartTransitionIntent::Return { anchor: None },
        )
    }

    pub(crate) fn update_smart_folder_scoped_address(&mut self) {
        let Some(state) = self.top_level_grid_view.smart_folder() else {
            return;
        };
        let Some(current) = state.scoped_current() else {
            return;
        };
        let Some(name) = self
            .settings
            .smart_folders
            .iter()
            .find(|definition| definition.id == state.definition_id)
            .map(|definition| definition.name.clone())
        else {
            return;
        };
        let entry_root = state.scoped_entry_root().unwrap_or(current);
        let relative = current.strip_prefix(entry_root).ok();
        let mut segments = vec![name];
        if let Some(entry_name) = entry_root.file_name().and_then(|value| value.to_str()) {
            segments.push(entry_name.to_string());
        } else {
            segments.push(entry_root.display().to_string());
        }
        if let Some(relative) = relative {
            segments.extend(
                relative
                    .components()
                    .filter_map(|component| component.as_os_str().to_str())
                    .map(str::to_string),
            );
        }
        self.address = segments.join(" > ");
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
        if !self.resident_smart_root_restore_is_exact(&definition) {
            let state = self
                .top_level_grid_view
                .smart_folder()
                .filter(|state| state.definition_id == definition_id);
            let anchor = state.and_then(|state| {
                self.top_level_grid_view
                    .smart_folder_session()
                    .and_then(|session| session.active_root_entry(state))
            });
            let navigation_entries = state
                .map(|state| state.navigation_entries.as_ref().clone())
                .unwrap_or_default();
            return self.begin_staged_smart_target_navigation(
                super::top_level_grid_view::SmartFolderViewState::root_with_navigation_entries(
                    definition_id,
                    navigation_entries,
                ),
                SmartTransitionIntent::Return { anchor },
            );
        }
        let retained_navigation_entries = self
            .top_level_grid_view
            .smart_folder()
            .filter(|state| state.definition_id == definition_id)
            .map(|state| state.navigation_entries.as_ref().clone())
            .unwrap_or_default();
        let returned_root_entry = self
            .top_level_grid_view
            .smart_folder()
            .filter(|state| state.definition_id == definition_id)
            .and_then(|state| {
                self.top_level_grid_view
                    .smart_folder_session()
                    .and_then(|session| session.active_root_entry(state))
            });
        let root_state =
            super::top_level_grid_view::SmartFolderViewState::root_with_navigation_entries(
                definition_id,
                retained_navigation_entries,
            );
        let synthetic = smart_folder_synthetic_path(definition_id);
        // The physical child's favorite is captured before comparing the parked root with
        // current settings. Otherwise frame-end reconciliation sees a spurious sort change and
        // replaces the exact moved grid with a fresh first-row prepare.
        self.transition_favorite_view_for_path(Some(&synthetic));
        self.record_smart_folder_scope_transition(&root_state);
        let root_restore =
            super::top_level_grid_view::TopLevelGridRestore::SmartFolder(root_state.clone());
        self.top_level_grid_view.replace_surface(
            super::top_level_grid_view::TopLevelGridSurface::SmartFolder(root_state),
        );
        let offscreen_root_matches = self
            .top_level_grid_view
            .smart_folder_session()
            .and_then(|session| match session.phase.visible() {
                SmartFolderOpenPhase::Child {
                    parked_root: SmartFolderRootPayload::Offscreen(prepared),
                    ..
                } => Some(
                    smart_folder_prepared_definition_matches(
                        &prepared.snapshot.definition,
                        &definition,
                    ) && prepared.presentation
                        == SmartFolderPresentation::current(self, definition.grouping)
                        && prepared.metadata_revision == self.smart_folder_metadata_revision,
                ),
                _ => None,
            })
            .unwrap_or(false);
        if offscreen_root_matches
            && self.restore_prepared_smart_folder_session(
                definition_id,
                returned_root_entry.as_deref(),
            )
        {
            return true;
        }
        let reusable_snapshot = self
            .top_level_grid_view
            .smart_folder_session()
            .and_then(SmartFolderSession::root_snapshot)
            .filter(|snapshot| smart_folder_scan_rules_match(&snapshot.definition, &definition))
            .cloned();
        if let Some(mut snapshot) = reusable_snapshot {
            let presentation_matches =
                self.top_level_grid_view
                    .smart_folder_session()
                    .is_some_and(|session| {
                        session.presentation
                            == SmartFolderPresentation::current(self, definition.grouping)
                    });
            if self.restore_prepared_smart_folder_session(
                definition_id,
                returned_root_entry.as_deref(),
            ) {
                if !presentation_matches {
                    adopt_smart_folder_presentation(&mut snapshot.definition, &definition);
                    self.smart_folder_open_origin = Some(SmartFolderOpenOrigin::ReturnReprepare {
                        prior: root_restore,
                        anchor: returned_root_entry,
                    });
                    self.start_smart_folder_prepare_inner(
                        snapshot,
                        false,
                        false,
                        HashSet::new(),
                        None,
                        None,
                    );
                }
                return true;
            }
            if self.items_are_smart_folder_view
                && self.current_smart_folder_id == Some(definition_id)
            {
                self.address = format!("スマートフォルダ: {}", definition.name);
                return true;
            }
        } else {
            self.top_level_grid_view
                .discard_smart_folder_session(definition_id);
        }
        self.begin_staged_smart_target_navigation(
            super::top_level_grid_view::SmartFolderViewState::root(definition_id, Vec::new()),
            SmartTransitionIntent::Return {
                anchor: returned_root_entry,
            },
        )
    }

    pub(crate) fn poll_smart_folder(&mut self, ctx: &egui::Context) {
        self.poll_smart_folder_transition_root(ctx);
        // Retain accepted scan/count/prepare results while the App-global sidecar coordinator owns
        // a load continuation. Normal polling consumes the same payload exactly once afterwards.
        if self.sidecar_restore_active() {
            return;
        }
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
                        self.retire_smart_folder_payloads(std::iter::once(
                            Box::new(result) as RetiredSmartFolderPayload
                        ));
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
                        self.retire_smart_folder_payloads([
                            Box::new(pending) as RetiredSmartFolderPayload,
                            Box::new(result) as RetiredSmartFolderPayload,
                        ]);
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
                        self.retire_smart_folder_payloads([
                            Box::new(pending) as RetiredSmartFolderPayload,
                            Box::new(result) as RetiredSmartFolderPayload,
                        ]);
                    } else if result.snapshot.entries.len() >= SMART_FOLDER_CONFIRM_THRESHOLD
                        && smart_folder_definition_needs_result_count(&result.snapshot.definition)
                    {
                        self.start_smart_folder_result_count_after_scan(
                            result.snapshot,
                            pending.refresh,
                            pending.tombstones_at_start,
                        );
                    } else if result.snapshot.entries.len() >= SMART_FOLDER_CONFIRM_THRESHOLD {
                        let result_count = result.snapshot.entries.len();
                        let replaced =
                            self.smart_folder_confirm_pending
                                .replace(SmartFolderConfirmPending {
                                    snapshot: result.snapshot,
                                    result_count,
                                    membership: None,
                                    membership_revision: None,
                                    generation: pending.generation,
                                    refresh: pending.refresh,
                                    tombstones_at_start: pending.tombstones_at_start,
                                });
                        self.retire_smart_folder_payloads(
                            replaced.map(|confirm| Box::new(confirm) as RetiredSmartFolderPayload),
                        );
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
                    let retired = cancelled_smart_folder_payloads(
                        self.smart_folder_pending.take(),
                        None,
                        None,
                    );
                    self.retire_smart_folder_payloads(retired);
                    self.smart_folder_progress = None;
                    self.restore_pending_smart_folder_origin();
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let retired = cancelled_smart_folder_payloads(
                        self.smart_folder_pending.take(),
                        None,
                        None,
                    );
                    self.retire_smart_folder_payloads(retired);
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
                Ok(SmartFolderPrepareEvent::Counted(counted)) => {
                    let mut counted = *counted;
                    let Some(pending) = self.smart_folder_prepare_pending.take() else {
                        self.retire_smart_folder_payloads(std::iter::once(
                            Box::new(counted) as RetiredSmartFolderPayload
                        ));
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
                                    &counted.snapshot.definition,
                                )
                        })
                        .cloned();
                    let Some(current_definition) = current_definition else {
                        self.smart_folder_progress = None;
                        self.retire_smart_folder_payloads([
                            Box::new(pending) as RetiredSmartFolderPayload,
                            Box::new(counted) as RetiredSmartFolderPayload,
                        ]);
                        break;
                    };
                    if pending.generation != self.smart_folder_generation
                        || pending.definition_id != counted.snapshot.definition.id
                    {
                        self.smart_folder_progress = None;
                        self.retire_smart_folder_payloads([
                            Box::new(pending) as RetiredSmartFolderPayload,
                            Box::new(counted) as RetiredSmartFolderPayload,
                        ]);
                        break;
                    }
                    adopt_smart_folder_presentation(
                        &mut counted.snapshot.definition,
                        &current_definition,
                    );
                    if counted.metadata_revision != self.smart_folder_metadata_revision {
                        self.start_smart_folder_result_count_after_scan(
                            counted.snapshot,
                            counted.refresh,
                            counted.tombstones_at_start,
                        );
                    } else if counted.result_count >= SMART_FOLDER_CONFIRM_THRESHOLD {
                        let replaced =
                            self.smart_folder_confirm_pending
                                .replace(SmartFolderConfirmPending {
                                    snapshot: counted.snapshot,
                                    result_count: counted.result_count,
                                    membership: Some(counted.membership),
                                    membership_revision: Some(counted.metadata_revision),
                                    generation: pending.generation,
                                    refresh: counted.refresh,
                                    tombstones_at_start: counted.tombstones_at_start,
                                });
                        self.retire_smart_folder_payloads(
                            replaced.map(|confirm| Box::new(confirm) as RetiredSmartFolderPayload),
                        );
                    } else {
                        self.start_smart_folder_prepare_after_count(
                            counted.snapshot,
                            counted.refresh,
                            counted.tombstones_at_start,
                            counted.membership,
                        );
                    }
                    break;
                }
                Ok(SmartFolderPrepareEvent::Done(prepared)) => {
                    let prepared = *prepared;
                    let Some(pending) = self.smart_folder_prepare_pending.take() else {
                        self.retire_prepared_smart_folder(prepared);
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
                    let Some(current_definition) = current_definition else {
                        self.retire_prepared_smart_folder(prepared);
                        break;
                    };
                    if pending.generation != self.smart_folder_generation
                        || pending.definition_id != prepared.snapshot.definition.id
                    {
                        self.retire_prepared_smart_folder(prepared);
                        break;
                    }
                    if prepared.presentation
                        != SmartFolderPresentation::current(self, current_definition.grouping)
                    {
                        let PreparedSmartFolder {
                            mut snapshot,
                            presentation: _,
                            items,
                            image_metas,
                            video_items,
                            metadata,
                            resort_metadata,
                            metadata_revision: _,
                            refresh,
                            authoritative_rescan,
                            authoritative_ignored_tombstones,
                            applied_tombstones,
                        } = prepared;
                        adopt_smart_folder_presentation(
                            &mut snapshot.definition,
                            &current_definition,
                        );
                        self.retire_prepared_smart_folder_install_payload(
                            items,
                            image_metas,
                            video_items,
                            metadata,
                            resort_metadata,
                            applied_tombstones,
                        );
                        self.start_smart_folder_prepare_inner(
                            snapshot,
                            refresh,
                            authoritative_rescan,
                            authoritative_ignored_tombstones,
                            None,
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
                    break;
                }
                Ok(SmartFolderPrepareEvent::Cancelled) => {
                    let retired = cancelled_smart_folder_payloads(
                        None,
                        self.smart_folder_prepare_pending.take(),
                        None,
                    );
                    self.retire_smart_folder_payloads(retired);
                    self.smart_folder_progress = None;
                    break;
                }
                Ok(SmartFolderPrepareEvent::Error(message)) => {
                    let retired = cancelled_smart_folder_payloads(
                        None,
                        self.smart_folder_prepare_pending.take(),
                        None,
                    );
                    self.retire_smart_folder_payloads(retired);
                    self.smart_folder_progress = None;
                    self.show_feedback_toast(message);
                    self.restore_pending_smart_folder_origin();
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let retired = cancelled_smart_folder_payloads(
                        None,
                        self.smart_folder_prepare_pending.take(),
                        None,
                    );
                    self.retire_smart_folder_payloads(retired);
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
            presentation,
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
            self.retire_prepared_smart_folder_install_payload(
                items,
                image_metas,
                video_items,
                metadata,
                resort_metadata,
                applied_tombstones,
            );
            if authoritative_rescan
                && snapshot.entries.len() >= SMART_FOLDER_CONFIRM_THRESHOLD
                && smart_folder_definition_needs_result_count(&snapshot.definition)
            {
                self.start_smart_folder_result_count_after_scan(
                    snapshot,
                    refresh,
                    authoritative_ignored_tombstones,
                );
            } else {
                self.start_smart_folder_prepare_inner(
                    snapshot,
                    refresh,
                    authoritative_rescan,
                    authoritative_ignored_tombstones,
                    None,
                    None,
                );
            }
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
                self.retire_prepared_smart_folder_install_payload(
                    items,
                    image_metas,
                    video_items,
                    metadata,
                    resort_metadata,
                    applied_tombstones,
                );
                self.start_smart_folder_prepare_inner(
                    snapshot,
                    refresh,
                    true,
                    authoritative_ignored_tombstones,
                    None,
                    None,
                );
                return false;
            }
        }
        let open_origin = self.smart_folder_open_origin.take();
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
        let root_navigation_entries = smart_root_navigation_entries(&items);
        let synthetic_path = smart_folder_synthetic_path(definition_id);
        // Opening becomes a history transition only now that scan+prepare succeeded.  A failed
        // or cancelled scan therefore never creates a dead entry in the Back stack.
        if !matches!(
            open_origin,
            Some(SmartFolderOpenOrigin::ReturnReprepare { .. })
        ) {
            if let Some(origin) = open_origin.as_ref() {
                self.record_folder_nav_transition_from_restore(&synthetic_path, origin.prior());
            } else {
                self.record_folder_nav_transition(&synthetic_path);
            }
        }
        if let Some(restore) = open_origin
            .as_ref()
            .and_then(|origin| origin.prior().subfolder_restore())
        {
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
        let mut top_level_state = self
            .top_level_grid_view
            .smart_folder()
            .filter(|state| state.definition_id == definition_id)
            .cloned()
            .unwrap_or_else(|| {
                super::top_level_grid_view::SmartFolderViewState::root(definition_id, Vec::new())
            });
        let _ = top_level_state.refresh_navigation_entries(root_navigation_entries);
        self.top_level_grid_view.replace_surface(
            super::top_level_grid_view::TopLevelGridSurface::SmartFolder(top_level_state),
        );
        self.top_level_grid_view
            .install_smart_folder_session(SmartFolderSession::new(
                snapshot,
                resort_metadata,
                presentation,
                metadata_revision,
            ));
        self.smart_folder_progress = None;
        self.address = format!("スマートフォルダ: {definition_name}");
        if let Some(&index) = self.visible_indices.first() {
            self.selected = Some(index);
            self.scroll_to_selected = true;
        }
        if let Some(SmartFolderOpenOrigin::ReturnReprepare {
            anchor: Some(path), ..
        }) = open_origin.as_ref()
            && let Some(index) = self.items.iter().position(|item| {
                item.drag_source_path()
                    .is_some_and(|source| crate::folder_tree::path_eq(source, path))
            })
        {
            self.select_item_after_load(index);
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
        if self.smart_folder_opening_pending() {
            // The root is still visible while one physical child is being opened. Preserve that
            // request; its frozen presentation is compared with current settings at return, or
            // re-prepared after an unadopted abort.
            return true;
        }
        let Some(id) = self.current_smart_folder_id else {
            return true;
        };
        // 通常一覧と同じグローバルなソート順を使う。スマートフォルダ定義へは
        // 書き戻さず、保存済み snapshot を現在の設定で prepare し直すだけにする。
        if let Some((snapshot, reused_metadata)) = self
            .top_level_grid_view
            .smart_folder_session()
            .filter(|session| session.definition_id == id)
            .and_then(|session| {
                Some((
                    session.root_snapshot()?.clone(),
                    session.root_resort_metadata(),
                ))
            })
        {
            self.start_smart_folder_prepare_inner(
                snapshot,
                false,
                false,
                HashSet::new(),
                None,
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
            .top_level_grid_view
            .smart_folder_session_mut()
            .filter(|session| session.definition_id == definition.id)
            .and_then(SmartFolderSession::root_snapshot_mut)
            .is_some_and(|snapshot| {
                let changed = snapshot.definition.grouping != definition.grouping;
                adopt_smart_folder_presentation(&mut snapshot.definition, definition);
                changed
            });
        if self.current_smart_folder_id == Some(definition.id) {
            self.address = format!("スマートフォルダ: {}", definition.name);
            self.update_smart_folder_scoped_address();
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
        if self.smart_folder_opening_pending() {
            return;
        }
        if self.current_smart_folder_id == Some(definition.id) && grouping_changed {
            if let Some(navigation_entries) = self
                .top_level_grid_view
                .smart_folder()
                .filter(|state| state.definition_id == definition.id)
                .map(|state| state.navigation_entries.as_ref().clone())
            {
                self.top_level_grid_view.replace_surface(
                    super::top_level_grid_view::TopLevelGridSurface::SmartFolder(
                        super::top_level_grid_view::SmartFolderViewState::root_with_navigation_entries(
                            definition.id,
                            navigation_entries,
                        ),
                    ),
                );
            }
            let prepared = self
                .top_level_grid_view
                .smart_folder_session()
                .and_then(|session| {
                    Some((
                        session.root_snapshot()?.clone(),
                        session.root_resort_metadata(),
                    ))
                });
            if let Some((snapshot, reused_metadata)) = prepared {
                self.start_smart_folder_prepare_inner(
                    snapshot,
                    false,
                    false,
                    HashSet::new(),
                    None,
                    reused_metadata,
                );
            } else {
                let _ = self.refresh_smart_folder_staged(definition.id);
            }
        }
    }

    /// Delete tombstones are part of the resident session. The prepared root grid is edited in
    /// place so an immediate return cannot resurrect an unusable cell; the scan snapshot keeps
    /// its indices stable for reusable metadata and applies the tombstone on later rebuilds.
    pub(crate) fn remove_paths_from_smart_folder_snapshots(&mut self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let removed: HashSet<String> = paths
            .iter()
            .map(|path| crate::path_key::normalize_keep_drive(path))
            .collect();
        if removed.is_empty() {
            return;
        }

        let resident_id = self
            .top_level_grid_view
            .smart_folder_session()
            .and_then(|session| {
                let snapshot = session.root_snapshot()?;
                snapshot
                    .entries
                    .iter()
                    .any(|entry| {
                        smart_folder_path_is_removed(
                            &crate::path_key::normalize_keep_drive(&entry.path),
                            &removed,
                        )
                    })
                    .then_some(session.definition_id)
            });
        if let Some(id) = resident_id {
            self.smart_folder_removed_paths
                .entry(id)
                .or_default()
                .extend(removed.iter().cloned());
            if let Some(grid) = self
                .top_level_grid_view
                .smart_folder_session_mut()
                .and_then(|session| session.phase.parked_root_mut())
            {
                grid.remove_paths(&removed);
            }
        }

        let confirm_affected = self
            .smart_folder_confirm_pending
            .as_ref()
            .is_some_and(|confirm| {
                confirm.snapshot.entries.iter().any(|entry| {
                    smart_folder_path_is_removed(
                        &crate::path_key::normalize_keep_drive(&entry.path),
                        &removed,
                    )
                })
            });
        if let Some(confirm) = self.smart_folder_confirm_pending.as_mut()
            && confirm_affected
        {
            self.smart_folder_removed_paths
                .entry(confirm.snapshot.definition.id)
                .or_default()
                .extend(removed.iter().cloned());
            if remove_paths_from_smart_folder_snapshot(&mut confirm.snapshot, &removed) {
                // Entry indices in the exact-count membership belong to the pre-removal snapshot.
                // A confirmed continuation can safely re-evaluate the now-smaller candidate set.
                confirm.membership = None;
                confirm.membership_revision = None;
            }
            confirm.result_count = confirm.result_count.min(confirm.snapshot.entries.len());
        }
        let pending_affected = self
            .smart_folder_prepare_pending
            .as_ref()
            .is_some_and(|pending| Some(pending.definition_id) == resident_id);
        if pending_affected {
            let retired = cancelled_smart_folder_payloads(
                None,
                self.smart_folder_prepare_pending.take(),
                None,
            );
            self.retire_smart_folder_payloads(retired);
            let restart = self
                .top_level_grid_view
                .smart_folder_session()
                .and_then(|session| {
                    Some((
                        session.root_snapshot()?.clone(),
                        session.root_resort_metadata(),
                    ))
                });
            if let Some((snapshot, reused_metadata)) = restart {
                self.start_smart_folder_prepare_inner(
                    snapshot,
                    false,
                    false,
                    HashSet::new(),
                    None,
                    reused_metadata,
                );
            }
        }
    }

    /// リネーム移行完了後は全定義の snapshot を失効させる。旧 path の即時除外は
    /// tombstone で行い、新 path はメタデータ移行完了後の authoritative scan でだけ採用する。
    pub(crate) fn refresh_smart_folders_after_rename(&mut self) {
        let reopen = self.current_smart_folder_id;
        // Path facts in every offscreen scan are stale after a rename, including a target that
        // differs from the currently visible Smart definition. Retire that request without
        // touching the old visible session or its tombstones.
        let retired_transition = self.retire_smart_folder_transition();
        self.cancel_smart_folder_pending();
        if let Some(id) = reopen {
            let _ = self.refresh_smart_folder_staged(id);
        } else if retired_transition {
            self.show_feedback_toast(
                "名前変更でスマートフォルダの読み込みを中止しました。開き直してください".into(),
            );
        }
    }

    pub(crate) fn render_smart_folder_overlay(&mut self, ctx: &egui::Context) {
        if let Some(transition) = self.smart_folder_transition.as_ref()
            && matches!(
                transition.phase,
                SmartFolderTransitionPhase::RootScan(_)
                    | SmartFolderTransitionPhase::RootCount(_)
                    | SmartFolderTransitionPhase::RootPrepare(_)
                    | SmartFolderTransitionPhase::RootReady(_)
            )
        {
            let request_id = transition.request_id;
            let progress = transition.progress.clone();
            let scanning = matches!(transition.phase, SmartFolderTransitionPhase::RootScan(_));
            let counting = matches!(transition.phase, SmartFolderTransitionPhase::RootCount(_));
            let mut cancel = false;
            egui::Modal::new(egui::Id::new("smart_folder_transition_progress_modal")).show(
                ctx,
                |ui| {
                    ui.set_min_width(460.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.heading(if scanning {
                            "スマートフォルダを走査中..."
                        } else if counting {
                            "スマートフォルダの件数を確認中..."
                        } else {
                            "スマートフォルダの表示を準備中..."
                        });
                    });
                    ui.add_space(6.0);
                    if scanning {
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
                    } else if !counting {
                        ui.label(format!(
                            "{}: {} / {} 件",
                            progress.phase.label(),
                            progress.completed,
                            progress.total
                        ));
                        if progress.total > 0 {
                            ui.add(
                                egui::ProgressBar::new(
                                    progress.completed as f32 / progress.total as f32,
                                )
                                .show_percentage(),
                            );
                        }
                    }
                    ui.add_space(8.0);
                    cancel = ui.button("中止").clicked();
                },
            );
            if cancel {
                self.cancel_staged_smart_root_modal_request(request_id);
            }
            ctx.request_repaint_after(Duration::from_millis(50));
            return;
        }
        if let Some(transition) = self.smart_folder_transition.as_ref()
            && let SmartFolderTransitionPhase::RootConfirm(confirm) = &transition.phase
        {
            let request_id = transition.request_id;
            let count = confirm.result_count;
            let name = confirm.snapshot.definition.name.clone();
            let mut proceed = false;
            let mut cancel = false;
            egui::Modal::new(egui::Id::new("smart_folder_transition_large_confirm")).show(
                ctx,
                |ui| {
                    ui.heading("スマートフォルダを表示");
                    ui.label(format!("「{name}」の結果は {count} 件です。"));
                    ui.label("一覧の準備に時間とメモリを使用する可能性があります。");
                    ui.horizontal(|ui| {
                        proceed = ui.button("続行").clicked();
                        cancel = ui.button("中止").clicked();
                    });
                },
            );
            if cancel {
                self.cancel_staged_smart_root_modal_request(request_id);
            } else if proceed
                && self
                    .smart_folder_transition
                    .as_ref()
                    .is_some_and(|current| current.request_id == request_id)
                && let Some(mut transition) = self.smart_folder_transition.take()
            {
                let phase =
                    std::mem::replace(&mut transition.phase, SmartFolderTransitionPhase::Retired);
                if let SmartFolderTransitionPhase::RootConfirm(confirm) = phase {
                    let membership = (confirm.membership_revision
                        == Some(self.smart_folder_metadata_revision))
                    .then_some(confirm.membership)
                    .flatten();
                    match self.spawn_smart_transition_prepare(
                        &transition,
                        confirm.snapshot,
                        confirm.refresh,
                        confirm.tombstones_at_start,
                        membership,
                    ) {
                        Ok(pending) => {
                            transition.phase = SmartFolderTransitionPhase::RootPrepare(pending);
                            self.smart_folder_transition = Some(transition);
                        }
                        Err(message) => self.show_feedback_toast(message),
                    }
                }
            }
        }
        if let Some(confirm) = self.smart_folder_confirm_pending.as_ref() {
            let count = confirm.result_count;
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
                        let membership = (confirm.membership_revision
                            == Some(self.smart_folder_metadata_revision))
                        .then_some(confirm.membership)
                        .flatten();
                        if let Some(membership) = membership {
                            self.start_smart_folder_prepare_after_count(
                                confirm.snapshot,
                                confirm.refresh,
                                confirm.tombstones_at_start,
                                membership,
                            );
                        } else {
                            self.start_smart_folder_prepare_after_scan(
                                confirm.snapshot,
                                confirm.refresh,
                                confirm.tombstones_at_start,
                            );
                        }
                    } else {
                        self.retire_smart_folder_payloads(std::iter::once(
                            Box::new(confirm) as RetiredSmartFolderPayload
                        ));
                    }
                }
            } else if cancel {
                self.cancel_smart_folder_pending_and_restore_origin();
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
            file_size: Some(1),
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
    fn smart_root_presentation_preview_matches_favorite_transition_boundaries() {
        use crate::settings::{FavoriteEntry, FavoriteViewState, SortOrder};

        let mut app = crate::app::setup_app_for_test();
        let child = app.tmp.path().join("favorite-child");
        let root = smart_folder_synthetic_path(uuid::Uuid::new_v4());
        app.settings.remember_favorite_view_state = true;
        app.settings.sort_order = SortOrder::DateAsc;
        let favorite = FavoriteEntry::new("child".into(), child.clone());
        let mut child_state = FavoriteViewState::from_settings(&app.settings);
        child_state.sort_order = SortOrder::FileName;
        app.favorite_view_states.insert(favorite.id, child_state);
        app.settings.favorites.push(favorite);
        app.transition_favorite_view_for_path(Some(&child));
        assert_eq!(app.settings.sort_order, SortOrder::FileName);

        app.settings.remember_favorite_view_state = false;
        let expected = SmartFolderPresentation::for_location(
            &app,
            &root,
            crate::settings::SubfolderExpansionOrder::default(),
        );
        app.transition_favorite_view_for_path(Some(&root));
        assert_eq!(
            expected,
            SmartFolderPresentation::current(
                &app,
                crate::settings::SubfolderExpansionOrder::default(),
            )
        );
        assert_eq!(app.settings.sort_order, SortOrder::DateAsc);

        app.settings.remember_favorite_view_state = true;
        app.transition_favorite_view_for_path(Some(&child));
        assert_eq!(app.settings.sort_order, SortOrder::FileName);
        let fresh_root_favorite = FavoriteEntry::new("new root".into(), root.clone());
        let id = fresh_root_favorite.id;
        app.settings.favorites.push(fresh_root_favorite);
        assert!(!app.favorite_view_states.contains_key(&id));
        let expected = SmartFolderPresentation::for_location(
            &app,
            &root,
            crate::settings::SubfolderExpansionOrder::default(),
        );
        app.transition_favorite_view_for_path(Some(&root));
        assert_eq!(
            expected,
            SmartFolderPresentation::current(
                &app,
                crate::settings::SubfolderExpansionOrder::default(),
            )
        );
        assert_eq!(app.settings.sort_order, SortOrder::FileName);
        assert!(app.favorite_view_states.contains_key(&id));
    }

    #[test]
    fn prepared_smart_folder_restore_releases_rating_only_details_sort() {
        let mut app = crate::app::setup_app_for_test();
        let definition_id = uuid::Uuid::new_v4();
        let mut definition = crate::settings::SmartFolderDefinition::new("復元テスト");
        definition.id = definition_id;
        let snapshot = SmartFolderSnapshot {
            definition,
            entries: Arc::new(Vec::new()),
            video_thumb_overrides: HashMap::new(),
            diag: SmartFolderDiag::default(),
        };
        let state = super::super::top_level_grid_view::SmartFolderViewState::root(
            definition_id,
            Vec::new(),
        );
        app.top_level_grid_view.replace_surface(
            super::super::top_level_grid_view::TopLevelGridSurface::SmartFolder(state),
        );
        let presentation = SmartFolderPresentation::current(&app, snapshot.definition.grouping);
        let metadata_revision = app.smart_folder_metadata_revision;
        app.top_level_grid_view
            .install_smart_folder_session(SmartFolderSession::new(
                snapshot,
                Arc::new(ReusedSmartFolderMetadata::default()),
                presentation,
                metadata_revision,
            ));

        app.items = vec![
            GridItem::Image(PathBuf::from(r"C:\Smart\a.jpg")),
            GridItem::Image(PathBuf::from(r"C:\Smart\b.jpg")),
        ];
        app.thumbnails = vec![ThumbnailState::Failed, ThumbnailState::Failed];
        app.image_metas = vec![Some((1, 10)), Some((2, 20))];
        app.visible_indices = vec![0, 1];
        app.details_order = vec![1, 0];
        app.items_are_smart_folder_view = true;
        app.settings.grid_view_mode = crate::settings::GridViewMode::Details;

        let child = PathBuf::from(r"C:\Smart\child");
        let parked_root = SmartFolderRootPayload::Visible(app.take_visible_smart_folder_grid());
        let mut session = app.top_level_grid_view.take_smart_folder_session().unwrap();
        session.phase = SmartFolderOpenPhase::Child {
            logical_path: child,
            parked_root,
        };
        app.top_level_grid_view
            .install_smart_folder_session(session);

        // Rating 一覧で専用列をソートした後、履歴から prepared root へ戻る経路を再現する。
        app.items_are_smart_folder_view = false;
        app.items_are_rating_view = true;
        app.settings.details_sort_key = crate::settings::DetailsSortKey::RatedAt;
        app.settings.details_sort_ascending = false;

        assert!(app.restore_prepared_smart_folder_session(definition_id, None));
        assert!(!app.items_are_rating_view);
        assert!(app.items_are_smart_folder_view);
        assert_eq!(
            app.settings.details_sort_key,
            crate::settings::DetailsSortKey::Toolbar
        );
        assert!(app.settings.details_sort_ascending);
        assert_eq!(app.details_order, vec![0, 1]);
        assert_eq!(app.grid_sort_lock_reason(), None);
    }

    #[test]
    fn cancelling_pending_owners_moves_queued_results_and_confirm_snapshot_together() {
        struct DropProbe {
            tx: std::sync::mpsc::Sender<std::thread::ThreadId>,
        }
        impl Drop for DropProbe {
            fn drop(&mut self) {
                let _ = self.tx.send(std::thread::current().id());
            }
        }

        let mut definition = crate::settings::SmartFolderDefinition::new("queued-drop");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            PathBuf::from(r"C:\Books"),
            true,
            true,
            Default::default(),
        ));

        let queued_entries = Arc::new(vec![smart_entry(r"C:\Books\queued.zip", 0, "")]);
        let queued_entries_weak = Arc::downgrade(&queued_entries);
        let queued_snapshot = SmartFolderSnapshot {
            definition: definition.clone(),
            entries: Arc::clone(&queued_entries),
            video_thumb_overrides: HashMap::new(),
            diag: SmartFolderDiag::default(),
        };
        drop(queued_entries);
        let (scan_tx, scan_rx) = mpsc::channel();
        scan_tx
            .send(SmartFolderScanEvent::Done(SmartFolderScanResult {
                snapshot: queued_snapshot,
            }))
            .unwrap();
        drop(scan_tx);
        let scan_cancel = Arc::new(AtomicBool::new(false));
        let scan_pending = SmartFolderPending {
            definition_id: definition.id,
            generation: 1,
            refresh: false,
            tombstones_at_start: HashSet::new(),
            cancel: Arc::clone(&scan_cancel),
            rx: scan_rx,
        };

        let (_prepare_tx, prepare_rx) = mpsc::channel();
        let prepare_cancel = Arc::new(AtomicBool::new(false));
        let prepare_pending = SmartFolderPreparePending {
            definition_id: definition.id,
            generation: 1,
            cancel: Arc::clone(&prepare_cancel),
            rx: prepare_rx,
        };

        let confirm_entries = Arc::new(vec![smart_entry(r"C:\Books\confirm.zip", 0, "")]);
        let confirm_entries_weak = Arc::downgrade(&confirm_entries);
        let confirm = SmartFolderConfirmPending {
            snapshot: SmartFolderSnapshot {
                definition,
                entries: Arc::clone(&confirm_entries),
                video_thumb_overrides: HashMap::new(),
                diag: SmartFolderDiag::default(),
            },
            result_count: 1,
            membership: None,
            membership_revision: None,
            generation: 1,
            refresh: false,
            tombstones_at_start: HashSet::new(),
        };
        drop(confirm_entries);

        let mut retired = cancelled_smart_folder_payloads(
            Some(scan_pending),
            Some(prepare_pending),
            Some(confirm),
        );
        assert!(scan_cancel.load(Ordering::Relaxed));
        assert!(prepare_cancel.load(Ordering::Relaxed));
        assert_eq!(retired.len(), 3);
        assert!(queued_entries_weak.upgrade().is_some());
        assert!(confirm_entries_weak.upgrade().is_some());

        let ui_thread = std::thread::current().id();
        let (drop_tx, drop_rx) = mpsc::channel();
        retired.push(Box::new(DropProbe { tx: drop_tx }) as RetiredSmartFolderPayload);
        std::thread::Builder::new()
            .name("smart-folder-cancel-drop-test".into())
            .spawn(move || drop(retired))
            .unwrap()
            .join()
            .unwrap();
        assert_ne!(drop_rx.recv().unwrap(), ui_thread);
        assert!(queued_entries_weak.upgrade().is_none());
        assert!(confirm_entries_weak.upgrade().is_none());
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
        assert!(!smart_folder_definition_uses_metadata(
            &definition,
            SmartFolderMetadataDependency::Bookmarks
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
        definition.rules[0].filter.edits.clear();
        definition.rules[0]
            .filter
            .edits
            .insert(crate::settings::FacetEditFlag::Bookmarked);
        assert!(smart_folder_definition_uses_metadata(
            &definition,
            SmartFolderMetadataDependency::Bookmarks
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
    fn smart_folder_never_descends_into_portable_metadata_bundle() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = tmp.path().join("root");
        let bundle = root.join(crate::fs_entry::PORTABLE_METADATA_BUNDLE_DIRNAME);
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(root.join("visible.jpg"), b"v").unwrap();
        std::fs::write(bundle.join("internal.jpg"), b"i").unwrap();

        let mut definition = crate::settings::SmartFolderDefinition::new("internal-filter");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            root.clone(),
            true,
            true,
            Default::default(),
        ));
        let result = run_test_scan(definition, unfiltered_scan_options());
        assert!(
            result
                .snapshot
                .entries
                .iter()
                .all(|entry| !entry.path.starts_with(&bundle))
        );
        assert!(result.snapshot.entries.iter().any(|entry| {
            entry.path.file_name().and_then(|name| name.to_str()) == Some("visible.jpg")
        }));
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
            Some(2 * 1024 * 1024),
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
            Some(2 * 1024 * 1024),
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
            None,
            &filter,
            "",
            now_unix_secs(),
        ));
        assert!(!passes_cheap_filter_values(
            SmartFolderEntryKind::Image,
            Path::new(r"C:\Books\empty.jpg"),
            "empty.jpg",
            now_unix_secs(),
            Some(0),
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
            None,
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
        let bookmarks = crate::bookmark_browser::BookmarkPresence::default();
        let converted = HashMap::new();
        assert!(metadata_filter_passes(
            &filter, &entry, &key, &ratings, &tags, &edits, &bookmarks, &converted,
        ));
        filter.tags.clear();
        filter.include_untagged = true;
        assert!(!metadata_filter_passes(
            &filter, &entry, &key, &ratings, &tags, &edits, &bookmarks, &converted,
        ));
    }

    #[test]
    fn large_result_count_applies_saved_rating_filter_before_confirmation() {
        let mut filter = crate::settings::SmartFolderFilter {
            ratings: [false; 6],
            ..Default::default()
        };
        filter.ratings[5] = true;
        let mut definition = crate::settings::SmartFolderDefinition::new("starred");
        definition.rules.push(rule(
            uuid::Uuid::new_v4(),
            PathBuf::from(r"C:\Books"),
            true,
            true,
            filter,
        ));
        let snapshot = SmartFolderSnapshot {
            definition,
            entries: Arc::new(vec![
                smart_entry(r"C:\Books\a.zip", 0, ""),
                smart_entry(r"C:\Books\b.zip", 0, ""),
                smart_entry(r"C:\Books\c.zip", 0, ""),
            ]),
            video_thumb_overrides: HashMap::new(),
            diag: SmartFolderDiag::default(),
        };
        let (tx, _rx) = mpsc::channel();
        let counted = count_smart_folder_results(
            snapshot,
            7,
            false,
            HashSet::new(),
            HashSet::new(),
            false,
            false,
            false,
            None,
            &AtomicBool::new(false),
            &tx,
        )
        .unwrap()
        .unwrap();

        assert_eq!(counted.snapshot.entries.len(), 3);
        assert_eq!(
            counted.result_count, 0,
            "confirmation must use the metadata-filtered result, not scan candidates"
        );
        assert_eq!(counted.metadata_revision, 7);
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
            &crate::bookmark_browser::BookmarkPresence::default(),
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
            ConvertedArchiveSourceState::CachedZip(cache_path),
        )]);

        assert!(metadata_filter_passes(
            &filter,
            &entry,
            &key,
            &HashMap::new(),
            &HashMap::new(),
            &edits,
            &crate::bookmark_browser::BookmarkPresence::default(),
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

    #[test]
    fn smart_size_sort_keeps_real_zero_known_and_unknown_last_for_local_and_remote_comparator() {
        let mut entries = vec![
            smart_entry(r"C:\Smart\unknown.jpg", 0, ""),
            smart_entry(r"C:\Smart\ten.jpg", 0, ""),
            smart_entry(r"C:\Smart\zero.jpg", 0, ""),
        ];
        for entry in &mut entries {
            entry.kind = SmartFolderEntryKind::Image;
        }
        entries[0].file_size = None;
        entries[1].file_size = Some(10);
        entries[2].file_size = Some(0);
        let included = [0, 1, 2];
        let names = |sort| {
            let keys = build_smart_entry_sort_keys(
                &entries,
                &included,
                sort,
                &crate::settings::GridDisplayOrder::default(),
            );
            let mut positions = [0, 1, 2];
            positions.sort_by(|&a, &b| {
                compare_smart_entries_within_group(
                    sort,
                    &entries[included[a]],
                    &keys[a],
                    &entries[included[b]],
                    &keys[b],
                )
            });
            positions.map(|position| {
                entries[included[position]]
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
        };
        assert_eq!(
            names(crate::settings::SortOrder::SizeAsc),
            ["zero.jpg", "ten.jpg", "unknown.jpg"]
        );
        assert_eq!(
            names(crate::settings::SortOrder::SizeDesc),
            ["ten.jpg", "zero.jpg", "unknown.jpg"]
        );
    }

    #[test]
    fn smart_sort_applies_name_and_numeric_desc_with_the_shared_comparator() {
        let mut entries = vec![
            smart_entry(r"C:\Smart\page2.jpg", 0, ""),
            smart_entry(r"C:\Smart\page10.jpg", 1, ""),
        ];
        for entry in &mut entries {
            entry.kind = SmartFolderEntryKind::Image;
        }
        let included = [0, 1];
        let names = |sort| {
            let keys = build_smart_entry_sort_keys(
                &entries,
                &included,
                sort,
                &crate::settings::GridDisplayOrder::default(),
            );
            let mut positions = [0, 1];
            positions.sort_by(|&a, &b| {
                compare_smart_entries_within_group(
                    sort,
                    &entries[included[a]],
                    &keys[a],
                    &entries[included[b]],
                    &keys[b],
                )
            });
            positions.map(|position| {
                entries[included[position]]
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
        };
        assert_eq!(
            names(crate::settings::SortOrder::FileNameDesc),
            ["page10.jpg", "page2.jpg"]
        );
        assert_eq!(
            names(crate::settings::SortOrder::NumericDesc),
            ["page10.jpg", "page2.jpg"]
        );
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
            None,
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
            None,
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
