//! 名前付きコレクションのPC管理UI。
//!
//! Manager の編集/import/export と、main Grid から toolbar collection へ参照を追加する
//! 非同期 ownership を接続する。Collection Grid を通常 folder に擬装せず、元ファイルも
//! 変更しない。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use crossbeam_channel::Receiver;
use eframe::egui;

use crate::app::App;
use crate::collection_store::{
    CollectionBatchAddOutcome, CollectionCatalogSnapshot, CollectionEntry, CollectionEntryId,
    CollectionId, CollectionImportLineStatus, CollectionImportPreview, CollectionOrderMode,
    CollectionPrepareError, CollectionPreparedRegistration, CollectionRevisionWatch,
    CollectionRuntimeEvent, CollectionRuntimeEventStream, CollectionSnapshot,
    CollectionStoreClient, CollectionStoreError, CollectionStoreRuntime, parse_collection_text,
    prepare_collection_export, prepare_collection_registrations, serialize_collection_paths,
    write_collection_export_atomic,
};

const COLLECTION_REORDER_DEFAULT_WINDOW_W: f32 = 880.0;
const COLLECTION_REORDER_DEFAULT_WINDOW_H: f32 = 640.0;
const COLLECTION_REORDER_MIN_WINDOW_W: f32 = 560.0;
const COLLECTION_REORDER_MIN_WINDOW_H: f32 = 360.0;
const COLLECTION_REORDER_DEFAULT_TILE_PX: f32 = 128.0;
const COLLECTION_REORDER_MIN_TILE_PX: f32 = 72.0;
const COLLECTION_REORDER_MAX_TILE_PX: f32 = 240.0;
const COLLECTION_REORDER_AUTO_SCROLL_EDGE_PX: f32 = 48.0;
const COLLECTION_REORDER_AUTO_SCROLL_MAX_STEP_PX: f32 = 18.0;
const COLLECTION_REORDER_SCROLLBAR_RESERVE_PX: f32 = 24.0;

#[derive(Clone, Debug, PartialEq, Eq)]
enum CollectionRuntimePhase {
    Inert,
    Starting,
    Ready,
    Failed(String),
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CollectionToolbarStatus {
    Starting,
    Ready,
    Unavailable,
}

impl CollectionToolbarStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Starting => "準備中…",
            Self::Ready => "（なし）",
            Self::Unavailable => "利用不可",
        }
    }

    pub(crate) fn selected_text(
        self,
        target_id: Option<CollectionId>,
        rows: &[(CollectionId, String)],
    ) -> String {
        if self != Self::Ready {
            return self.label().to_owned();
        }
        rows.iter()
            .find(|(id, _)| Some(*id) == target_id)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| self.label().to_owned())
    }
}

struct CatalogRequest {
    minimum_revision: u64,
    queued_at: Option<Instant>,
    receiver: Receiver<Result<CollectionCatalogSnapshot, CollectionStoreError>>,
}

struct SnapshotRequest {
    collection_id: CollectionId,
    minimum_revision: u64,
    queued_at: Option<Instant>,
    receiver: Receiver<Result<CollectionSnapshot, CollectionStoreError>>,
}

impl Drop for CatalogRequest {
    fn drop(&mut self) {
        if let Some(start) = self.queued_at {
            collection_ui_actor_rtt(
                "list_catalog",
                None,
                self.minimum_revision,
                0,
                "cancelled",
                start,
            );
        }
    }
}

impl Drop for SnapshotRequest {
    fn drop(&mut self) {
        if let Some(start) = self.queued_at {
            collection_ui_actor_rtt(
                "load_collection",
                Some(self.collection_id),
                self.minimum_revision,
                0,
                "cancelled",
                start,
            );
        }
    }
}

fn collection_ui_actor_rtt(
    operation: &'static str,
    collection_id: Option<CollectionId>,
    minimum_revision: u64,
    entries: usize,
    outcome: &'static str,
    start: Instant,
) {
    crate::perf::event(
        "collection",
        "actor_rtt",
        None,
        0,
        &[
            ("source", serde_json::Value::from("manager")),
            ("operation", serde_json::Value::from(operation)),
            (
                "collection_id",
                serde_json::Value::from(collection_id.map(|id| id.as_uuid().to_string())),
            ),
            (
                "minimum_revision",
                serde_json::Value::from(minimum_revision),
            ),
            ("entries", serde_json::Value::from(entries)),
            (
                "ms",
                serde_json::Value::from(start.elapsed().as_secs_f64() * 1000.0),
            ),
            ("outcome", serde_json::Value::from(outcome)),
        ],
    );
}

struct WorkerTask<T> {
    receiver: std::sync::mpsc::Receiver<T>,
    handle: Option<std::thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
}

impl<T> WorkerTask<T> {
    fn spawn(
        name: &str,
        work: impl FnOnce(Arc<AtomicBool>) -> T + Send + 'static,
    ) -> Result<Self, String>
    where
        T: Send + 'static,
    {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let handle = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let result = work(worker_cancel);
                let _ = sender.send(result);
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            receiver,
            handle: Some(handle),
            cancel,
        })
    }

    fn poll_finished(&mut self) -> Option<Result<T, String>> {
        let handle = self.handle.as_ref()?;
        if !handle.is_finished() {
            return None;
        }
        let handle = self.handle.take().expect("worker handle checked above");
        if handle.join().is_err() {
            return Some(Err("worker が予期せず終了しました".into()));
        }
        Some(
            self.receiver
                .try_recv()
                .map_err(|error| format!("worker の結果を受信できませんでした: {error}")),
        )
    }

    fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl<T> Drop for WorkerTask<T> {
    fn drop(&mut self) {
        self.cancel();
        if self
            .handle
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
            && let Some(handle) = self.handle.take()
        {
            let _ = handle.join();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NameAction {
    Create,
    Rename {
        collection_id: CollectionId,
        expected_revision: u64,
    },
}

#[derive(Clone, Debug)]
enum CollectionAddOrigin {
    Manager,
    Toolbar { collection_name: String },
    Grid(crate::app::top_level_grid_view::CollectionGridRequestStamp),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CollectionOperationOrigin {
    Manager,
    Grid(crate::app::top_level_grid_view::CollectionGridRequestStamp),
}

impl CollectionAddOrigin {
    fn is_toolbar(&self) -> bool {
        matches!(self, Self::Toolbar { .. })
    }

    fn is_detached(&self) -> bool {
        !matches!(self, Self::Manager)
    }

    fn collection_name(&self) -> Option<&str> {
        match self {
            Self::Manager => None,
            Self::Toolbar { collection_name } => Some(collection_name),
            Self::Grid(_) => None,
        }
    }

    fn grid_stamp(&self) -> Option<crate::app::top_level_grid_view::CollectionGridRequestStamp> {
        match self {
            Self::Grid(stamp) => Some(*stamp),
            Self::Manager | Self::Toolbar { .. } => None,
        }
    }
}

impl CollectionOperationOrigin {
    fn grid_stamp(self) -> Option<crate::app::top_level_grid_view::CollectionGridRequestStamp> {
        match self {
            Self::Grid(stamp) => Some(stamp),
            Self::Manager => None,
        }
    }
}

#[derive(Clone, Debug)]
enum ClassificationTarget {
    Add {
        collection_id: CollectionId,
        expected_revision: u64,
        import_errors: Vec<String>,
        origin: CollectionAddOrigin,
    },
    Relink {
        collection_id: CollectionId,
        expected_revision: u64,
        entry_id: CollectionEntryId,
        origin: CollectionOperationOrigin,
    },
}

#[derive(Debug)]
struct ClassificationResult {
    prepared: Vec<CollectionPreparedRegistration>,
    cancelled: bool,
}

enum ActorTask {
    Create {
        receiver: Receiver<Result<CollectionSnapshot, CollectionStoreError>>,
    },
    SnapshotMutation {
        collection_id: CollectionId,
        origin: CollectionOperationOrigin,
        receiver: Receiver<Result<CollectionSnapshot, CollectionStoreError>>,
    },
    ContextRemove {
        origin: crate::app::top_level_grid_view::CollectionGridRequestStamp,
        collection_id: CollectionId,
        entry_ids: Vec<CollectionEntryId>,
        receiver: Receiver<Result<CollectionSnapshot, CollectionStoreError>>,
    },
    Delete {
        deleted: CollectionId,
        receiver: Receiver<Result<CollectionCatalogSnapshot, CollectionStoreError>>,
    },
    Add {
        collection_id: CollectionId,
        errors: Vec<String>,
        origin: CollectionAddOrigin,
        receiver: Receiver<Result<CollectionBatchAddOutcome, CollectionStoreError>>,
    },
}

struct ToolbarAddSnapshotRequest {
    collection_id: CollectionId,
    collection_name: String,
    paths: Vec<PathBuf>,
    receiver: Receiver<Result<CollectionSnapshot, CollectionStoreError>>,
}

#[derive(Clone, Debug)]
pub(crate) enum CollectionGridSnapshotAction {
    Import(PathBuf),
    Export(PathBuf),
    SetOrder {
        mode: CollectionOrderMode,
        sort: crate::settings::SortOrder,
    },
    OpenReorder,
}

impl CollectionGridSnapshotAction {
    fn requires_exact_root_revision(&self) -> bool {
        matches!(self, Self::OpenReorder | Self::SetOrder { .. })
    }
}

struct CollectionGridSnapshotRequest {
    target: crate::app::collection_grid::CollectionGridContentTarget,
    action: CollectionGridSnapshotAction,
    receiver: Receiver<Result<CollectionSnapshot, CollectionStoreError>>,
}

#[derive(Clone)]
struct CollectionReorderEntry {
    entry: CollectionEntry,
    texture: Option<egui::TextureHandle>,
}

enum CollectionReorderPhase {
    Ready,
    Saving {
        receiver: Receiver<Result<CollectionSnapshot, CollectionStoreError>>,
    },
    Refreshing {
        receiver: Receiver<Result<CollectionSnapshot, CollectionStoreError>>,
    },
    Error {
        message: String,
        conflict: bool,
    },
}

impl CollectionReorderPhase {
    fn is_busy(&self) -> bool {
        matches!(self, Self::Saving { .. } | Self::Refreshing { .. })
    }
}

struct CollectionReorderState {
    collection_id: CollectionId,
    collection_name: String,
    base_revision: u64,
    entries: Vec<CollectionReorderEntry>,
    selected: Option<usize>,
    selected_ids: HashSet<CollectionEntryId>,
    selection_anchor: Option<usize>,
    dragging: Option<usize>,
    drag_auto_scroll_enabled: bool,
    drag_insert_index: Option<usize>,
    scroll_offset_y: f32,
    thumb_tile_px: f32,
    dirty: bool,
    phase: CollectionReorderPhase,
}

enum CollectionDialogOperation {
    Idle,
    Name {
        action: NameAction,
        draft: String,
    },
    ConfirmDelete {
        collection_id: CollectionId,
        expected_revision: u64,
        name: String,
    },
    ConfirmRemove {
        collection_id: CollectionId,
        expected_revision: u64,
        entry_ids: Vec<CollectionEntryId>,
        origin: CollectionOperationOrigin,
    },
    ReadingImport {
        collection_id: CollectionId,
        expected_revision: u64,
        source_path: PathBuf,
        origin: CollectionAddOrigin,
        task: WorkerTask<Result<CollectionImportPreview, String>>,
    },
    PreviewImport {
        collection_id: CollectionId,
        expected_revision: u64,
        source_path: PathBuf,
        preview: CollectionImportPreview,
        origin: CollectionAddOrigin,
    },
    Classifying {
        target: ClassificationTarget,
        progress: Arc<(AtomicUsize, AtomicUsize)>,
        task: WorkerTask<ClassificationResult>,
    },
    ToolbarAddSnapshot(ToolbarAddSnapshotRequest),
    GridSnapshot(CollectionGridSnapshotRequest),
    Submitting(ActorTask),
    ExportSnapshot {
        collection_id: CollectionId,
        destination: PathBuf,
        origin: CollectionOperationOrigin,
        receiver: Receiver<Result<CollectionSnapshot, CollectionStoreError>>,
    },
    ExportWriting {
        collection_id: CollectionId,
        collection_revision: u64,
        destination: PathBuf,
        origin: CollectionOperationOrigin,
        progress: Arc<(AtomicUsize, AtomicUsize)>,
        task: WorkerTask<Result<(), CollectionPrepareError>>,
    },
}

impl CollectionDialogOperation {
    fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    fn manager_close_behavior(&self) -> CollectionManagerCloseBehavior {
        if self.is_detached() {
            return CollectionManagerCloseBehavior::Detach;
        }
        match self {
            Self::Submitting(_) | Self::ExportSnapshot { .. } => {
                CollectionManagerCloseBehavior::KeepOpen
            }
            _ => CollectionManagerCloseBehavior::Cancel,
        }
    }

    fn is_detached(&self) -> bool {
        match self {
            Self::ToolbarAddSnapshot(_) | Self::GridSnapshot(_) => true,
            Self::ConfirmRemove {
                origin: CollectionOperationOrigin::Grid(_),
                ..
            }
            | Self::ExportSnapshot {
                origin: CollectionOperationOrigin::Grid(_),
                ..
            }
            | Self::ExportWriting {
                origin: CollectionOperationOrigin::Grid(_),
                ..
            } => true,
            Self::ReadingImport { origin, .. } | Self::PreviewImport { origin, .. } => {
                origin.is_detached()
            }
            Self::Classifying { target, .. } => match target {
                ClassificationTarget::Add { origin, .. } => origin.is_detached(),
                ClassificationTarget::Relink { origin, .. } => {
                    matches!(origin, CollectionOperationOrigin::Grid(_))
                }
            },
            Self::Submitting(ActorTask::SnapshotMutation { origin, .. }) => {
                matches!(origin, CollectionOperationOrigin::Grid(_))
            }
            Self::Submitting(ActorTask::ContextRemove { .. })
            | Self::Submitting(ActorTask::Add {
                origin: CollectionAddOrigin::Grid(_),
                ..
            })
            | Self::Submitting(ActorTask::Add {
                origin: CollectionAddOrigin::Toolbar { .. },
                ..
            }) => true,
            _ => false,
        }
    }

    fn toolbar_add_origin(&self) -> Option<&CollectionAddOrigin> {
        match self {
            Self::Classifying {
                target: ClassificationTarget::Add { origin, .. },
                ..
            }
            | Self::Submitting(ActorTask::Add { origin, .. })
                if origin.is_toolbar() =>
            {
                Some(origin)
            }
            _ => None,
        }
    }

    fn cancel_worker(&self) {
        match self {
            Self::ReadingImport { task, .. } => task.cancel(),
            Self::Classifying { task, .. } => task.cancel(),
            Self::ExportWriting { task, .. } => task.cancel(),
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CollectionManagerCloseBehavior {
    Cancel,
    KeepOpen,
    Detach,
}

pub(crate) struct CollectionUiState {
    runtime: Option<CollectionStoreRuntime>,
    events: Option<CollectionRuntimeEventStream>,
    client: Option<CollectionStoreClient>,
    watch: Option<CollectionRevisionWatch>,
    phase: CollectionRuntimePhase,
    catalog: Option<CollectionCatalogSnapshot>,
    catalog_request: Option<CatalogRequest>,
    wanted_catalog_revision: u64,
    selected_id: Option<CollectionId>,
    snapshot: Option<CollectionSnapshot>,
    snapshot_request: Option<SnapshotRequest>,
    wanted_collection_revision: u64,
    show_manager: bool,
    checked_entries: HashSet<CollectionEntryId>,
    selected_entry: Option<CollectionEntryId>,
    operation: CollectionDialogOperation,
    message: Option<(bool, String)>,
    manager_new_name: String,
    manager_rename_inputs: HashMap<CollectionId, String>,
    reorder: Option<CollectionReorderState>,
}

impl Default for CollectionUiState {
    fn default() -> Self {
        Self {
            runtime: None,
            events: None,
            client: None,
            watch: None,
            phase: CollectionRuntimePhase::Inert,
            catalog: None,
            catalog_request: None,
            wanted_catalog_revision: 0,
            selected_id: None,
            snapshot: None,
            snapshot_request: None,
            wanted_collection_revision: 0,
            show_manager: false,
            checked_entries: HashSet::new(),
            selected_entry: None,
            operation: CollectionDialogOperation::Idle,
            message: None,
            manager_new_name: String::new(),
            manager_rename_inputs: HashMap::new(),
            reorder: None,
        }
    }
}

impl CollectionUiState {
    fn can_edit(&self) -> bool {
        matches!(self.phase, CollectionRuntimePhase::Ready)
            && self
                .reorder
                .as_ref()
                .is_none_or(|state| !state.phase.is_busy())
    }

    fn install_runtime(&mut self, runtime: CollectionStoreRuntime) {
        self.shutdown_for_exit();
        self.client = Some(runtime.client());
        self.events = Some(runtime.event_stream());
        self.runtime = Some(runtime);
        self.phase = CollectionRuntimePhase::Starting;
        self.message = None;
    }

    fn install_process_owned_runtime(
        &mut self,
        client: CollectionStoreClient,
        events: CollectionRuntimeEventStream,
    ) {
        self.shutdown_for_exit();
        self.client = Some(client);
        self.events = Some(events);
        self.phase = CollectionRuntimePhase::Starting;
        self.message = None;
    }

    fn install_start_failure(&mut self, error: CollectionStoreError) {
        self.shutdown_for_exit();
        self.phase = CollectionRuntimePhase::Failed(error.to_string());
    }

    fn shutdown_for_exit(&mut self) {
        self.operation.cancel_worker();
        self.operation = CollectionDialogOperation::Idle;
        self.catalog_request = None;
        self.snapshot_request = None;
        self.watch = None;
        self.events = None;
        self.client = None;
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_and_join();
        }
        if !matches!(self.phase, CollectionRuntimePhase::Inert) {
            self.phase = CollectionRuntimePhase::Closed;
        }
    }

    fn request_catalog(&mut self, minimum_revision: u64) {
        self.wanted_catalog_revision = self.wanted_catalog_revision.max(minimum_revision);
        if self.catalog_request.is_some() {
            return;
        }
        let Some(client) = &self.client else {
            return;
        };
        let queued_at = crate::perf::is_enabled().then(Instant::now);
        match client.list_catalog() {
            Ok(receiver) => {
                self.catalog_request = Some(CatalogRequest {
                    minimum_revision: self.wanted_catalog_revision,
                    queued_at,
                    receiver,
                });
            }
            Err(CollectionStoreError::Starting | CollectionStoreError::Busy) => {}
            Err(error) => self.message = Some((true, collection_error_message(&error))),
        }
    }

    fn select_collection(&mut self, id: Option<CollectionId>) {
        if self.selected_id == id {
            return;
        }
        self.selected_id = id;
        self.snapshot = None;
        self.snapshot_request = None;
        self.wanted_collection_revision = 0;
        self.checked_entries.clear();
        self.selected_entry = None;
        self.operation.cancel_worker();
        self.operation = CollectionDialogOperation::Idle;
        if let Some(id) = id {
            self.request_snapshot(id, 0);
        }
    }

    fn request_snapshot(&mut self, collection_id: CollectionId, minimum_revision: u64) {
        if self.selected_id != Some(collection_id) {
            return;
        }
        self.wanted_collection_revision = self.wanted_collection_revision.max(minimum_revision);
        if self.snapshot_request.is_some() {
            return;
        }
        let Some(client) = &self.client else {
            return;
        };
        let queued_at = crate::perf::is_enabled().then(Instant::now);
        match client.load_collection(collection_id) {
            Ok(receiver) => {
                self.snapshot_request = Some(SnapshotRequest {
                    collection_id,
                    minimum_revision: self.wanted_collection_revision,
                    queued_at,
                    receiver,
                });
            }
            Err(CollectionStoreError::Starting | CollectionStoreError::Busy) => {}
            Err(error) => self.message = Some((true, collection_error_message(&error))),
        }
    }

    fn install_catalog(&mut self, catalog: CollectionCatalogSnapshot) {
        if catalog.catalog_revision < self.wanted_catalog_revision {
            self.request_catalog(self.wanted_catalog_revision);
            return;
        }
        self.wanted_catalog_revision = catalog.catalog_revision;
        let selected_still_exists = self.selected_id.is_some_and(|id| {
            catalog
                .definitions
                .iter()
                .any(|definition| definition.id == id)
        });
        let next = if selected_still_exists {
            self.selected_id
        } else {
            catalog.definitions.first().map(|definition| definition.id)
        };
        self.catalog = Some(catalog);
        if self.selected_id != next {
            self.select_collection(next);
        } else if let Some(id) = next {
            let revision = self
                .catalog
                .as_ref()
                .and_then(|catalog| catalog.definitions.iter().find(|item| item.id == id))
                .map_or(0, |definition| definition.revision);
            if self
                .snapshot
                .as_ref()
                .map_or(0, CollectionSnapshot::revision)
                < revision
            {
                self.request_snapshot(id, revision);
            }
        }
    }

    fn install_snapshot(&mut self, snapshot: CollectionSnapshot) {
        if self.selected_id != Some(snapshot.collection_id())
            || snapshot.revision() < self.wanted_collection_revision
        {
            if let Some(id) = self.selected_id {
                self.request_snapshot(id, self.wanted_collection_revision);
            }
            return;
        }
        self.wanted_collection_revision = snapshot.revision();
        self.checked_entries
            .retain(|id| snapshot.entries.iter().any(|entry| entry.id == *id));
        if self
            .selected_entry
            .is_some_and(|id| !snapshot.entries.iter().any(|entry| entry.id == id))
        {
            self.selected_entry = None;
        }
        self.snapshot = Some(snapshot);
    }
}

fn collection_error_message(error: &CollectionStoreError) -> String {
    match error {
        CollectionStoreError::Busy => {
            "コレクション処理が混み合っています。もう一度お試しください。".into()
        }
        CollectionStoreError::Starting => "コレクションを準備しています。".into(),
        CollectionStoreError::Unavailable => "コレクションを利用できません。".into(),
        CollectionStoreError::NotFound => "コレクションまたは項目が見つかりません。".into(),
        CollectionStoreError::Conflict { .. } => {
            "別の操作でコレクションが更新されました。最新の内容を読み直しました。".into()
        }
        CollectionStoreError::DuplicateSource(_) => "同じ参照は既に登録されています。".into(),
        CollectionStoreError::InvalidName => "コレクション名を入力してください。".into(),
        CollectionStoreError::InvalidPath(_) => "登録できないパスです。".into(),
        CollectionStoreError::InvalidOrder => "並び順を更新できませんでした。".into(),
        CollectionStoreError::ManualOrderInactive => "手動順のときだけ並べ替えられます。".into(),
        CollectionStoreError::IncompatibleSchema(_) => {
            "この版では新しいコレクションデータを開けません。".into()
        }
        CollectionStoreError::Persistence(message) => {
            format!("コレクションを保存できませんでした: {message}")
        }
    }
}

fn collection_reorder_select_single(state: &mut CollectionReorderState, index: usize) {
    state.selected_ids.clear();
    if let Some(entry) = state.entries.get(index) {
        state.selected_ids.insert(entry.entry.id);
        state.selected = Some(index);
        state.selection_anchor = Some(index);
    }
}

fn collection_reorder_toggle_selection(state: &mut CollectionReorderState, index: usize) {
    let Some(entry_id) = state.entries.get(index).map(|entry| entry.entry.id) else {
        return;
    };
    if !state.selected_ids.remove(&entry_id) {
        state.selected_ids.insert(entry_id);
    }
    if state.selected_ids.is_empty() {
        state.selected_ids.insert(entry_id);
    }
    state.selected = Some(index);
    state.selection_anchor = Some(index);
}

fn collection_reorder_select_range(state: &mut CollectionReorderState, index: usize) {
    if state.entries.is_empty() {
        return;
    }
    let anchor = state
        .selection_anchor
        .unwrap_or_else(|| state.selected.unwrap_or(index))
        .min(state.entries.len() - 1);
    let start = anchor.min(index);
    let end = anchor.max(index).min(state.entries.len() - 1);
    state.selected_ids.clear();
    for entry in &state.entries[start..=end] {
        state.selected_ids.insert(entry.entry.id);
    }
    state.selected = Some(index.min(state.entries.len() - 1));
}

fn ensure_collection_reorder_selection(state: &mut CollectionReorderState) {
    let live_ids = state
        .entries
        .iter()
        .map(|entry| entry.entry.id)
        .collect::<HashSet<_>>();
    state.selected_ids.retain(|id| live_ids.contains(id));
    if state.entries.is_empty() {
        state.selected = None;
        state.selection_anchor = None;
        return;
    }
    let selected = state
        .selected
        .unwrap_or(0)
        .min(state.entries.len().saturating_sub(1));
    if state.selected_ids.is_empty() {
        collection_reorder_select_single(state, selected);
        return;
    }
    state.selected = Some(selected);
    state.selection_anchor = state
        .selection_anchor
        .map(|anchor| anchor.min(state.entries.len().saturating_sub(1)))
        .or(Some(selected));
}

fn selected_collection_reorder_indices(state: &CollectionReorderState) -> Vec<usize> {
    state
        .entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            state
                .selected_ids
                .contains(&entry.entry.id)
                .then_some(index)
        })
        .collect()
}

fn move_selected_collection_reorder_group(
    state: &mut CollectionReorderState,
    insert_index: usize,
) -> bool {
    let selected_indices = selected_collection_reorder_indices(state);
    if selected_indices.is_empty() {
        return false;
    }
    let before = state
        .entries
        .iter()
        .map(|entry| entry.entry.id)
        .collect::<Vec<_>>();
    let focus_id = state
        .selected
        .and_then(|index| state.entries.get(index))
        .map(|entry| entry.entry.id);
    let removed_before = selected_indices
        .iter()
        .filter(|index| **index < insert_index.min(state.entries.len()))
        .count();
    let destination = insert_index
        .min(state.entries.len())
        .saturating_sub(removed_before);
    let selected_ids = state.selected_ids.clone();
    let mut moving = Vec::with_capacity(selected_indices.len());
    let mut remaining = Vec::with_capacity(state.entries.len() - selected_indices.len());
    for entry in state.entries.drain(..) {
        if selected_ids.contains(&entry.entry.id) {
            moving.push(entry);
        } else {
            remaining.push(entry);
        }
    }
    let destination = destination.min(remaining.len());
    remaining.splice(destination..destination, moving);
    state.entries = remaining;
    state.selected =
        focus_id.and_then(|id| state.entries.iter().position(|entry| entry.entry.id == id));
    let after = state
        .entries
        .iter()
        .map(|entry| entry.entry.id)
        .collect::<Vec<_>>();
    before != after
}

fn move_selected_collection_reorder_by(state: &mut CollectionReorderState, delta: i32) -> bool {
    let before = state
        .entries
        .iter()
        .map(|entry| entry.entry.id)
        .collect::<Vec<_>>();
    if delta < 0 {
        for index in 1..state.entries.len() {
            let selected = state.selected_ids.contains(&state.entries[index].entry.id);
            let previous_selected = state
                .selected_ids
                .contains(&state.entries[index - 1].entry.id);
            if selected && !previous_selected {
                state.entries.swap(index, index - 1);
            }
        }
    } else if delta > 0 {
        for index in (0..state.entries.len().saturating_sub(1)).rev() {
            let selected = state.selected_ids.contains(&state.entries[index].entry.id);
            let next_selected = state
                .selected_ids
                .contains(&state.entries[index + 1].entry.id);
            if selected && !next_selected {
                state.entries.swap(index, index + 1);
            }
        }
    }
    let focus_id = state.selected.and_then(|index| before.get(index)).copied();
    state.selected =
        focus_id.and_then(|id| state.entries.iter().position(|entry| entry.entry.id == id));
    before
        != state
            .entries
            .iter()
            .map(|entry| entry.entry.id)
            .collect::<Vec<_>>()
}

fn collection_reorder_grid_columns(available_width: f32, tile_width: f32, gap: f32) -> usize {
    let content_width = (available_width - COLLECTION_REORDER_SCROLLBAR_RESERVE_PX).max(tile_width);
    (((content_width + gap) / (tile_width + gap))
        .floor()
        .max(4.0)) as usize
}

fn collection_reorder_scroll_height(available_height: f32, rows: usize, row_height: f32) -> f32 {
    let content_height = rows.max(1) as f32 * row_height;
    content_height
        .min(available_height.max(row_height))
        .max(row_height)
}

fn collection_reorder_auto_scroll_delta(
    pointer_y: f32,
    viewport_top: f32,
    viewport_bottom: f32,
) -> f32 {
    let height = (viewport_bottom - viewport_top).max(0.0);
    if height <= 1.0 {
        return 0.0;
    }
    let edge = COLLECTION_REORDER_AUTO_SCROLL_EDGE_PX
        .min(height * 0.45)
        .max(1.0);
    if pointer_y < viewport_top + edge {
        -((viewport_top + edge - pointer_y) / edge).clamp(0.0, 1.0)
            * COLLECTION_REORDER_AUTO_SCROLL_MAX_STEP_PX
    } else if pointer_y > viewport_bottom - edge {
        ((pointer_y - (viewport_bottom - edge)) / edge).clamp(0.0, 1.0)
            * COLLECTION_REORDER_AUTO_SCROLL_MAX_STEP_PX
    } else {
        0.0
    }
}

fn collection_reorder_drop_target_for_pos(
    rect: egui::Rect,
    item_index: usize,
    len: usize,
    pointer_pos: Option<egui::Pos2>,
) -> Option<(usize, f32)> {
    let pos = pointer_pos?;
    if !rect.contains(pos) {
        return None;
    }
    let insert_after = pos.x >= rect.center().x;
    Some((
        if insert_after {
            item_index + 1
        } else {
            item_index
        }
        .min(len),
        if insert_after {
            rect.right() + 4.0
        } else {
            rect.left() - 4.0
        },
    ))
}

fn draw_collection_reorder_insert_indicator(ui: &egui::Ui, x: f32, rect: egui::Rect) {
    let y0 = rect.top() + 5.0;
    let y1 = rect.bottom() - 5.0;
    if y1 <= y0 {
        return;
    }
    let stroke = egui::Stroke::new(3.0, ui.visuals().selection.stroke.color);
    let painter = ui.painter();
    painter.line_segment([egui::pos2(x, y0), egui::pos2(x, y1)], stroke);
    painter.line_segment([egui::pos2(x - 6.0, y0), egui::pos2(x + 6.0, y0)], stroke);
    painter.line_segment([egui::pos2(x - 6.0, y1), egui::pos2(x + 6.0, y1)], stroke);
}

impl App {
    fn collection_reorder_textures_for(
        &self,
        collection_id: CollectionId,
        revision: u64,
    ) -> HashMap<CollectionEntryId, egui::TextureHandle> {
        let Some(session) = self.top_level_grid_view.collection_session() else {
            return HashMap::new();
        };
        if session.identity.collection_id != collection_id
            || !matches!(
                session.position,
                crate::app::top_level_grid_view::CollectionGridPosition::Root
            )
            || session.accepted_revision != revision
            || session.installed_items_generation != Some(self.items_generation)
        {
            return HashMap::new();
        }
        let Some(prepared) = session.prepared() else {
            return HashMap::new();
        };
        if prepared.collection_revision != revision || prepared.entries.len() != self.items.len() {
            return HashMap::new();
        }
        prepared
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let texture =
                    self.thumb_adjust_tex.get(&index).cloned().or_else(|| {
                        match self.thumbnails.get(index) {
                            Some(crate::grid_item::ThumbnailState::Loaded { tex, .. }) => {
                                Some(tex.clone())
                            }
                            _ => None,
                        }
                    })?;
                Some((entry.entry_id, texture))
            })
            .collect()
    }

    fn open_collection_reorder(
        &mut self,
        snapshot: CollectionSnapshot,
        selected_entry_id: Option<CollectionEntryId>,
    ) {
        let textures =
            self.collection_reorder_textures_for(snapshot.collection_id(), snapshot.revision());
        let entries = snapshot
            .entries
            .iter()
            .cloned()
            .map(|entry| CollectionReorderEntry {
                texture: textures.get(&entry.id).cloned(),
                entry,
            })
            .collect::<Vec<_>>();
        let selected = selected_entry_id
            .and_then(|id| entries.iter().position(|entry| entry.entry.id == id))
            .or((!entries.is_empty()).then_some(0));
        let selected_ids = selected
            .and_then(|index| entries.get(index))
            .map(|entry| HashSet::from([entry.entry.id]))
            .unwrap_or_default();
        let base_revision = snapshot.revision();
        self.collection_ui.reorder = Some(CollectionReorderState {
            collection_id: snapshot.collection_id(),
            collection_name: snapshot.definition.name,
            base_revision,
            entries,
            selected,
            selected_ids,
            selection_anchor: selected,
            dragging: None,
            drag_auto_scroll_enabled: false,
            drag_insert_index: None,
            scroll_offset_y: 0.0,
            thumb_tile_px: COLLECTION_REORDER_DEFAULT_TILE_PX,
            dirty: false,
            phase: CollectionReorderPhase::Ready,
        });
    }

    fn refresh_collection_reorder_textures(&mut self) {
        let Some(state) = self.collection_ui.reorder.as_ref() else {
            return;
        };
        let collection_id = state.collection_id;
        let revision = state.base_revision;
        let textures = self.collection_reorder_textures_for(collection_id, revision);
        if textures.is_empty() {
            return;
        }
        if let Some(state) = self.collection_ui.reorder.as_mut()
            && state.collection_id == collection_id
            && state.base_revision == revision
        {
            for entry in &mut state.entries {
                if entry.texture.is_none() {
                    entry.texture = textures.get(&entry.entry.id).cloned();
                }
            }
        }
    }

    fn start_collection_reorder_save(&mut self) {
        let Some(state) = self.collection_ui.reorder.as_mut() else {
            return;
        };
        if state.phase.is_busy() || !state.dirty {
            return;
        }
        let Some(client) = self.collection_ui.client.clone() else {
            state.phase = CollectionReorderPhase::Error {
                message: "コレクションを利用できません。".into(),
                conflict: false,
            };
            return;
        };
        let order = state
            .entries
            .iter()
            .map(|entry| entry.entry.id)
            .collect::<Vec<_>>();
        match client.reorder_manual(state.collection_id, state.base_revision, order) {
            Ok(receiver) => state.phase = CollectionReorderPhase::Saving { receiver },
            Err(error) => {
                state.phase = CollectionReorderPhase::Error {
                    message: collection_error_message(&error),
                    conflict: matches!(error, CollectionStoreError::Conflict { .. }),
                };
            }
        }
    }

    fn start_collection_reorder_refresh(&mut self) {
        let Some(state) = self.collection_ui.reorder.as_mut() else {
            return;
        };
        if state.phase.is_busy() {
            return;
        }
        let Some(client) = self.collection_ui.client.clone() else {
            state.phase = CollectionReorderPhase::Error {
                message: "コレクションを利用できません。".into(),
                conflict: false,
            };
            return;
        };
        match client.load_collection(state.collection_id) {
            Ok(receiver) => state.phase = CollectionReorderPhase::Refreshing { receiver },
            Err(error) => {
                state.phase = CollectionReorderPhase::Error {
                    message: collection_error_message(&error),
                    conflict: matches!(error, CollectionStoreError::Conflict { .. }),
                };
            }
        }
    }

    fn poll_collection_reorder(&mut self, ctx: &egui::Context) {
        let Some(state) = self.collection_ui.reorder.as_mut() else {
            return;
        };
        let phase = std::mem::replace(&mut state.phase, CollectionReorderPhase::Ready);
        match phase {
            CollectionReorderPhase::Ready | CollectionReorderPhase::Error { .. } => {
                state.phase = phase;
            }
            CollectionReorderPhase::Saving { receiver } => match receiver.try_recv() {
                Ok(Ok(snapshot)) if snapshot.collection_id() == state.collection_id => {
                    let collection_id = state.collection_id;
                    self.collection_ui.reorder = None;
                    self.collection_ui
                        .request_catalog(self.collection_ui.wanted_catalog_revision);
                    if self.collection_ui.selected_id == Some(collection_id) {
                        self.collection_ui.install_snapshot(snapshot);
                    }
                    self.collection_ui.message =
                        Some((false, "コレクションの順序を保存しました。".into()));
                    self.show_feedback_toast("コレクションの順序を保存しました。".into());
                }
                Ok(Ok(_)) => {
                    state.phase = CollectionReorderPhase::Error {
                        message: "保存先のコレクション応答が一致しませんでした。".into(),
                        conflict: true,
                    };
                }
                Ok(Err(error)) => {
                    let conflict = matches!(error, CollectionStoreError::Conflict { .. });
                    let message = collection_error_message(&error);
                    state.phase = CollectionReorderPhase::Error {
                        message: message.clone(),
                        conflict,
                    };
                    self.collection_ui.message = Some((true, message));
                    if conflict {
                        self.collection_ui
                            .request_catalog(self.collection_ui.wanted_catalog_revision);
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    state.phase = CollectionReorderPhase::Saving { receiver };
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    state.phase = CollectionReorderPhase::Error {
                        message: "コレクション保存の応答が失われました。".into(),
                        conflict: false,
                    };
                }
            },
            CollectionReorderPhase::Refreshing { receiver } => match receiver.try_recv() {
                Ok(Ok(snapshot))
                    if snapshot.collection_id() == state.collection_id
                        && snapshot.definition.order_mode == CollectionOrderMode::Manual =>
                {
                    let revision = snapshot.revision();
                    let old_textures = state
                        .entries
                        .iter()
                        .filter_map(|entry| {
                            entry
                                .texture
                                .clone()
                                .map(|texture| (entry.entry.id, texture))
                        })
                        .collect::<HashMap<_, _>>();
                    state.collection_name = snapshot.definition.name;
                    state.base_revision = revision;
                    state.entries = snapshot
                        .entries
                        .iter()
                        .cloned()
                        .map(|entry| CollectionReorderEntry {
                            texture: old_textures.get(&entry.id).cloned(),
                            entry,
                        })
                        .collect();
                    state.selected = (!state.entries.is_empty()).then_some(0);
                    state.selected_ids.clear();
                    state.selection_anchor = state.selected;
                    state.dragging = None;
                    state.drag_auto_scroll_enabled = false;
                    state.drag_insert_index = None;
                    state.dirty = false;
                    state.phase = CollectionReorderPhase::Ready;
                    ensure_collection_reorder_selection(state);
                }
                Ok(Ok(_)) => {
                    state.phase = CollectionReorderPhase::Error {
                        message: "最新内容は手動順ではないため、並べ替えを続けられません。".into(),
                        conflict: true,
                    };
                }
                Ok(Err(error)) => {
                    state.phase = CollectionReorderPhase::Error {
                        message: collection_error_message(&error),
                        conflict: matches!(error, CollectionStoreError::Conflict { .. }),
                    };
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    state.phase = CollectionReorderPhase::Refreshing { receiver };
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    state.phase = CollectionReorderPhase::Error {
                        message: "最新内容の応答が失われました。".into(),
                        conflict: false,
                    };
                }
            },
        }
    }

    pub(crate) fn collection_reorder_open(&self) -> bool {
        self.collection_ui.reorder.is_some()
    }

    pub(crate) fn collection_definition(
        &self,
        collection_id: CollectionId,
    ) -> Option<crate::collection_store::CollectionDefinition> {
        self.collection_ui
            .catalog
            .as_ref()?
            .definitions
            .iter()
            .find(|definition| definition.id == collection_id)
            .cloned()
    }

    pub(crate) fn collection_catalog_revision(&self, collection_id: CollectionId) -> Option<u64> {
        self.collection_ui
            .catalog
            .as_ref()?
            .definitions
            .iter()
            .find(|definition| definition.id == collection_id)
            .map(|definition| definition.revision)
    }

    pub(crate) fn collection_catalog_contains(&self, collection_id: CollectionId) -> bool {
        match self.collection_ui.phase {
            CollectionRuntimePhase::Ready => {
                self.collection_ui.catalog.as_ref().is_some_and(|catalog| {
                    catalog
                        .definitions
                        .iter()
                        .any(|definition| definition.id == collection_id)
                })
            }
            CollectionRuntimePhase::Starting
            | CollectionRuntimePhase::Inert
            | CollectionRuntimePhase::Failed(_)
            | CollectionRuntimePhase::Closed => true,
        }
    }

    pub(crate) fn folder_nav_history_target_label(
        &self,
        target: &crate::app::FolderNavHistoryTarget,
    ) -> String {
        match target {
            crate::app::FolderNavHistoryTarget::Path(path) => path.to_string_lossy().into_owned(),
            crate::app::FolderNavHistoryTarget::Rating { stars } => {
                format!("レーティング: {}", "★".repeat(usize::from(*stars)))
            }
            crate::app::FolderNavHistoryTarget::SmartFolder(state) => {
                format!("スマートフォルダ: {}", state.definition_id)
            }
            crate::app::FolderNavHistoryTarget::Collection(restore) => {
                let id = restore.identity.collection_id;
                self.collection_ui
                    .catalog
                    .as_ref()
                    .and_then(|catalog| {
                        catalog
                            .definitions
                            .iter()
                            .find(|definition| definition.id == id)
                    })
                    .map_or_else(
                        || format!("コレクション: {id:?}"),
                        |definition| format!("コレクション: {}", definition.name),
                    )
            }
        }
    }

    /// Removes history destinations only when a Ready catalog authoritatively proves that their
    /// stable collection ID no longer exists. The same filter runs after rollback restore so an
    /// older snapshot cannot resurrect a deleted collection destination.
    pub(crate) fn prune_collection_folder_history_from_ready_catalog(&mut self) {
        if !matches!(self.collection_ui.phase, CollectionRuntimePhase::Ready) {
            return;
        }
        let Some(catalog) = self.collection_ui.catalog.as_ref() else {
            return;
        };
        let available: HashSet<CollectionId> = catalog
            .definitions
            .iter()
            .map(|definition| definition.id)
            .collect();
        let retain = |target: &crate::app::FolderNavHistoryTarget| {
            target
                .collection_id()
                .is_none_or(|collection_id| available.contains(&collection_id))
        };
        self.folder_nav_back_stack.retain(&retain);
        self.folder_nav_forward_stack.retain(&retain);
        for workspace in &mut self.quick_folder_workspaces {
            workspace.history.back_stack.retain(&retain);
            workspace.history.forward_stack.retain(&retain);
        }
    }

    fn persist_collection_toolbar_target(&self) {
        // Unit tests construct many Apps against process-global settings test databases in
        // parallel. The Settings round-trip test covers this field's persistence; handler tests
        // must not write through another test's temporary global database.
        #[cfg(not(test))]
        self.settings.save();
    }

    pub(crate) fn install_collection_runtime(&mut self, runtime: CollectionStoreRuntime) {
        self.collection_ui.install_runtime(runtime);
    }

    pub(crate) fn install_process_owned_collection_runtime(
        &mut self,
        client: CollectionStoreClient,
        events: CollectionRuntimeEventStream,
    ) {
        self.collection_ui
            .install_process_owned_runtime(client, events);
    }

    pub(crate) fn install_collection_runtime_failure(&mut self, error: CollectionStoreError) {
        self.collection_ui.install_start_failure(error);
    }

    pub(crate) fn shutdown_collection_runtime_for_exit(&mut self) {
        self.collection_ui.shutdown_for_exit();
    }

    pub(crate) fn collection_manager_open(&self) -> bool {
        self.collection_ui.show_manager
    }

    /// True only while the collection operation draws an `egui::Modal` in this frame.
    /// Background snapshot/classification/actor/export phases use the modeless status window and
    /// must not block input to the whole viewer.
    pub(crate) fn collection_operation_modal_visible(&self) -> bool {
        match &self.collection_ui.operation {
            CollectionDialogOperation::Name { .. }
            | CollectionDialogOperation::ConfirmDelete { .. }
            | CollectionDialogOperation::ConfirmRemove { .. } => true,
            CollectionDialogOperation::PreviewImport { origin, .. } => origin
                .grid_stamp()
                .is_none_or(|stamp| self.collection_grid_request_stamp_is_current(stamp)),
            CollectionDialogOperation::Idle
            | CollectionDialogOperation::ReadingImport { .. }
            | CollectionDialogOperation::Classifying { .. }
            | CollectionDialogOperation::ToolbarAddSnapshot(_)
            | CollectionDialogOperation::GridSnapshot(_)
            | CollectionDialogOperation::Submitting(_)
            | CollectionDialogOperation::ExportSnapshot { .. }
            | CollectionDialogOperation::ExportWriting { .. } => false,
        }
    }

    pub(crate) fn open_collection_manager(&mut self, selected: Option<CollectionId>) {
        self.collection_ui.show_manager = true;
        if self.collection_ui.operation.is_idle()
            && let Some(id) = selected
        {
            self.collection_ui.select_collection(Some(id));
        }
    }

    pub(crate) fn collection_toolbar_catalog(
        &self,
    ) -> (
        Option<CollectionId>,
        Vec<(CollectionId, String)>,
        CollectionToolbarStatus,
    ) {
        let rows: Vec<(CollectionId, String)> = self
            .collection_ui
            .catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .definitions
                    .iter()
                    .map(|definition| (definition.id, definition.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let target_id = self
            .settings
            .toolbar_collection_target_id
            .map(CollectionId::from_uuid)
            .filter(|target| rows.iter().any(|(id, _)| id == target));
        (
            target_id,
            rows,
            match self.collection_ui.phase {
                CollectionRuntimePhase::Starting => CollectionToolbarStatus::Starting,
                CollectionRuntimePhase::Ready => CollectionToolbarStatus::Ready,
                CollectionRuntimePhase::Inert
                | CollectionRuntimePhase::Failed(_)
                | CollectionRuntimePhase::Closed => CollectionToolbarStatus::Unavailable,
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn collection_toolbar_add_settled_after_revision_for_test(
        &self,
        collection_id: CollectionId,
        revision: u64,
    ) -> bool {
        self.collection_ui.operation.is_idle()
            && self.collection_ui.catalog.as_ref().is_some_and(|catalog| {
                catalog.definitions.iter().any(|definition| {
                    definition.id == collection_id && definition.revision > revision
                })
            })
    }

    /// Changes only the toolbar's durable add/open target. Manager selection, pending operations,
    /// collection Grid ownership, and navigation are deliberately outside this transition.
    pub(crate) fn select_collection_toolbar_target(&mut self, id: CollectionId) {
        if !matches!(self.collection_ui.phase, CollectionRuntimePhase::Ready)
            || !self.collection_ui.catalog.as_ref().is_some_and(|catalog| {
                catalog
                    .definitions
                    .iter()
                    .any(|definition| definition.id == id)
            })
        {
            return;
        }
        let id = Some(id.as_uuid());
        if self.settings.toolbar_collection_target_id != id {
            self.settings.toolbar_collection_target_id = id;
            self.persist_collection_toolbar_target();
        }
    }

    pub(crate) fn set_collection_pinned(&mut self, id: CollectionId, pinned: bool) {
        if !matches!(self.collection_ui.phase, CollectionRuntimePhase::Ready)
            || !self.collection_ui.catalog.as_ref().is_some_and(|catalog| {
                catalog
                    .definitions
                    .iter()
                    .any(|definition| definition.id == id)
            })
        {
            return;
        }
        let raw = id.as_uuid();
        let changed = if pinned {
            if self.settings.pinned_collections.contains(&raw) {
                false
            } else {
                self.settings.pinned_collections.push(raw);
                true
            }
        } else {
            let before = self.settings.pinned_collections.len();
            self.settings
                .pinned_collections
                .retain(|candidate| *candidate != raw);
            self.settings.pinned_collections.len() != before
        };
        if changed {
            self.persist_collection_toolbar_target();
        }
    }

    /// An authoritative Ready catalog is the only owner allowed to repair a missing toolbar
    /// target. Starting/unavailable phases and transient snapshot gaps retain the saved choice.
    fn reconcile_collection_toolbar_target(&mut self) {
        if !matches!(self.collection_ui.phase, CollectionRuntimePhase::Ready) {
            return;
        }
        let Some(catalog) = self.collection_ui.catalog.as_ref() else {
            return;
        };
        let catalog_ids = catalog
            .definitions
            .iter()
            .map(|definition| definition.id.as_uuid())
            .collect::<HashSet<_>>();
        let mut seen = HashSet::new();
        let old_pins = self.settings.pinned_collections.clone();
        self.settings
            .pinned_collections
            .retain(|id| catalog_ids.contains(id) && seen.insert(*id));
        let pins_changed = self.settings.pinned_collections != old_pins;
        let saved = self
            .settings
            .toolbar_collection_target_id
            .map(CollectionId::from_uuid);
        let target = saved
            .filter(|id| {
                catalog
                    .definitions
                    .iter()
                    .any(|definition| definition.id == *id)
            })
            .or_else(|| catalog.definitions.first().map(|definition| definition.id));
        let target = target.map(CollectionId::as_uuid);
        if self.settings.toolbar_collection_target_id != target {
            self.settings.toolbar_collection_target_id = target;
            self.persist_collection_toolbar_target();
        } else if pins_changed {
            self.persist_collection_toolbar_target();
        }
    }

    /// Adds the current main-Grid selection to a toolbar collection without changing the
    /// collection manager's selected definition. Checked cells take precedence over the cursor,
    /// exactly as they do for the bookshelf. The actor snapshot is loaded after the click so the
    /// add uses the target collection's latest revision rather than a possibly stale manager copy.
    pub(crate) fn add_grid_selection_to_collection(&mut self, collection_id: CollectionId) {
        if !self.collection_ui.can_edit() {
            self.show_feedback_toast("コレクションを現在編集できません。".into());
            return;
        }
        if !self.collection_ui.operation.is_idle() {
            self.show_feedback_toast("別のコレクション処理が進行中です。".into());
            return;
        }
        let Some(collection_name) = self
            .collection_ui
            .catalog
            .as_ref()
            .and_then(|catalog| {
                catalog
                    .definitions
                    .iter()
                    .find(|definition| definition.id == collection_id)
            })
            .map(|definition| definition.name.clone())
        else {
            self.collection_ui
                .request_catalog(self.collection_ui.wanted_catalog_revision);
            self.show_feedback_toast("追加先のコレクションが見つかりません。".into());
            return;
        };

        let indices = self.grid_selection_indices();
        if indices.is_empty() {
            self.show_feedback_toast("コレクションに追加する項目を選択してください。".into());
            return;
        }
        let mut paths = Vec::with_capacity(indices.len());
        for index in indices {
            let Some(item) = self.items.get(index) else {
                self.show_feedback_toast(
                    "選択内容が更新されたため、コレクションに追加できません。".into(),
                );
                return;
            };
            let Some(path) = item.drag_source_path() else {
                let message = item.file_operation_refusal().map_or_else(
                    || "この項目はコレクションに追加できません".to_owned(),
                    |reason| reason.message("コレクションに追加"),
                );
                self.show_feedback_toast(message);
                return;
            };
            paths.push(path.to_path_buf());
        }

        let Some(client) = self.collection_ui.client.clone() else {
            self.show_feedback_toast("コレクションを利用できません。".into());
            return;
        };
        match client.load_collection(collection_id) {
            Ok(receiver) => {
                let count = paths.len();
                self.collection_ui.operation =
                    CollectionDialogOperation::ToolbarAddSnapshot(ToolbarAddSnapshotRequest {
                        collection_id,
                        collection_name: collection_name.clone(),
                        paths,
                        receiver,
                    });
                self.show_feedback_toast(format!(
                    "「{collection_name}」へ追加する {count} 件を確認しています。"
                ));
            }
            Err(error) => {
                let message = collection_error_message(&error);
                self.collection_ui.message = Some((true, message.clone()));
                self.show_feedback_toast(format!(
                    "「{collection_name}」へ追加できませんでした: {message}"
                ));
            }
        }
    }

    pub(crate) fn collection_store_client(&self) -> Option<CollectionStoreClient> {
        self.collection_ui
            .can_edit()
            .then(|| self.collection_ui.client.clone())
            .flatten()
    }

    pub(crate) fn collection_store_client_for_migration(
        &self,
    ) -> Result<Option<CollectionStoreClient>, CollectionStoreError> {
        match &self.collection_ui.phase {
            CollectionRuntimePhase::Ready => self
                .collection_ui
                .client
                .clone()
                .map(Some)
                .ok_or(CollectionStoreError::Unavailable),
            CollectionRuntimePhase::Starting => Err(CollectionStoreError::Starting),
            // Inert is the explicit headless/test configuration: no collection database was
            // installed, hence there is no durable collection owner to migrate.
            CollectionRuntimePhase::Inert => Ok(None),
            CollectionRuntimePhase::Failed(_) | CollectionRuntimePhase::Closed => {
                Err(CollectionStoreError::Unavailable)
            }
        }
    }

    pub(crate) fn select_collection_management_target(&mut self, id: CollectionId) {
        if !self.collection_ui.operation.is_idle() {
            return;
        }
        self.collection_ui.select_collection(Some(id));
        self.collection_ui.show_manager = true;
    }

    /// Runtime/worker結果はviewport/fullscreen早期returnより前で回収する。
    pub(crate) fn poll_collection_ui(&mut self, ctx: &egui::Context) {
        let events = self
            .collection_ui
            .events
            .as_ref()
            .map(|events| std::iter::from_fn(|| events.try_recv()).collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            match event {
                CollectionRuntimeEvent::Ready(catalog) => {
                    self.collection_ui.phase = CollectionRuntimePhase::Ready;
                    self.collection_ui.watch = self
                        .collection_ui
                        .client
                        .as_ref()
                        .and_then(|client| client.subscribe().ok());
                    self.collection_ui.install_catalog(catalog);
                    self.prune_collection_folder_history_from_ready_catalog();
                }
                CollectionRuntimeEvent::Failed(error) => {
                    self.collection_ui.phase =
                        CollectionRuntimePhase::Failed(collection_error_message(&error));
                    self.collection_ui.catalog_request = None;
                    self.collection_ui.snapshot_request = None;
                    self.collection_ui.operation.cancel_worker();
                    self.collection_ui.operation = CollectionDialogOperation::Idle;
                    self.collection_ui.message = Some((true, collection_error_message(&error)));
                }
                CollectionRuntimeEvent::Closed => {
                    self.collection_ui.phase = CollectionRuntimePhase::Closed;
                    self.collection_ui.catalog_request = None;
                    self.collection_ui.snapshot_request = None;
                    self.collection_ui.operation.cancel_worker();
                    self.collection_ui.operation = CollectionDialogOperation::Idle;
                }
            }
        }

        if let Some(notice) = self
            .collection_ui
            .watch
            .as_ref()
            .and_then(CollectionRevisionWatch::take_latest)
        {
            self.collection_ui.request_catalog(notice.catalog_revision);
            if let Some(id) = self.collection_ui.selected_id
                && let Some((_, revision)) = notice
                    .collection_revisions
                    .iter()
                    .find(|(candidate, _)| *candidate == id)
            {
                self.collection_ui.request_snapshot(id, *revision);
            }
        }

        if let Some(mut request) = self.collection_ui.catalog_request.take() {
            let minimum_revision = request.minimum_revision;
            let reply = request.receiver.try_recv();
            if !matches!(&reply, Err(crossbeam_channel::TryRecvError::Empty))
                && let Some(start) = request.queued_at.take()
            {
                let (entries, outcome) = match &reply {
                    Ok(Ok(catalog))
                        if catalog.catalog_revision
                            < self
                                .collection_ui
                                .wanted_catalog_revision
                                .max(minimum_revision) =>
                    {
                        (catalog.definitions.len(), "stale")
                    }
                    Ok(Ok(catalog)) => (catalog.definitions.len(), "ok"),
                    Ok(Err(_)) => (0, "error"),
                    Err(_) => (0, "disconnected"),
                };
                collection_ui_actor_rtt(
                    "list_catalog",
                    None,
                    minimum_revision,
                    entries,
                    outcome,
                    start,
                );
            }
            match reply {
                Ok(Ok(catalog)) => {
                    self.collection_ui.install_catalog(catalog);
                    self.prune_collection_folder_history_from_ready_catalog();
                }
                Ok(Err(error)) => {
                    self.collection_ui.message = Some((true, collection_error_message(&error)))
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    self.collection_ui.catalog_request = Some(request)
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.collection_ui.message =
                        Some((true, "コレクション一覧の応答が失われました。".into()))
                }
            }
            if self.collection_ui.catalog_request.is_none()
                && self
                    .collection_ui
                    .catalog
                    .as_ref()
                    .map_or(0, |catalog| catalog.catalog_revision)
                    < self
                        .collection_ui
                        .wanted_catalog_revision
                        .max(minimum_revision)
            {
                self.collection_ui
                    .request_catalog(self.collection_ui.wanted_catalog_revision);
            }
        }

        if let Some(mut request) = self.collection_ui.snapshot_request.take() {
            let collection_id = request.collection_id;
            let minimum_revision = request.minimum_revision;
            let reply = request.receiver.try_recv();
            if !matches!(&reply, Err(crossbeam_channel::TryRecvError::Empty))
                && let Some(start) = request.queued_at.take()
            {
                let (entries, outcome) = match &reply {
                    Ok(Ok(snapshot))
                        if self.collection_ui.selected_id != Some(collection_id)
                            || snapshot.collection_id() != collection_id
                            || snapshot.revision()
                                < self
                                    .collection_ui
                                    .wanted_collection_revision
                                    .max(minimum_revision) =>
                    {
                        (snapshot.entries.len(), "stale")
                    }
                    Ok(Ok(snapshot)) => (snapshot.entries.len(), "ok"),
                    Ok(Err(_)) => (0, "error"),
                    Err(_) => (0, "disconnected"),
                };
                collection_ui_actor_rtt(
                    "load_collection",
                    Some(collection_id),
                    minimum_revision,
                    entries,
                    outcome,
                    start,
                );
            }
            match reply {
                Ok(Ok(snapshot)) => self.collection_ui.install_snapshot(snapshot),
                Ok(Err(CollectionStoreError::NotFound)) => {
                    self.collection_ui
                        .request_catalog(self.collection_ui.wanted_catalog_revision);
                }
                Ok(Err(error)) => {
                    self.collection_ui.message = Some((true, collection_error_message(&error)))
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    self.collection_ui.snapshot_request = Some(request)
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.collection_ui.message =
                        Some((true, "コレクション内容の応答が失われました。".into()))
                }
            }
            if self.collection_ui.snapshot_request.is_none()
                && self.collection_ui.selected_id == Some(collection_id)
                && self
                    .collection_ui
                    .snapshot
                    .as_ref()
                    .map_or(0, CollectionSnapshot::revision)
                    < self
                        .collection_ui
                        .wanted_collection_revision
                        .max(minimum_revision)
            {
                self.collection_ui
                    .request_snapshot(collection_id, self.collection_ui.wanted_collection_revision);
            }
        }

        self.poll_collection_operation(ctx);
        self.poll_collection_reorder(ctx);
        self.reconcile_collection_toolbar_target();
        if matches!(self.collection_ui.phase, CollectionRuntimePhase::Starting)
            || self.collection_ui.catalog_request.is_some()
            || self.collection_ui.snapshot_request.is_some()
            || !self.collection_ui.operation.is_idle()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn poll_collection_operation(&mut self, _ctx: &egui::Context) {
        let operation = std::mem::replace(
            &mut self.collection_ui.operation,
            CollectionDialogOperation::Idle,
        );
        self.collection_ui.operation = match operation {
            CollectionDialogOperation::ReadingImport {
                collection_id,
                expected_revision,
                source_path,
                origin,
                mut task,
            } => match task.poll_finished() {
                None => CollectionDialogOperation::ReadingImport {
                    collection_id,
                    expected_revision,
                    source_path,
                    origin,
                    task,
                },
                Some(Ok(Ok(preview))) => CollectionDialogOperation::PreviewImport {
                    collection_id,
                    expected_revision,
                    source_path,
                    preview,
                    origin,
                },
                Some(Ok(Err(error))) | Some(Err(error)) => {
                    self.collection_ui.message = Some((true, error));
                    CollectionDialogOperation::Idle
                }
            },
            CollectionDialogOperation::Classifying {
                target,
                progress,
                mut task,
            } => match task.poll_finished() {
                None => CollectionDialogOperation::Classifying {
                    target,
                    progress,
                    task,
                },
                Some(Err(error)) => {
                    if let ClassificationTarget::Add { origin, .. } = &target {
                        self.report_collection_add_result(origin, true, error);
                    } else {
                        self.collection_ui.message = Some((true, error));
                    }
                    CollectionDialogOperation::Idle
                }
                Some(Ok(result)) if result.cancelled => {
                    if let ClassificationTarget::Add { origin, .. } = &target {
                        self.report_collection_add_result(
                            origin,
                            false,
                            "追加を取り消しました。".into(),
                        );
                    } else {
                        self.collection_ui.message = Some((false, "処理を取り消しました。".into()));
                    }
                    CollectionDialogOperation::Idle
                }
                Some(Ok(result)) => self.submit_collection_classification(target, result.prepared),
            },
            CollectionDialogOperation::ToolbarAddSnapshot(request) => {
                match request.receiver.try_recv() {
                    Ok(Ok(snapshot)) if snapshot.collection_id() == request.collection_id => {
                        let collection_name = snapshot.definition.name.clone();
                        self.collection_classification_operation(
                            request.paths,
                            ClassificationTarget::Add {
                                collection_id: request.collection_id,
                                expected_revision: snapshot.revision(),
                                import_errors: Vec::new(),
                                origin: CollectionAddOrigin::Toolbar { collection_name },
                            },
                        )
                    }
                    Ok(Ok(_)) => {
                        let message = "追加先のコレクション応答が一致しませんでした。".to_owned();
                        self.collection_ui.message = Some((true, message.clone()));
                        self.show_feedback_toast(format!(
                            "「{}」へ追加できませんでした: {message}",
                            request.collection_name
                        ));
                        CollectionDialogOperation::Idle
                    }
                    Ok(Err(error)) => {
                        let message = collection_error_message(&error);
                        self.collection_ui.message = Some((true, message.clone()));
                        self.show_feedback_toast(format!(
                            "「{}」へ追加できませんでした: {message}",
                            request.collection_name
                        ));
                        CollectionDialogOperation::Idle
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {
                        CollectionDialogOperation::ToolbarAddSnapshot(request)
                    }
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        let message = "追加先のコレクション応答が失われました。".to_owned();
                        self.collection_ui.message = Some((true, message.clone()));
                        self.show_feedback_toast(format!(
                            "「{}」へ追加できませんでした: {message}",
                            request.collection_name
                        ));
                        CollectionDialogOperation::Idle
                    }
                }
            }
            CollectionDialogOperation::GridSnapshot(request) => match request.receiver.try_recv() {
                Ok(Ok(snapshot))
                    if snapshot.collection_id() == request.target.stamp.collection_id
                        && snapshot.revision() >= request.target.expected_revision
                        && self.collection_grid_content_target().is_ok_and(|current| {
                            current.stamp == request.target.stamp
                                && current.expected_revision == request.target.expected_revision
                        })
                        && (!request.action.requires_exact_root_revision()
                            || snapshot.revision() == request.target.expected_revision) =>
                {
                    self.continue_collection_grid_snapshot(request.target, request.action, snapshot)
                }
                Ok(Ok(_)) => {
                    self.show_feedback_toast(
                        "表示が切り替わったため、コレクション操作を取り消しました。".into(),
                    );
                    CollectionDialogOperation::Idle
                }
                Ok(Err(error)) => {
                    let message = collection_error_message(&error);
                    self.show_feedback_toast(message.clone());
                    self.collection_ui.message = Some((true, message));
                    CollectionDialogOperation::Idle
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    CollectionDialogOperation::GridSnapshot(request)
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.show_feedback_toast("コレクション内容の応答が失われました。".into());
                    CollectionDialogOperation::Idle
                }
            },
            CollectionDialogOperation::Submitting(task) => self.poll_collection_actor_task(task),
            CollectionDialogOperation::ExportSnapshot {
                collection_id,
                destination,
                origin,
                receiver,
            } => match receiver.try_recv() {
                Ok(Ok(snapshot)) => {
                    self.start_collection_export_worker(snapshot, destination, origin)
                }
                Ok(Err(error)) => {
                    self.report_collection_operation_result(
                        origin,
                        true,
                        collection_error_message(&error),
                    );
                    CollectionDialogOperation::Idle
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    CollectionDialogOperation::ExportSnapshot {
                        collection_id,
                        destination,
                        origin,
                        receiver,
                    }
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.report_collection_operation_result(
                        origin,
                        true,
                        "エクスポート用一覧の応答が失われました。".into(),
                    );
                    CollectionDialogOperation::Idle
                }
            },
            CollectionDialogOperation::ExportWriting {
                collection_id,
                collection_revision,
                destination,
                origin,
                progress,
                mut task,
            } => match task.poll_finished() {
                None => CollectionDialogOperation::ExportWriting {
                    collection_id,
                    collection_revision,
                    destination,
                    origin,
                    progress,
                    task,
                },
                Some(Ok(Ok(()))) => {
                    self.collection_ui.message = Some((
                        false,
                        format!(
                            "revision {collection_revision} を {} へエクスポートしました。",
                            destination.display()
                        ),
                    ));
                    if matches!(origin, CollectionOperationOrigin::Grid(_)) {
                        self.show_feedback_toast(format!(
                            "{} へエクスポートしました。",
                            destination.display()
                        ));
                    }
                    CollectionDialogOperation::Idle
                }
                Some(Ok(Err(CollectionPrepareError::Cancelled))) => {
                    self.report_collection_operation_result(
                        origin,
                        false,
                        "エクスポートを取り消しました。".into(),
                    );
                    CollectionDialogOperation::Idle
                }
                Some(Ok(Err(error))) => {
                    self.report_collection_operation_result(origin, true, error.to_string());
                    CollectionDialogOperation::Idle
                }
                Some(Err(error)) => {
                    self.report_collection_operation_result(origin, true, error);
                    CollectionDialogOperation::Idle
                }
            },
            operation => operation,
        };
    }

    pub(crate) fn start_collection_grid_content_action(
        &mut self,
        target: crate::app::collection_grid::CollectionGridContentTarget,
        action: CollectionGridSnapshotAction,
    ) {
        if !self.collection_ui.can_edit() || !self.collection_ui.operation.is_idle() {
            self.show_feedback_toast("別のコレクション処理が進行中です。".into());
            return;
        }
        if !self.collection_grid_request_stamp_is_current(target.stamp) {
            self.show_feedback_toast(
                "現在のコレクション一覧を更新してから操作してください。".into(),
            );
            return;
        }
        let Some(client) = self.collection_ui.client.clone() else {
            self.show_feedback_toast("コレクションを利用できません。".into());
            return;
        };
        match client.load_collection(target.stamp.collection_id) {
            Ok(receiver) => {
                self.collection_ui.operation =
                    CollectionDialogOperation::GridSnapshot(CollectionGridSnapshotRequest {
                        target,
                        action,
                        receiver,
                    });
            }
            Err(error) => self.show_feedback_toast(collection_error_message(&error)),
        }
    }

    fn continue_collection_grid_snapshot(
        &mut self,
        target: crate::app::collection_grid::CollectionGridContentTarget,
        action: CollectionGridSnapshotAction,
        snapshot: CollectionSnapshot,
    ) -> CollectionDialogOperation {
        let stamp = target.stamp;
        let origin = CollectionOperationOrigin::Grid(stamp);
        match action {
            CollectionGridSnapshotAction::Import(path) => self.collection_import_read_operation(
                snapshot.collection_id(),
                snapshot.revision(),
                path,
                CollectionAddOrigin::Grid(stamp),
            ),
            CollectionGridSnapshotAction::Export(path) => {
                self.start_collection_export_worker(snapshot, path, origin)
            }
            CollectionGridSnapshotAction::SetOrder { mode, sort } => {
                let Some(client) = &self.collection_ui.client else {
                    return CollectionDialogOperation::Idle;
                };
                match client.set_order(snapshot.collection_id(), snapshot.revision(), mode, sort) {
                    Ok(receiver) => {
                        CollectionDialogOperation::Submitting(ActorTask::SnapshotMutation {
                            collection_id: snapshot.collection_id(),
                            origin,
                            receiver,
                        })
                    }
                    Err(error) => {
                        self.show_feedback_toast(collection_error_message(&error));
                        CollectionDialogOperation::Idle
                    }
                }
            }
            CollectionGridSnapshotAction::OpenReorder => {
                if snapshot.definition.order_mode != CollectionOrderMode::Manual {
                    self.show_feedback_toast("手動順のコレクションだけを並べ替えられます。".into());
                    return CollectionDialogOperation::Idle;
                }
                if snapshot.entries.is_empty() {
                    self.show_feedback_toast("並べ替える参照がありません。".into());
                    return CollectionDialogOperation::Idle;
                }
                self.open_collection_reorder(snapshot, target.selected_entry_id);
                CollectionDialogOperation::Idle
            }
        }
    }

    fn submit_collection_classification(
        &mut self,
        target: ClassificationTarget,
        prepared: Vec<CollectionPreparedRegistration>,
    ) -> CollectionDialogOperation {
        let mut registrations = Vec::new();
        let mut errors = Vec::new();
        for item in prepared {
            match item.result {
                Ok(registration) => registrations.push(registration),
                Err(error) => errors.push(format!("{}: {error}", item.path.display())),
            }
        }
        let Some(client) = self.collection_ui.client.clone() else {
            self.collection_ui.message = Some((true, "コレクションを利用できません。".into()));
            return CollectionDialogOperation::Idle;
        };
        match target {
            ClassificationTarget::Add {
                collection_id,
                expected_revision,
                import_errors,
                origin,
            } => {
                if origin
                    .grid_stamp()
                    .is_some_and(|stamp| !self.collection_grid_request_stamp_is_current(stamp))
                {
                    self.report_collection_add_result(
                        &origin,
                        false,
                        "表示が切り替わったため、追加を取り消しました。".into(),
                    );
                    return CollectionDialogOperation::Idle;
                }
                errors.extend(import_errors);
                if !errors.is_empty() {
                    self.report_collection_add_result(
                        &origin,
                        true,
                        format!(
                            "確認できない項目があるため、追加していません。\n{}",
                            errors.join("\n")
                        ),
                    );
                    return CollectionDialogOperation::Idle;
                }
                if registrations.is_empty() {
                    self.report_collection_add_result(
                        &origin,
                        false,
                        "追加する項目がありません。".into(),
                    );
                    return CollectionDialogOperation::Idle;
                }
                match client.add_batch(collection_id, expected_revision, registrations) {
                    Ok(receiver) => CollectionDialogOperation::Submitting(ActorTask::Add {
                        collection_id,
                        errors,
                        origin,
                        receiver,
                    }),
                    Err(error) => {
                        self.report_collection_add_result(
                            &origin,
                            true,
                            collection_error_message(&error),
                        );
                        CollectionDialogOperation::Idle
                    }
                }
            }
            ClassificationTarget::Relink {
                collection_id,
                expected_revision,
                entry_id,
                origin,
            } => {
                if origin
                    .grid_stamp()
                    .is_some_and(|stamp| !self.collection_grid_request_stamp_is_current(stamp))
                {
                    self.report_collection_operation_result(
                        origin,
                        false,
                        "表示が切り替わったため、再リンクを取り消しました。".into(),
                    );
                    return CollectionDialogOperation::Idle;
                }
                if registrations.len() != 1 || !errors.is_empty() {
                    self.report_collection_operation_result(
                        origin,
                        true,
                        errors
                            .into_iter()
                            .next()
                            .unwrap_or_else(|| "再リンク先を確認できませんでした。".into()),
                    );
                    return CollectionDialogOperation::Idle;
                }
                match client.relink(
                    collection_id,
                    expected_revision,
                    entry_id,
                    registrations.remove(0),
                ) {
                    Ok(receiver) => {
                        CollectionDialogOperation::Submitting(ActorTask::SnapshotMutation {
                            collection_id,
                            origin,
                            receiver,
                        })
                    }
                    Err(error) => {
                        self.report_collection_operation_result(
                            origin,
                            true,
                            collection_error_message(&error),
                        );
                        CollectionDialogOperation::Idle
                    }
                }
            }
        }
    }

    fn poll_collection_actor_task(&mut self, task: ActorTask) -> CollectionDialogOperation {
        match task {
            ActorTask::Create { receiver } => match receiver.try_recv() {
                Ok(Ok(snapshot)) => {
                    let id = snapshot.collection_id();
                    self.collection_ui.selected_id = Some(id);
                    self.collection_ui.install_snapshot(snapshot);
                    self.collection_ui
                        .request_catalog(self.collection_ui.wanted_catalog_revision);
                    self.collection_ui.message =
                        Some((false, "コレクションを作成しました。".into()));
                    CollectionDialogOperation::Idle
                }
                Ok(Err(error)) => self.finish_actor_error(error),
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    CollectionDialogOperation::Submitting(ActorTask::Create { receiver })
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.finish_actor_disconnect()
                }
            },
            ActorTask::SnapshotMutation {
                collection_id,
                origin,
                receiver,
            } => match receiver.try_recv() {
                Ok(Ok(snapshot)) => {
                    if self.collection_ui.selected_id == Some(collection_id) {
                        self.collection_ui.install_snapshot(snapshot);
                    }
                    self.collection_ui
                        .request_catalog(self.collection_ui.wanted_catalog_revision);
                    self.collection_ui.message =
                        Some((false, "コレクションを更新しました。".into()));
                    if matches!(origin, CollectionOperationOrigin::Grid(_)) {
                        self.show_feedback_toast("コレクションを更新しました。".into());
                    }
                    CollectionDialogOperation::Idle
                }
                Ok(Err(error)) => {
                    if matches!(origin, CollectionOperationOrigin::Grid(_)) {
                        self.show_feedback_toast(collection_error_message(&error));
                    }
                    self.finish_actor_error(error)
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    CollectionDialogOperation::Submitting(ActorTask::SnapshotMutation {
                        collection_id,
                        origin,
                        receiver,
                    })
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    if matches!(origin, CollectionOperationOrigin::Grid(_)) {
                        self.show_feedback_toast("コレクション保存の応答が失われました。".into());
                    }
                    self.finish_actor_disconnect()
                }
            },
            ActorTask::ContextRemove {
                origin,
                collection_id,
                entry_ids,
                receiver,
            } => match receiver.try_recv() {
                Ok(Ok(snapshot)) => {
                    if self.collection_ui.selected_id == Some(collection_id) {
                        self.collection_ui.install_snapshot(snapshot);
                    }
                    self.collection_ui
                        .request_catalog(self.collection_ui.wanted_catalog_revision);
                    self.collection_ui.message = Some((
                        false,
                        format!(
                            "{} 件をコレクションから外しました。元ファイルは変更していません。",
                            entry_ids.len()
                        ),
                    ));
                    self.show_feedback_toast(format!(
                        "{} 件をコレクションから外しました。元ファイルは変更していません。",
                        entry_ids.len()
                    ));
                    // The actor commit is global. The origin stamp is deliberately consumed only
                    // as response provenance; Grid convergence comes from the revision watch.
                    self.apply_collection_grid_remove_success(origin, &entry_ids);
                    CollectionDialogOperation::Idle
                }
                Ok(Err(error)) => {
                    let message = collection_error_message(&error);
                    self.show_feedback_toast(message);
                    self.finish_actor_error(error)
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    CollectionDialogOperation::Submitting(ActorTask::ContextRemove {
                        origin,
                        collection_id,
                        entry_ids,
                        receiver,
                    })
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.show_feedback_toast("コレクション保存の応答が失われました。".into());
                    self.finish_actor_disconnect()
                }
            },
            ActorTask::Delete { deleted, receiver } => match receiver.try_recv() {
                Ok(Ok(catalog)) => {
                    if self.collection_ui.selected_id == Some(deleted) {
                        self.collection_ui.selected_id = None;
                        self.collection_ui.snapshot = None;
                    }
                    self.collection_ui.install_catalog(catalog);
                    self.prune_collection_folder_history_from_ready_catalog();
                    self.collection_ui.message = Some((
                        false,
                        "コレクションを削除しました。元ファイルは変更していません。".into(),
                    ));
                    CollectionDialogOperation::Idle
                }
                Ok(Err(error)) => self.finish_actor_error(error),
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    CollectionDialogOperation::Submitting(ActorTask::Delete { deleted, receiver })
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.finish_actor_disconnect()
                }
            },
            ActorTask::Add {
                collection_id,
                errors,
                origin,
                receiver,
            } => match receiver.try_recv() {
                Ok(Ok(outcome)) => {
                    if self.collection_ui.selected_id == Some(collection_id) {
                        self.collection_ui.install_snapshot(outcome.snapshot);
                    }
                    self.collection_ui
                        .request_catalog(self.collection_ui.wanted_catalog_revision);
                    let mut message = format!("{} 件を追加しました。", outcome.added.len());
                    if !outcome.duplicates.is_empty() {
                        message.push_str(&format!(
                            " 重複 {} 件は追加していません。",
                            outcome.duplicates.len()
                        ));
                    }
                    if !errors.is_empty() {
                        message.push_str("\n");
                        message.push_str(&errors.join("\n"));
                    }
                    self.report_collection_add_result(&origin, !errors.is_empty(), message);
                    CollectionDialogOperation::Idle
                }
                Ok(Err(error)) => {
                    let message = collection_error_message(&error);
                    let operation = self.finish_actor_error(error);
                    if origin.is_detached() {
                        self.report_collection_add_result(&origin, true, message);
                    }
                    operation
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    CollectionDialogOperation::Submitting(ActorTask::Add {
                        collection_id,
                        errors,
                        origin,
                        receiver,
                    })
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    let operation = self.finish_actor_disconnect();
                    if origin.is_detached() {
                        self.report_collection_add_result(
                            &origin,
                            true,
                            "コレクション保存の応答が失われました。".into(),
                        );
                    }
                    operation
                }
            },
        }
    }

    fn report_collection_add_result(
        &mut self,
        origin: &CollectionAddOrigin,
        is_error: bool,
        message: String,
    ) {
        self.collection_ui.message = Some((is_error, message.clone()));
        if let Some(collection_name) = origin.collection_name() {
            let prefix = if is_error {
                format!("「{collection_name}」へ追加できませんでした")
            } else {
                format!("「{collection_name}」")
            };
            self.show_feedback_toast(format!("{prefix}: {message}"));
        } else if matches!(origin, CollectionAddOrigin::Grid(_)) {
            self.show_feedback_toast(message);
        }
    }

    fn report_collection_operation_result(
        &mut self,
        origin: CollectionOperationOrigin,
        is_error: bool,
        message: String,
    ) {
        self.collection_ui.message = Some((is_error, message.clone()));
        if origin.grid_stamp().is_some() {
            self.show_feedback_toast(message);
        }
    }

    fn finish_actor_error(&mut self, error: CollectionStoreError) -> CollectionDialogOperation {
        let conflict = matches!(error, CollectionStoreError::Conflict { .. });
        self.collection_ui.message = Some((true, collection_error_message(&error)));
        if conflict {
            self.collection_ui
                .request_catalog(self.collection_ui.wanted_catalog_revision);
            if let Some(id) = self.collection_ui.selected_id {
                self.collection_ui.request_snapshot(id, 0);
            }
        }
        CollectionDialogOperation::Idle
    }

    fn finish_actor_disconnect(&mut self) -> CollectionDialogOperation {
        self.collection_ui.message = Some((true, "コレクション保存の応答が失われました。".into()));
        CollectionDialogOperation::Idle
    }

    fn start_collection_export_worker(
        &mut self,
        snapshot: CollectionSnapshot,
        destination: PathBuf,
        origin: CollectionOperationOrigin,
    ) -> CollectionDialogOperation {
        let display_order = self.settings.grid_display_order.clone();
        let progress = Arc::new((
            AtomicUsize::new(0),
            AtomicUsize::new(snapshot.entries.len()),
        ));
        let worker_progress = Arc::clone(&progress);
        let collection_id = snapshot.collection_id();
        let collection_revision = snapshot.revision();
        let worker_destination = destination.clone();
        let task = WorkerTask::spawn("collection-export", move |cancel| {
            let perf_start = crate::perf::is_enabled().then(Instant::now);
            let result = (|| {
                let prepared = prepare_collection_export(
                    &snapshot,
                    &display_order,
                    &cancel,
                    |done, total| {
                        worker_progress.0.store(done, Ordering::Release);
                        worker_progress.1.store(total, Ordering::Release);
                    },
                )?;
                let contents =
                    serialize_collection_paths(prepared.ordered_paths.iter().map(PathBuf::as_path));
                write_collection_export_atomic(&worker_destination, contents.as_bytes(), &cancel)
            })();
            if let Some(start) = perf_start {
                crate::perf::event(
                    "collection",
                    "export",
                    None,
                    0,
                    &[
                        (
                            "collection_id",
                            serde_json::Value::from(collection_id.as_uuid().to_string()),
                        ),
                        ("revision", serde_json::Value::from(collection_revision)),
                        ("entries", serde_json::Value::from(snapshot.entries.len())),
                        (
                            "ms",
                            serde_json::Value::from(start.elapsed().as_secs_f64() * 1000.0),
                        ),
                        (
                            "outcome",
                            serde_json::Value::from(match &result {
                                Ok(_) => "ok",
                                Err(CollectionPrepareError::Cancelled) => "cancelled",
                                Err(_) => "error",
                            }),
                        ),
                    ],
                );
            }
            result
        });
        match task {
            Ok(task) => CollectionDialogOperation::ExportWriting {
                collection_id,
                collection_revision,
                destination,
                origin,
                progress,
                task,
            },
            Err(error) => {
                self.collection_ui.message = Some((true, error));
                CollectionDialogOperation::Idle
            }
        }
    }

    fn start_collection_export_snapshot_request(
        &mut self,
        collection_id: CollectionId,
        destination: PathBuf,
    ) {
        if !self.collection_ui.can_edit() {
            self.collection_ui.message = Some((true, "コレクションを現在編集できません。".into()));
            return;
        }
        let Some(client) = &self.collection_ui.client else {
            self.collection_ui.message = Some((true, "コレクションを利用できません。".into()));
            return;
        };
        match client.load_collection(collection_id) {
            Ok(receiver) => {
                self.collection_ui.operation = CollectionDialogOperation::ExportSnapshot {
                    collection_id,
                    destination,
                    origin: CollectionOperationOrigin::Manager,
                    receiver,
                };
            }
            Err(error) => {
                self.collection_ui.message = Some((true, collection_error_message(&error)))
            }
        }
    }

    pub(crate) fn show_collection_manager(&mut self, ctx: &egui::Context) {
        if !self.collection_ui.show_manager {
            self.show_detached_collection_operation_status(ctx);
            self.show_collection_operation_modal(ctx);
            return;
        }
        let catalog = self.collection_ui.catalog.clone();
        let phase = self.collection_ui.phase.clone();
        let busy = !self.collection_ui.operation.is_idle();
        let can_edit = matches!(phase, CollectionRuntimePhase::Ready);
        let toolbar_target_id = self
            .settings
            .toolbar_collection_target_id
            .map(CollectionId::from_uuid);
        let toolbar_target_name = toolbar_target_id.and_then(|id| {
            catalog.as_ref().and_then(|catalog| {
                catalog
                    .definitions
                    .iter()
                    .find(|definition| definition.id == id)
                    .map(|definition| definition.name.clone())
            })
        });
        let mut open = true;
        let mut action = None;
        let mut cancel_worker = false;
        egui::Window::new("コレクションの管理")
            .id(egui::Id::new("collection_manager"))
            .open(&mut open)
            .default_size(egui::vec2(720.0, 440.0))
            .min_size(egui::vec2(620.0, 320.0))
            .resizable(true)
            .show(ctx, |ui| {
                ui.set_min_width(660.0);
                match &phase {
                    CollectionRuntimePhase::Inert => {
                        ui.colored_label(ui.visuals().warn_fg_color, "この起動ではコレクション機能が初期化されていません。");
                    }
                    CollectionRuntimePhase::Starting => {
                        ui.spinner();
                        ui.label("コレクションを準備しています…");
                    }
                    CollectionRuntimePhase::Failed(error) => {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                    CollectionRuntimePhase::Closed => {
                        ui.colored_label(ui.visuals().warn_fg_color, "コレクションは終了しました。");
                    }
                    CollectionRuntimePhase::Ready => {}
                }

                ui.horizontal(|ui| {
                    ui.label("現在の追加先のコレクション");
                    ui.strong(toolbar_target_name.as_deref().unwrap_or("未選択"));
                    if ui
                        .add_enabled(
                            toolbar_target_id.is_some(),
                            egui::Button::new("開く"),
                        )
                        .clicked()
                    {
                        action = toolbar_target_id.map(CollectionUiAction::Open);
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("新しいコレクション");
                    crate::ime_focus::add_singleline(
                        ui,
                        &mut self.collection_ui.manager_new_name,
                        None,
                        |edit| edit.desired_width(220.0),
                    );
                    let name = self.collection_ui.manager_new_name.trim().to_owned();
                    if ui
                        .add_enabled(
                            !name.is_empty() && !busy && can_edit,
                            egui::Button::new("作成"),
                        )
                        .clicked()
                    {
                        action = Some(CollectionUiAction::Create(name));
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("コレクション一覧");
                    if busy {
                        ui.label(egui::RichText::new("処理中…").weak());
                    }
                });
                egui::ScrollArea::vertical()
                    .id_salt("collection_definitions")
                    .max_height(300.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| match &catalog {
                        Some(catalog) if catalog.definitions.is_empty() => {
                            ui.weak("コレクションはまだありません。");
                        }
                        Some(catalog) => {
                            let definition_ids = catalog
                                .definitions
                                .iter()
                                .map(|definition| definition.id)
                                .collect::<HashSet<_>>();
                            self.collection_ui
                                .manager_rename_inputs
                                .retain(|id, _| definition_ids.contains(id));
                            for definition in catalog.definitions.iter() {
                                let is_target = toolbar_target_id == Some(definition.id);
                                let mut pinned = self
                                    .settings
                                    .pinned_collections
                                    .contains(&definition.id.as_uuid());
                                ui.horizontal(|ui| {
                                    if is_target {
                                        ui.strong("●");
                                    } else {
                                        ui.label(" ");
                                    }
                                    let input = self
                                        .collection_ui
                                        .manager_rename_inputs
                                        .entry(definition.id)
                                        .or_insert_with(|| definition.name.clone());
                                    crate::ime_focus::add_singleline(ui, input, None, |edit| {
                                        edit.desired_width(220.0)
                                    });
                                    let new_name = input.trim().to_owned();
                                    let can_rename = !new_name.is_empty()
                                        && new_name != definition.name
                                        && !busy
                                        && can_edit;
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.button("開く").clicked() {
                                                action = Some(CollectionUiAction::Open(definition.id));
                                            }
                                            if ui
                                                .add_enabled(
                                                    !busy && can_edit,
                                                    egui::Button::new("削除"),
                                                )
                                                .on_hover_text("定義と参照だけを削除します。元ファイルは削除しません。")
                                                .clicked()
                                            {
                                                action = Some(CollectionUiAction::OpenDelete {
                                                    id: definition.id,
                                                    revision: definition.revision,
                                                    name: definition.name.clone(),
                                                });
                                            }
                                            if ui
                                                .add_enabled(
                                                    !is_target && !busy && can_edit,
                                                    egui::Button::new("追加先にする"),
                                                )
                                                .clicked()
                                            {
                                                action = Some(CollectionUiAction::SetTarget(definition.id));
                                            }
                                            if ui
                                                .add_enabled(
                                                    can_rename,
                                                    egui::Button::new("名前変更"),
                                                )
                                                .clicked()
                                            {
                                                action = Some(CollectionUiAction::Rename {
                                                    id: definition.id,
                                                    revision: definition.revision,
                                                    name: new_name.clone(),
                                                });
                                            }
                                            if ui
                                                .selectable_label(pinned, "固定")
                                                .on_hover_text("ツールバーにこのコレクションのボタンを固定表示する")
                                                .clicked()
                                            {
                                                pinned = !pinned;
                                                action = Some(CollectionUiAction::SetPinned {
                                                    id: definition.id,
                                                    pinned,
                                                });
                                            }
                                        },
                                    );
                                });
                            }
                        }
                        None if matches!(phase, CollectionRuntimePhase::Ready) => {
                            ui.spinner();
                            ui.label("一覧を読み込んでいます…");
                        }
                        None => {}
                    });

                if let Some((is_error, message)) = &self.collection_ui.message {
                    ui.separator();
                    if *is_error {
                        ui.colored_label(ui.visuals().error_fg_color, message);
                    } else {
                        ui.label(message);
                    }
                }
                cancel_worker = draw_collection_operation_status(ui, &self.collection_ui.operation);
            });

        if cancel_worker {
            let toolbar_origin = self.collection_ui.operation.toolbar_add_origin().cloned();
            self.collection_ui.operation.cancel_worker();
            self.collection_ui.operation = CollectionDialogOperation::Idle;
            if let Some(origin) = toolbar_origin {
                self.report_collection_add_result(&origin, false, "追加を取り消しました。".into());
            } else {
                self.collection_ui.message = Some((false, "処理を取り消しました。".into()));
            }
        }

        if !open {
            match self.collection_ui.operation.manager_close_behavior() {
                CollectionManagerCloseBehavior::KeepOpen => {
                    self.collection_ui.show_manager = true;
                }
                CollectionManagerCloseBehavior::Detach => {
                    self.collection_ui.show_manager = false;
                }
                CollectionManagerCloseBehavior::Cancel => {
                    self.collection_ui.operation.cancel_worker();
                    self.collection_ui.operation = CollectionDialogOperation::Idle;
                    self.collection_ui.show_manager = false;
                }
            }
        }
        if let Some(action) = action {
            self.apply_collection_ui_action(action);
        }
        self.show_collection_operation_modal(ctx);
    }

    fn show_detached_collection_operation_status(&mut self, ctx: &egui::Context) {
        let show = self.collection_ui.operation.is_detached()
            && matches!(
                self.collection_ui.operation,
                CollectionDialogOperation::ReadingImport { .. }
                    | CollectionDialogOperation::Classifying { .. }
                    | CollectionDialogOperation::ToolbarAddSnapshot(_)
                    | CollectionDialogOperation::GridSnapshot(_)
                    | CollectionDialogOperation::Submitting(_)
                    | CollectionDialogOperation::ExportSnapshot { .. }
                    | CollectionDialogOperation::ExportWriting { .. }
            );
        if !show {
            return;
        }
        let mut cancel = false;
        egui::Window::new("コレクション処理")
            .id(egui::Id::new("collection_detached_operation_status"))
            .collapsible(false)
            .resizable(false)
            .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
            .show(ctx, |ui| {
                cancel = draw_collection_operation_status(ui, &self.collection_ui.operation);
            });
        if cancel {
            let add_origin = self.collection_ui.operation.toolbar_add_origin().cloned();
            self.collection_ui.operation.cancel_worker();
            self.collection_ui.operation = CollectionDialogOperation::Idle;
            if let Some(origin) = add_origin {
                self.report_collection_add_result(&origin, false, "追加を取り消しました。".into());
            } else {
                self.show_feedback_toast("コレクション処理を取り消しました。".into());
            }
        }
    }

    fn apply_collection_ui_action(&mut self, action: CollectionUiAction) {
        if action.requires_ready() && !self.collection_ui.can_edit() {
            self.collection_ui.message = Some((true, "コレクションを現在編集できません。".into()));
            return;
        }
        match action {
            CollectionUiAction::Open(id) => self.open_collection_grid_from_navigation(id),
            CollectionUiAction::SetTarget(id) => self.select_collection_toolbar_target(id),
            CollectionUiAction::SetPinned { id, pinned } => self.set_collection_pinned(id, pinned),
            CollectionUiAction::Create(name) => {
                self.collection_ui.manager_new_name.clear();
                self.collection_ui.operation =
                    self.submit_collection_name(NameAction::Create, name);
            }
            CollectionUiAction::Rename { id, revision, name } => {
                self.collection_ui.operation = self.submit_collection_name(
                    NameAction::Rename {
                        collection_id: id,
                        expected_revision: revision,
                    },
                    name,
                );
            }
            CollectionUiAction::OpenDelete { id, revision, name } => {
                self.collection_ui.operation = CollectionDialogOperation::ConfirmDelete {
                    collection_id: id,
                    expected_revision: revision,
                    name,
                };
            }
        }
    }

    fn start_collection_import_read(
        &mut self,
        collection_id: CollectionId,
        expected_revision: u64,
        source_path: PathBuf,
    ) {
        self.collection_ui.operation = self.collection_import_read_operation(
            collection_id,
            expected_revision,
            source_path,
            CollectionAddOrigin::Manager,
        );
    }

    fn collection_import_read_operation(
        &mut self,
        collection_id: CollectionId,
        expected_revision: u64,
        source_path: PathBuf,
        origin: CollectionAddOrigin,
    ) -> CollectionDialogOperation {
        let worker_path = source_path.clone();
        match WorkerTask::spawn("collection-import-read", move |cancel| {
            let perf_start = crate::perf::is_enabled().then(Instant::now);
            let read = std::fs::read_to_string(&worker_path);
            let bytes = read.as_ref().map_or(0, String::len);
            let result = read
                .map(|text| parse_collection_text(&text, &worker_path))
                .map_err(|error| format!("インポートファイルを読めませんでした: {error}"));
            if let Some(start) = perf_start {
                crate::perf::event(
                    "collection",
                    "import_parse",
                    None,
                    0,
                    &[
                        (
                            "collection_id",
                            serde_json::Value::from(collection_id.as_uuid().to_string()),
                        ),
                        ("revision", serde_json::Value::from(expected_revision)),
                        ("bytes", serde_json::Value::from(bytes)),
                        (
                            "lines",
                            serde_json::Value::from(
                                result.as_ref().map_or(0, |preview| preview.lines.len()),
                            ),
                        ),
                        (
                            "ms",
                            serde_json::Value::from(start.elapsed().as_secs_f64() * 1000.0),
                        ),
                        (
                            "outcome",
                            serde_json::Value::from(if cancel.load(Ordering::Acquire) {
                                "cancelled"
                            } else if result.is_ok() {
                                "ok"
                            } else {
                                "error"
                            }),
                        ),
                    ],
                );
            }
            result
        }) {
            Ok(task) => CollectionDialogOperation::ReadingImport {
                collection_id,
                expected_revision,
                source_path,
                origin,
                task,
            },
            Err(error) => {
                self.report_collection_add_result(&origin, true, error);
                CollectionDialogOperation::Idle
            }
        }
    }

    fn start_collection_classification(
        &mut self,
        paths: Vec<PathBuf>,
        target: ClassificationTarget,
    ) {
        self.collection_ui.operation = self.collection_classification_operation(paths, target);
    }

    fn collection_classification_operation(
        &mut self,
        paths: Vec<PathBuf>,
        target: ClassificationTarget,
    ) -> CollectionDialogOperation {
        let progress = Arc::new((AtomicUsize::new(0), AtomicUsize::new(paths.len())));
        let worker_progress = Arc::clone(&progress);
        let (collection_id, revision) = match &target {
            ClassificationTarget::Add {
                collection_id,
                expected_revision,
                ..
            }
            | ClassificationTarget::Relink {
                collection_id,
                expected_revision,
                ..
            } => (*collection_id, *expected_revision),
        };
        match WorkerTask::spawn("collection-source-prepare", move |cancel| {
            let perf_start = crate::perf::is_enabled().then(Instant::now);
            let prepared = prepare_collection_registrations(&paths, &cancel, |done, total| {
                worker_progress.0.store(done, Ordering::Release);
                worker_progress.1.store(total, Ordering::Release);
            });
            if let Some(start) = perf_start {
                crate::perf::event(
                    "collection",
                    "import_classify",
                    None,
                    0,
                    &[
                        (
                            "collection_id",
                            serde_json::Value::from(collection_id.as_uuid().to_string()),
                        ),
                        ("revision", serde_json::Value::from(revision)),
                        ("candidates", serde_json::Value::from(paths.len())),
                        ("completed", serde_json::Value::from(prepared.len())),
                        (
                            "ms",
                            serde_json::Value::from(start.elapsed().as_secs_f64() * 1000.0),
                        ),
                        (
                            "outcome",
                            serde_json::Value::from(if cancel.load(Ordering::Acquire) {
                                "cancelled"
                            } else {
                                "ok"
                            }),
                        ),
                    ],
                );
            }
            ClassificationResult {
                prepared,
                cancelled: cancel.load(Ordering::Acquire),
            }
        }) {
            Ok(task) => CollectionDialogOperation::Classifying {
                target,
                progress,
                task,
            },
            Err(error) => {
                if let ClassificationTarget::Add { origin, .. } = &target {
                    self.report_collection_add_result(origin, true, error);
                } else {
                    self.collection_ui.message = Some((true, error));
                }
                CollectionDialogOperation::Idle
            }
        }
    }

    fn collection_import_classification_operation(
        &mut self,
        collection_id: CollectionId,
        expected_revision: u64,
        preview: &CollectionImportPreview,
        origin: CollectionAddOrigin,
    ) -> CollectionDialogOperation {
        let paths = preview
            .accepted_paths()
            .map(|(path, _)| path.to_path_buf())
            .collect();
        let errors = preview
            .lines
            .iter()
            .filter_map(|line| match &line.status {
                CollectionImportLineStatus::Invalid { reason } => {
                    Some(format!("{} 行: {reason}", line.line_number))
                }
                _ => None,
            })
            .collect();
        self.collection_classification_operation(
            paths,
            ClassificationTarget::Add {
                collection_id,
                expected_revision,
                import_errors: errors,
                origin,
            },
        )
    }

    fn show_collection_operation_modal(&mut self, ctx: &egui::Context) {
        let operation = std::mem::replace(
            &mut self.collection_ui.operation,
            CollectionDialogOperation::Idle,
        );
        self.collection_ui.operation = match operation {
            CollectionDialogOperation::Name { action, mut draft } => {
                let mut keep = true;
                let mut submit = false;
                egui::Modal::new(egui::Id::new("collection_name_modal")).show(ctx, |ui| {
                    ui.heading(match action {
                        NameAction::Create => "コレクションを作成",
                        NameAction::Rename { .. } => "コレクション名を変更",
                    });
                    let response =
                        crate::ime_focus::add_singleline(ui, &mut draft, None, |edit| edit);
                    response.request_focus();
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!draft.trim().is_empty(), egui::Button::new("保存"))
                            .clicked()
                        {
                            submit = true;
                        }
                        if ui.button("キャンセル").clicked() {
                            keep = false;
                        }
                    });
                });
                if submit {
                    self.submit_collection_name(action, draft)
                } else if keep {
                    CollectionDialogOperation::Name { action, draft }
                } else {
                    CollectionDialogOperation::Idle
                }
            }
            CollectionDialogOperation::ConfirmDelete {
                collection_id,
                expected_revision,
                name,
            } => {
                let mut decision = None;
                egui::Modal::new(egui::Id::new("collection_delete_modal")).show(ctx, |ui| {
                    ui.heading("コレクションを削除");
                    ui.label(format!("「{name}」の定義と参照を削除します。"));
                    ui.label("元のファイルやフォルダは削除しません。");
                    ui.horizontal(|ui| {
                        if ui.button("削除").clicked() {
                            decision = Some(true);
                        }
                        if ui.button("キャンセル").clicked() {
                            decision = Some(false);
                        }
                    });
                });
                match decision {
                    Some(true) => self.submit_collection_delete(collection_id, expected_revision),
                    Some(false) => CollectionDialogOperation::Idle,
                    None => CollectionDialogOperation::ConfirmDelete {
                        collection_id,
                        expected_revision,
                        name,
                    },
                }
            }
            CollectionDialogOperation::ConfirmRemove {
                collection_id,
                expected_revision,
                entry_ids,
                origin,
            } => {
                let mut decision = None;
                egui::Modal::new(egui::Id::new("collection_remove_modal")).show(ctx, |ui| {
                    ui.heading("コレクションから外す");
                    ui.label(format!(
                        "{} 件の参照をコレクションから外します。",
                        entry_ids.len()
                    ));
                    ui.label("元のファイルやフォルダは削除しません。");
                    ui.horizontal(|ui| {
                        if ui.button("外す").clicked() {
                            decision = Some(true);
                        }
                        if ui.button("キャンセル").clicked() {
                            decision = Some(false);
                        }
                    });
                });
                match decision {
                    Some(true) => self.submit_collection_remove(
                        collection_id,
                        expected_revision,
                        entry_ids,
                        origin,
                    ),
                    Some(false) => CollectionDialogOperation::Idle,
                    None => CollectionDialogOperation::ConfirmRemove {
                        collection_id,
                        expected_revision,
                        entry_ids,
                        origin,
                    },
                }
            }
            CollectionDialogOperation::PreviewImport {
                collection_id,
                expected_revision,
                source_path,
                preview,
                origin,
            } => {
                if origin
                    .grid_stamp()
                    .is_some_and(|stamp| !self.collection_grid_request_stamp_is_current(stamp))
                {
                    self.report_collection_add_result(
                        &origin,
                        false,
                        "表示が切り替わったため、インポートを取り消しました。".into(),
                    );
                    return;
                }
                let mut decision = None;
                egui::Modal::new(egui::Id::new("collection_import_preview_modal")).show(ctx, |ui| {
                    ui.heading("インポート内容の確認");
                    ui.label(format!("追加先: {}", self.collection_ui.catalog.as_ref().and_then(|catalog| catalog.definitions.iter().find(|item| item.id == collection_id)).map(|item| item.name.as_str()).unwrap_or("不明")));
                    ui.weak("この確認までは対象ファイルやフォルダへアクセスしていません。確認後に存在と種類を調べます。");
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for line in &preview.lines {
                            let is_network = line.source_key.as_ref().is_some_and(|key| {
                                key.normalized_path().starts_with("//")
                            });
                            let status = match (&line.status, is_network) {
                                (CollectionImportLineStatus::Accepted, true) => {
                                    "追加候補・ネットワーク"
                                }
                                (CollectionImportLineStatus::Accepted, false) => "追加候補",
                                (CollectionImportLineStatus::Duplicate { .. }, _) => "重複",
                                (CollectionImportLineStatus::Invalid { .. }, _) => "無効",
                            };
                            ui.label(format!("{}: [{status}] {}", line.line_number, line.resolved_path.as_ref().map_or_else(|| line.original.clone(), |path| path.display().to_string())));
                        }
                    });
                    ui.horizontal(|ui| {
                        let accepted = preview.accepted_paths().count();
                        let invalid = preview
                            .lines
                            .iter()
                            .filter(|line| matches!(line.status, CollectionImportLineStatus::Invalid { .. }))
                            .count();
                        if ui
                            .add_enabled(
                                accepted > 0 && invalid == 0,
                                egui::Button::new(format!("確認して {accepted} 件を調べる")),
                            )
                            .on_disabled_hover_text("無効な行を直してからインポートしてください。")
                            .clicked()
                        {
                            decision = Some(true);
                        }
                        if ui.button("キャンセル").clicked() { decision = Some(false); }
                    });
                });
                match decision {
                    Some(true) => self.collection_import_classification_operation(
                        collection_id,
                        expected_revision,
                        &preview,
                        origin,
                    ),
                    Some(false) => CollectionDialogOperation::Idle,
                    None => CollectionDialogOperation::PreviewImport {
                        collection_id,
                        expected_revision,
                        source_path,
                        preview,
                        origin,
                    },
                }
            }
            operation => operation,
        };
    }

    fn submit_collection_name(
        &mut self,
        action: NameAction,
        draft: String,
    ) -> CollectionDialogOperation {
        let Some(client) = &self.collection_ui.client else {
            return CollectionDialogOperation::Idle;
        };
        let request = match action {
            NameAction::Create => client
                .create_collection(draft.trim().to_owned())
                .map(|receiver| ActorTask::Create { receiver }),
            NameAction::Rename {
                collection_id,
                expected_revision,
            } => client
                .rename_collection(collection_id, expected_revision, draft.trim().to_owned())
                .map(|receiver| ActorTask::SnapshotMutation {
                    collection_id,
                    origin: CollectionOperationOrigin::Manager,
                    receiver,
                }),
        };
        match request {
            Ok(task) => CollectionDialogOperation::Submitting(task),
            Err(error) => {
                self.collection_ui.message = Some((true, collection_error_message(&error)));
                CollectionDialogOperation::Idle
            }
        }
    }

    fn submit_collection_delete(
        &mut self,
        collection_id: CollectionId,
        expected_revision: u64,
    ) -> CollectionDialogOperation {
        let Some(client) = &self.collection_ui.client else {
            return CollectionDialogOperation::Idle;
        };
        match client.delete_collection(collection_id, expected_revision) {
            Ok(receiver) => CollectionDialogOperation::Submitting(ActorTask::Delete {
                deleted: collection_id,
                receiver,
            }),
            Err(error) => {
                self.collection_ui.message = Some((true, collection_error_message(&error)));
                CollectionDialogOperation::Idle
            }
        }
    }

    fn submit_collection_remove(
        &mut self,
        collection_id: CollectionId,
        expected_revision: u64,
        entry_ids: Vec<CollectionEntryId>,
        origin: CollectionOperationOrigin,
    ) -> CollectionDialogOperation {
        let Some(client) = &self.collection_ui.client else {
            return CollectionDialogOperation::Idle;
        };
        match client.remove_entries(collection_id, expected_revision, entry_ids.clone()) {
            Ok(receiver) => CollectionDialogOperation::Submitting(match origin {
                CollectionOperationOrigin::Grid(origin) => ActorTask::ContextRemove {
                    origin,
                    collection_id,
                    entry_ids,
                    receiver,
                },
                CollectionOperationOrigin::Manager => ActorTask::SnapshotMutation {
                    collection_id,
                    origin,
                    receiver,
                },
            }),
            Err(error) => {
                self.collection_ui.message = Some((true, collection_error_message(&error)));
                CollectionDialogOperation::Idle
            }
        }
    }

    pub(crate) fn draw_collection_reorder(&mut self, ctx: &egui::Context) {
        if self.collection_ui.reorder.is_none() {
            return;
        }
        self.refresh_collection_reorder_textures();
        let title = self
            .collection_ui
            .reorder
            .as_ref()
            .map(|state| format!("コレクション並べ替え: {}", state.collection_name))
            .unwrap_or_else(|| "コレクション並べ替え".into());
        let mut window_open = true;
        let mut close = false;
        let mut save = false;
        let mut refresh = false;
        let mut discard = false;
        egui::Window::new(title)
            .id(egui::Id::new("collection_reorder_window"))
            .open(&mut window_open)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(
                COLLECTION_REORDER_DEFAULT_WINDOW_W,
                COLLECTION_REORDER_DEFAULT_WINDOW_H,
            ))
            .min_size(egui::vec2(
                COLLECTION_REORDER_MIN_WINDOW_W,
                COLLECTION_REORDER_MIN_WINDOW_H,
            ))
            .show(ctx, |ui| {
                let Some(state) = self.collection_ui.reorder.as_mut() else {
                    return;
                };
                ensure_collection_reorder_selection(state);
                let busy = state.phase.is_busy();
                if let CollectionReorderPhase::Error { message, conflict } = &state.phase {
                    let conflict = *conflict;
                    ui.colored_label(ui.visuals().error_fg_color, message);
                    if conflict {
                        ui.label(
                            egui::RichText::new(
                                "外部更新は自動で混ぜません。編集中の順序を保持しています。",
                            )
                            .weak(),
                        );
                    }
                    ui.horizontal(|ui| {
                        if !conflict {
                            if ui
                                .add_enabled(state.dirty, egui::Button::new("この順序で再試行"))
                                .clicked()
                            {
                                save = true;
                            }
                        }
                        if ui.button("最新内容を読み直す（変更破棄）").clicked() {
                            refresh = true;
                        }
                        if ui.button("変更を破棄して閉じる").clicked() {
                            discard = true;
                        }
                    });
                    ui.separator();
                }
                ui.horizontal(|ui| {
                    let selected_indices = selected_collection_reorder_indices(state);
                    let selected_count = selected_indices.len();
                    let selected_set = selected_indices.iter().copied().collect::<HashSet<_>>();
                    let can_left = !busy
                        && selected_indices
                            .iter()
                            .any(|index| *index > 0 && !selected_set.contains(&(*index - 1)));
                    let can_right = !busy
                        && selected_indices.iter().any(|index| {
                            *index + 1 < state.entries.len()
                                && !selected_set.contains(&(*index + 1))
                        });
                    if ui
                        .add_enabled(can_left, egui::Button::new("←"))
                        .on_hover_text("左へ移動")
                        .clicked()
                        && move_selected_collection_reorder_by(state, -1)
                    {
                        state.dirty = true;
                    }
                    if ui
                        .add_enabled(can_right, egui::Button::new("→"))
                        .on_hover_text("右へ移動")
                        .clicked()
                        && move_selected_collection_reorder_by(state, 1)
                    {
                        state.dirty = true;
                    }
                    let selected_response = ui
                        .add_enabled(
                            !busy && selected_count > 0,
                            egui::Label::new(format!(
                                "選択 {selected_count} 件（ここをドラッグして移動）"
                            ))
                            .sense(egui::Sense::click_and_drag()),
                        )
                        .on_hover_cursor(egui::CursorIcon::Grab)
                        .on_hover_text("選択した参照全体をドラッグして移動");
                    if !busy
                        && selected_response.drag_started()
                        && let Some(source) = selected_indices.first().copied()
                    {
                        state.selected = Some(source);
                        state.dragging = Some(source);
                        state.drag_auto_scroll_enabled = false;
                        state.drag_insert_index = Some(source);
                    }
                    ui.separator();
                    ui.add_enabled(
                        !busy,
                        egui::Slider::new(
                            &mut state.thumb_tile_px,
                            COLLECTION_REORDER_MIN_TILE_PX..=COLLECTION_REORDER_MAX_TILE_PX,
                        )
                        .text("サムネ"),
                    );
                    ui.separator();
                    if ui.add_enabled(!busy, egui::Button::new("閉じる")).clicked() {
                        if state.dirty {
                            save = true;
                        } else {
                            close = true;
                        }
                    }
                    if busy {
                        ui.spinner();
                        let label = match &state.phase {
                            CollectionReorderPhase::Saving { .. } => "保存中…",
                            CollectionReorderPhase::Refreshing { .. } => "再読込中…",
                            _ => "処理中…",
                        };
                        ui.label(egui::RichText::new(label).weak());
                    }
                });
                ui.separator();

                state.thumb_tile_px = state.thumb_tile_px.clamp(
                    COLLECTION_REORDER_MIN_TILE_PX,
                    COLLECTION_REORDER_MAX_TILE_PX,
                );
                let tile = egui::vec2(state.thumb_tile_px, state.thumb_tile_px + 20.0);
                let gap = 8.0;
                let scroll_width = ui.available_width().max(tile.x + gap);
                let columns = collection_reorder_grid_columns(scroll_width, tile.x, gap);
                let rows = state.entries.len().div_ceil(columns).max(1);
                let row_height = tile.y + gap;
                let scroll_height =
                    collection_reorder_scroll_height(ui.available_height(), rows, row_height);
                if state.dragging.is_none() {
                    let max_offset = (rows as f32 * row_height - scroll_height).max(0.0);
                    if ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Home)
                    }) {
                        state.scroll_offset_y = 0.0;
                    } else if ui
                        .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::End))
                    {
                        state.scroll_offset_y = max_offset;
                    } else if ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::PageUp)
                    }) {
                        state.scroll_offset_y = (state.scroll_offset_y - scroll_height
                            + row_height)
                            .clamp(0.0, max_offset);
                    } else if ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::PageDown)
                    }) {
                        state.scroll_offset_y = (state.scroll_offset_y + scroll_height
                            - row_height)
                            .clamp(0.0, max_offset);
                    }
                }
                let pointer_released = ui.input(|input| input.pointer.any_released());
                let pointer_pos = ui.input(|input| {
                    input
                        .pointer
                        .hover_pos()
                        .or_else(|| input.pointer.interact_pos())
                });
                let mut move_request = None;
                state.drag_insert_index = None;
                ui.allocate_ui_with_layout(
                    egui::vec2(scroll_width, scroll_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        let scroll_output = egui::ScrollArea::vertical()
                            .id_salt("collection_reorder_thumb_scroll")
                            .vertical_scroll_offset(state.scroll_offset_y)
                            .max_height(scroll_height)
                            .auto_shrink([false, false])
                            .show_rows(ui, row_height, rows, |ui, row_range| {
                                egui::Grid::new("collection_reorder_thumb_grid")
                                    .num_columns(columns)
                                    .spacing(egui::vec2(gap, gap))
                                    .show(ui, |ui| {
                                        for row in row_range {
                                            for column in 0..columns {
                                                let index = row * columns + column;
                                                let Some(entry) = state.entries.get(index) else {
                                                    let (rect, _) = ui.allocate_exact_size(
                                                        tile,
                                                        egui::Sense::hover(),
                                                    );
                                                    if !busy
                                                        && state.dragging.is_some()
                                                        && pointer_pos
                                                            .is_some_and(|pos| rect.contains(pos))
                                                    {
                                                        state.drag_insert_index =
                                                            Some(state.entries.len());
                                                        draw_collection_reorder_insert_indicator(
                                                            ui,
                                                            rect.left(),
                                                            rect,
                                                        );
                                                        if pointer_released {
                                                            move_request =
                                                                Some(state.entries.len());
                                                        }
                                                    }
                                                    continue;
                                                };
                                                let entry_id = entry.entry.id;
                                                let path = entry.entry.source_path.clone();
                                                let kind = entry.entry.resolved_kind;
                                                let texture = entry.texture.clone();
                                                let selected =
                                                    state.selected_ids.contains(&entry_id);
                                                let (rect, response) = ui.allocate_exact_size(
                                                    tile,
                                                    egui::Sense::click_and_drag(),
                                                );
                                                let fill = if state.dragging.is_some() && selected {
                                                    ui.visuals().selection.bg_fill
                                                } else if selected {
                                                    ui.visuals().widgets.active.bg_fill
                                                } else {
                                                    ui.visuals().extreme_bg_color
                                                };
                                                ui.painter().rect_filled(rect, 4.0, fill);
                                                ui.painter().rect_stroke(
                                                    rect,
                                                    4.0,
                                                    egui::Stroke::new(
                                                        1.0,
                                                        if selected {
                                                            ui.visuals().selection.stroke.color
                                                        } else {
                                                            ui.visuals()
                                                                .widgets
                                                                .noninteractive
                                                                .bg_stroke
                                                                .color
                                                        },
                                                    ),
                                                    egui::StrokeKind::Inside,
                                                );
                                                let image_rect =
                                                    rect.shrink2(egui::vec2(6.0, 18.0));
                                                if let Some(texture) = texture.as_ref() {
                                                    let size = texture.size_vec2();
                                                    let scale = (image_rect.width() / size.x)
                                                        .min(image_rect.height() / size.y)
                                                        .min(1.0);
                                                    let paint_rect = egui::Rect::from_center_size(
                                                        image_rect.center(),
                                                        size * scale,
                                                    );
                                                    ui.painter().image(
                                                        texture.id(),
                                                        paint_rect,
                                                        egui::Rect::from_min_max(
                                                            egui::Pos2::ZERO,
                                                            egui::pos2(1.0, 1.0),
                                                        ),
                                                        egui::Color32::WHITE,
                                                    );
                                                } else {
                                                    ui.painter().text(
                                                        image_rect.center(),
                                                        egui::Align2::CENTER_CENTER,
                                                        kind.as_str(),
                                                        egui::FontId::proportional(11.0),
                                                        ui.visuals().weak_text_color(),
                                                    );
                                                }
                                                ui.painter().text(
                                                    rect.left_bottom() + egui::vec2(6.0, -5.0),
                                                    egui::Align2::LEFT_BOTTOM,
                                                    format!("{:04}", index + 1),
                                                    egui::FontId::monospace(11.0),
                                                    ui.visuals().text_color(),
                                                );
                                                let response = response.on_hover_ui(|ui| {
                                                    if let Some(texture) = texture.as_ref() {
                                                        let size = texture.size_vec2();
                                                        let scale = (360.0 / size.x)
                                                            .min(360.0 / size.y)
                                                            .min(2.5);
                                                        let (preview_rect, _) = ui
                                                            .allocate_exact_size(
                                                                size * scale,
                                                                egui::Sense::hover(),
                                                            );
                                                        ui.painter().image(
                                                            texture.id(),
                                                            preview_rect,
                                                            egui::Rect::from_min_max(
                                                                egui::Pos2::ZERO,
                                                                egui::pos2(1.0, 1.0),
                                                            ),
                                                            egui::Color32::WHITE,
                                                        );
                                                    } else {
                                                        ui.label("サムネイルを準備中");
                                                    }
                                                    ui.label(path.display().to_string());
                                                    ui.weak(kind.as_str());
                                                });
                                                if !busy && response.clicked() {
                                                    let (ctrl, shift) = ui.input(|input| {
                                                        (
                                                            input.modifiers.ctrl
                                                                || input.modifiers.command,
                                                            input.modifiers.shift,
                                                        )
                                                    });
                                                    if shift {
                                                        collection_reorder_select_range(
                                                            state, index,
                                                        );
                                                    } else if ctrl {
                                                        collection_reorder_toggle_selection(
                                                            state, index,
                                                        );
                                                    } else {
                                                        collection_reorder_select_single(
                                                            state, index,
                                                        );
                                                    }
                                                }
                                                if !busy && response.drag_started() {
                                                    if !state.selected_ids.contains(&entry_id) {
                                                        collection_reorder_select_single(
                                                            state, index,
                                                        );
                                                    } else {
                                                        state.selected = Some(index);
                                                    }
                                                    state.dragging = Some(index);
                                                    state.drag_auto_scroll_enabled = true;
                                                    state.drag_insert_index = Some(index);
                                                }
                                                if !busy
                                                    && state.dragging.is_some()
                                                    && let Some((insert_index, indicator_x)) =
                                                        collection_reorder_drop_target_for_pos(
                                                            rect,
                                                            index,
                                                            state.entries.len(),
                                                            pointer_pos,
                                                        )
                                                {
                                                    state.drag_insert_index = Some(insert_index);
                                                    draw_collection_reorder_insert_indicator(
                                                        ui,
                                                        indicator_x,
                                                        rect,
                                                    );
                                                    if pointer_released {
                                                        move_request = Some(insert_index);
                                                    }
                                                }
                                            }
                                            ui.end_row();
                                        }
                                    });
                            });
                        state.scroll_offset_y = scroll_output.state.offset.y;
                        if !busy
                            && !pointer_released
                            && state.dragging.is_some()
                            && state.drag_auto_scroll_enabled
                            && let Some(pos) = pointer_pos
                        {
                            let delta = collection_reorder_auto_scroll_delta(
                                pos.y,
                                scroll_output.inner_rect.top(),
                                scroll_output.inner_rect.bottom(),
                            );
                            if delta.abs() > f32::EPSILON {
                                let max_offset = (scroll_output.content_size.y
                                    - scroll_output.inner_rect.height())
                                .max(0.0);
                                state.scroll_offset_y =
                                    (state.scroll_offset_y + delta).clamp(0.0, max_offset);
                                ctx.request_repaint_after(std::time::Duration::from_millis(16));
                            }
                        }
                    },
                );
                if let Some(insert_index) = move_request {
                    if move_selected_collection_reorder_group(state, insert_index) {
                        state.dirty = true;
                    }
                    state.dragging = None;
                    state.drag_auto_scroll_enabled = false;
                    state.drag_insert_index = None;
                } else if pointer_released {
                    state.dragging = None;
                    state.drag_auto_scroll_enabled = false;
                    state.drag_insert_index = None;
                }
            });

        if !window_open
            && let Some(state) = self.collection_ui.reorder.as_ref()
            && !state.phase.is_busy()
        {
            if state.dirty {
                save = true;
            } else {
                close = true;
            }
        }
        if discard || close {
            self.collection_ui.reorder = None;
        } else if refresh {
            self.start_collection_reorder_refresh();
        } else if save {
            self.start_collection_reorder_save();
        }
    }

    pub(crate) fn request_collection_grid_remove(
        &mut self,
        request: crate::app::collection_grid::CollectionGridRemoveTarget,
    ) {
        if !self.collection_ui.can_edit() || !self.collection_ui.operation.is_idle() {
            self.show_feedback_toast("コレクションを現在編集できません。".to_string());
            return;
        }
        self.collection_ui.operation = CollectionDialogOperation::ConfirmRemove {
            collection_id: request.stamp.collection_id,
            expected_revision: request.expected_revision,
            entry_ids: request.entry_ids,
            origin: CollectionOperationOrigin::Grid(request.stamp),
        };
    }
}

enum CollectionUiAction {
    Open(CollectionId),
    SetTarget(CollectionId),
    SetPinned {
        id: CollectionId,
        pinned: bool,
    },
    Create(String),
    Rename {
        id: CollectionId,
        revision: u64,
        name: String,
    },
    OpenDelete {
        id: CollectionId,
        revision: u64,
        name: String,
    },
}

impl CollectionUiAction {
    fn requires_ready(&self) -> bool {
        match self {
            Self::Open(_) => false,
            Self::SetTarget(_)
            | Self::SetPinned { .. }
            | Self::Create(_)
            | Self::Rename { .. }
            | Self::OpenDelete { .. } => true,
        }
    }
}

fn draw_collection_operation_status(
    ui: &mut egui::Ui,
    operation: &CollectionDialogOperation,
) -> bool {
    let mut cancel = false;
    match operation {
        CollectionDialogOperation::ReadingImport { source_path, .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("{} を読み込んでいます…", source_path.display()));
                cancel |= ui.button("取り消す").clicked();
            });
        }
        CollectionDialogOperation::Classifying { progress, .. } => {
            let done = progress.0.load(Ordering::Acquire);
            let total = progress.1.load(Ordering::Acquire);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("参照を確認しています… {done}/{total}"));
                cancel |= ui.button("取り消す").clicked();
            });
        }
        CollectionDialogOperation::ToolbarAddSnapshot(request) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!(
                    "「{}」の最新内容を確認しています…",
                    request.collection_name
                ));
            });
        }
        CollectionDialogOperation::GridSnapshot(_) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("コレクションの最新内容を確認しています…");
            });
        }
        CollectionDialogOperation::Submitting(_)
        | CollectionDialogOperation::ExportSnapshot { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("保存内容を確認しています…");
            });
        }
        CollectionDialogOperation::ExportWriting {
            destination,
            progress,
            ..
        } => {
            let done = progress.0.load(Ordering::Acquire);
            let total = progress.1.load(Ordering::Acquire);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!(
                    "{} へ書き出しています… {done}/{total}",
                    destination.display()
                ));
                cancel |= ui.button("取り消す").clicked();
            });
        }
        _ => {}
    }
    cancel
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::collection_store::{
        CollectionDefinition, CollectionEntry, CollectionRegistration, CollectionResolvedKind,
        CollectionSourcePath,
    };
    use crate::settings::SortOrder;

    fn wait_for(app: &mut App, mut predicate: impl FnMut(&App) -> bool) {
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate(app) {
            assert!(Instant::now() < deadline, "collection UI did not settle");
            app.poll_collection_ui(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        app.poll_collection_ui(&ctx);
    }

    fn start_ready_app(
        temp: &tempfile::TempDir,
    ) -> (crate::app::AppTestEnvForTest, CollectionStoreClient) {
        let runtime = CollectionStoreRuntime::start_at(temp.path().join("collection.db")).unwrap();
        let client = runtime.client();
        let mut app = crate::app::setup_app_for_test();
        app.install_collection_runtime(runtime);
        wait_for(&mut app, |app| {
            matches!(app.collection_ui.phase, CollectionRuntimePhase::Ready)
        });
        (app, client)
    }

    fn create_collection(app: &mut App, name: &str) -> CollectionSnapshot {
        let operation = app.submit_collection_name(NameAction::Create, name.into());
        app.collection_ui.operation = operation;
        wait_for(app, |app| {
            app.collection_ui.operation.is_idle() && app.collection_ui.snapshot.is_some()
        });
        app.collection_ui.snapshot.clone().unwrap()
    }

    fn press_delete(app: &mut App) {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Delete,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        });
        let _owner = app.keyboard_owner_for_pass(&ctx);
        app.handle_delete_key(&ctx);
        let _ = ctx.end_pass();
    }

    fn wait_for_collection_grid(app: &mut App, collection_id: CollectionId) {
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while app
            .top_level_grid_view
            .collection_session()
            .is_none_or(|session| {
                session.identity.collection_id != collection_id
                    || session.installed_items_generation != Some(app.items_generation)
                    || session.prepared().is_none()
            })
        {
            assert!(Instant::now() < deadline, "collection Grid did not settle");
            app.poll_collection_ui(&ctx);
            app.poll_collection_grid(&ctx);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn wait_for_collection_grid_revision(
        app: &mut App,
        collection_id: CollectionId,
        revision: u64,
    ) {
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while app
            .top_level_grid_view
            .collection_session()
            .and_then(crate::app::top_level_grid_view::CollectionGridSession::prepared)
            .is_none_or(|prepared| {
                prepared.collection_id != collection_id
                    || prepared.collection_revision < revision
                    || app
                        .top_level_grid_view
                        .collection_session()
                        .is_none_or(|session| {
                            session.installed_items_generation != Some(app.items_generation)
                        })
            })
        {
            assert!(
                Instant::now() < deadline,
                "collection Grid revision did not settle"
            );
            app.poll_collection_ui(&ctx);
            app.poll_collection_grid(&ctx);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn reorder_test_state(names: &[&str]) -> CollectionReorderState {
        let collection_id = CollectionId::new();
        let entries = names
            .iter()
            .enumerate()
            .map(|(index, name)| CollectionReorderEntry {
                entry: CollectionEntry {
                    id: CollectionEntryId::new(),
                    collection_id,
                    source_path: PathBuf::from(format!(r"C:\reorder\{name}.png")),
                    source_key: CollectionSourcePath::from_trusted(PathBuf::from(format!(
                        r"C:\reorder\{name}.png"
                    )))
                    .unwrap()
                    .key()
                    .clone(),
                    resolved_kind: CollectionResolvedKind::Image,
                    manual_position: index as u64,
                },
                texture: None,
            })
            .collect::<Vec<_>>();
        let first_id = entries.first().map(|entry| entry.entry.id);
        CollectionReorderState {
            collection_id,
            collection_name: "Reorder fixture".into(),
            base_revision: 7,
            entries,
            selected: first_id.map(|_| 0),
            selected_ids: first_id.into_iter().collect(),
            selection_anchor: first_id.map(|_| 0),
            dragging: None,
            drag_auto_scroll_enabled: false,
            drag_insert_index: None,
            scroll_offset_y: 0.0,
            thumb_tile_px: COLLECTION_REORDER_DEFAULT_TILE_PX,
            dirty: false,
            phase: CollectionReorderPhase::Ready,
        }
    }

    #[test]
    fn collection_reorder_group_drag_preserves_internal_order_and_stable_selection() {
        let mut state = reorder_test_state(&["A", "B", "C", "D", "E"]);
        let ids = state
            .entries
            .iter()
            .map(|entry| entry.entry.id)
            .collect::<Vec<_>>();
        state.selected = Some(3);
        state.selection_anchor = Some(1);
        state.selected_ids = HashSet::from([ids[1], ids[3]]);

        assert!(move_selected_collection_reorder_group(
            &mut state,
            ids.len()
        ));
        assert_eq!(
            state
                .entries
                .iter()
                .map(|entry| entry.entry.id)
                .collect::<Vec<_>>(),
            vec![ids[0], ids[2], ids[4], ids[1], ids[3]],
            "non-contiguous selected entries move as one group without changing their order"
        );
        assert_eq!(state.selected, Some(4));
        assert_eq!(state.selected_ids, HashSet::from([ids[1], ids[3]]));

        assert!(move_selected_collection_reorder_by(&mut state, -1));
        assert_eq!(
            state
                .entries
                .iter()
                .map(|entry| entry.entry.id)
                .collect::<Vec<_>>(),
            vec![ids[0], ids[2], ids[1], ids[3], ids[4]],
        );
    }

    #[test]
    fn collection_reorder_drop_marker_tracks_the_pointer_side_and_rejects_blank_space() {
        let rect = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 140.0));

        assert_eq!(
            collection_reorder_drop_target_for_pos(rect, 4, 10, Some(egui::pos2(20.0, 80.0))),
            Some((4, rect.left() - 4.0))
        );
        assert_eq!(
            collection_reorder_drop_target_for_pos(rect, 4, 10, Some(egui::pos2(95.0, 80.0))),
            Some((5, rect.right() + 4.0))
        );
        assert_eq!(
            collection_reorder_drop_target_for_pos(rect, 4, 10, Some(egui::pos2(9.0, 80.0))),
            None
        );
        assert_eq!(
            collection_reorder_drop_target_for_pos(rect, 4, 10, None),
            None
        );
    }

    #[test]
    fn collection_reorder_auto_scroll_activates_only_near_viewport_edges() {
        assert_eq!(
            collection_reorder_auto_scroll_delta(200.0, 100.0, 500.0),
            0.0
        );
        assert!(collection_reorder_auto_scroll_delta(112.0, 100.0, 500.0) < 0.0);
        assert!(collection_reorder_auto_scroll_delta(488.0, 100.0, 500.0) > 0.0);
        assert_eq!(
            collection_reorder_auto_scroll_delta(100.0, 100.0, 500.0),
            -COLLECTION_REORDER_AUTO_SCROLL_MAX_STEP_PX
        );
        assert_eq!(
            collection_reorder_auto_scroll_delta(500.0, 100.0, 500.0),
            COLLECTION_REORDER_AUTO_SCROLL_MAX_STEP_PX
        );
    }

    #[test]
    fn delete_key_on_collection_root_removes_checked_references_without_deleting_sources() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.png");
        let second = temp.path().join("second.png");
        std::fs::write(&first, b"first-bytes").unwrap();
        std::fs::write(&second, b"second-bytes").unwrap();
        let (mut app, client) = start_ready_app(&temp);
        let created = create_collection(&mut app, "Delete key");
        let added = client
            .add_batch(
                created.collection_id(),
                created.revision(),
                vec![
                    CollectionRegistration::from_trusted_path(
                        &first,
                        CollectionResolvedKind::Image,
                    )
                    .unwrap(),
                    CollectionRegistration::from_trusted_path(
                        &second,
                        CollectionResolvedKind::Image,
                    )
                    .unwrap(),
                ],
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .snapshot;
        app.open_collection_grid(added.collection_id(), None);
        wait_for_collection_grid(&mut app, added.collection_id());
        let prepared = app
            .top_level_grid_view
            .collection_session()
            .and_then(crate::app::top_level_grid_view::CollectionGridSession::prepared)
            .unwrap()
            .clone();
        let first_index = prepared
            .entries
            .iter()
            .position(|entry| entry.source_path == first)
            .unwrap();
        let second_index = prepared
            .entries
            .iter()
            .position(|entry| entry.source_path == second)
            .unwrap();
        app.selected = Some(first_index);
        app.checked.insert(second_index);
        let manager_selection = app.collection_ui.selected_id;
        assert!(!app.collection_ui.show_manager);

        press_delete(&mut app);
        assert!(
            !app.show_delete_confirm,
            "root Delete must not start file deletion"
        );
        assert_eq!(
            app.modal_dialog_block_reason(),
            Some("collection_operation_modal"),
            "the Grid removal confirmation must block input behind its visible modal"
        );
        assert!(!app.collection_ui.show_manager);
        assert_eq!(app.collection_ui.selected_id, manager_selection);
        let ui_app = std::rc::Rc::new(std::cell::RefCell::new(app));
        let render_app = std::rc::Rc::clone(&ui_app);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1040.0, 760.0))
            .build(move |ctx| render_app.borrow_mut().show_collection_manager(ctx));
        harness.step();
        harness.get_by_label("コレクションから外す");
        harness.get_by_label("キャンセル").click();
        harness.step();
        assert!(ui_app.borrow().collection_ui.operation.is_idle());
        assert!(!ui_app.borrow().collection_ui.show_manager);
        assert_eq!(ui_app.borrow().collection_ui.selected_id, manager_selection);
        drop(harness);
        let mut app = match std::rc::Rc::try_unwrap(ui_app) {
            Ok(app) => app.into_inner(),
            Err(_) => panic!("remove confirmation harness retained the App fixture"),
        };

        press_delete(&mut app);
        let (collection_id, expected_revision, entry_ids, origin) = match std::mem::replace(
            &mut app.collection_ui.operation,
            CollectionDialogOperation::Idle,
        ) {
            CollectionDialogOperation::ConfirmRemove {
                collection_id,
                expected_revision,
                entry_ids,
                origin,
            } => (collection_id, expected_revision, entry_ids, origin),
            _ => panic!("Delete must open reference-removal confirmation"),
        };
        assert_eq!(entry_ids, vec![prepared.entries[second_index].entry_id]);
        app.collection_ui.operation =
            app.submit_collection_remove(collection_id, expected_revision, entry_ids, origin);
        assert_eq!(
            app.modal_dialog_block_reason(),
            None,
            "the actor wait is modeless after the confirmation is submitted"
        );
        wait_for(&mut app, |app| app.collection_ui.operation.is_idle());
        assert!(!app.collection_ui.show_manager);
        assert_eq!(app.collection_ui.selected_id, manager_selection);

        let latest = client
            .load_collection(added.collection_id())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(latest.entries.len(), 1);
        assert_eq!(latest.entries[0].source_path, first);
        assert_eq!(std::fs::read(&first).unwrap(), b"first-bytes");
        assert_eq!(std::fs::read(&second).unwrap(), b"second-bytes");
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn delete_key_on_stale_collection_root_fails_closed_without_file_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.png");
        std::fs::write(&source, b"source-bytes").unwrap();
        let (mut app, client) = start_ready_app(&temp);
        let created = create_collection(&mut app, "Stale root");
        let added = client
            .add_batch(
                created.collection_id(),
                created.revision(),
                vec![
                    CollectionRegistration::from_trusted_path(
                        &source,
                        CollectionResolvedKind::Image,
                    )
                    .unwrap(),
                ],
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .snapshot;
        app.open_collection_grid(added.collection_id(), None);
        wait_for_collection_grid(&mut app, added.collection_id());
        app.selected = Some(0);
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .installed_items_generation = None;

        press_delete(&mut app);
        assert!(!app.show_delete_confirm);
        assert!(app.collection_ui.operation.is_idle());
        assert_eq!(std::fs::read(&source).unwrap(), b"source-bytes");
        assert!(app.fs_feedback_toast.is_some());
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn ready_catalog_prunes_deleted_collection_history_and_snapshot_rollback_cannot_restore_it() {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, client) = start_ready_app(&temp);
        app.active_quick_folder_slot = None;
        let deleted = create_collection(&mut app, "Deleted history target");
        let retained = create_collection(&mut app, "Retained history target");
        let deleted_target = crate::app::FolderNavHistoryTarget::Collection(
            crate::app::top_level_grid_view::CollectionGridRestore {
                identity: crate::app::top_level_grid_view::CollectionGridIdentity {
                    collection_id: deleted.collection_id(),
                },
                revision_at_open: deleted.revision(),
                viewport_anchor: None,
            },
        );
        let retained_target = crate::app::FolderNavHistoryTarget::Collection(
            crate::app::top_level_grid_view::CollectionGridRestore {
                identity: crate::app::top_level_grid_view::CollectionGridIdentity {
                    collection_id: retained.collection_id(),
                },
                revision_at_open: retained.revision(),
                viewport_anchor: None,
            },
        );
        app.folder_nav_back_stack = vec![deleted_target.clone(), retained_target.clone()];
        app.folder_nav_forward_stack = vec![deleted_target.clone()];
        app.quick_folder_workspaces[0].history.back_stack = vec![deleted_target.clone()];
        app.quick_folder_workspaces[1].history.forward_stack = vec![deleted_target.clone()];
        let rollback = app.folder_nav_history_snapshot();

        client
            .delete_collection(deleted.collection_id(), deleted.revision())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        wait_for(&mut app, |app| {
            !app.collection_catalog_contains(deleted.collection_id())
        });
        assert_eq!(app.folder_nav_back_stack, vec![retained_target]);
        assert!(app.folder_nav_forward_stack.is_empty());
        assert!(app.quick_folder_workspaces[0].history.back_stack.is_empty());
        assert!(
            app.quick_folder_workspaces[1]
                .history
                .forward_stack
                .is_empty()
        );

        app.restore_folder_nav_history(rollback);
        assert!(
            app.folder_nav_back_stack
                .iter()
                .all(|target| target.collection_id() != Some(deleted.collection_id()))
        );
        assert!(
            app.folder_nav_forward_stack
                .iter()
                .all(|target| target.collection_id() != Some(deleted.collection_id()))
        );
        assert!(app.quick_folder_workspaces.iter().all(|workspace| {
            workspace
                .history
                .back_stack
                .iter()
                .chain(&workspace.history.forward_stack)
                .all(|target| target.collection_id() != Some(deleted.collection_id()))
        }));

        let held_surface = app.top_level_grid_view.surface().clone();
        let held_folder = app.current_folder.clone();
        assert_eq!(
            app.dispatch_synthetic_folder_history_target(&deleted_target),
            crate::app::SyntheticFolderHistoryDispatch::Unavailable,
            "a deleted collection id is rejected as typed history, never treated as a path",
        );
        assert_eq!(app.top_level_grid_view.surface(), &held_surface);
        assert_eq!(app.current_folder, held_folder);

        app.collection_ui.phase = CollectionRuntimePhase::Starting;
        app.folder_nav_back_stack.push(deleted_target);
        app.prune_collection_folder_history_from_ready_catalog();
        assert!(
            app.folder_nav_back_stack
                .iter()
                .any(|target| target.collection_id() == Some(deleted.collection_id()))
        );
        app.shutdown_collection_runtime_for_exit();
    }

    fn snapshot_app(preview_import: bool) -> crate::app::AppTestEnvForTest {
        let mut app = crate::app::setup_app_for_test();
        let collection_id = CollectionId::new();
        let definition = CollectionDefinition {
            id: collection_id,
            name: "長い名前の旅行写真と資料コレクション".into(),
            order_mode: CollectionOrderMode::Manual,
            standard_sort: SortOrder::FileName,
            shuffle_seed: 0,
            revision: 7,
        };
        let paths = [
            (
                r"D:\Photo\2026\summer\IMG_0001.png",
                CollectionResolvedKind::Image,
            ),
            (
                r"E:\Archive\missing-chapter.cbz",
                CollectionResolvedKind::Unresolved,
            ),
            (
                r"D:\Movies\reference-video.mp4",
                CollectionResolvedKind::Video,
            ),
        ];
        let entries = paths
            .into_iter()
            .enumerate()
            .map(|(position, (path, kind))| {
                let source =
                    CollectionSourcePath::from_trusted(std::path::Path::new(path)).unwrap();
                CollectionEntry {
                    id: CollectionEntryId::new(),
                    collection_id,
                    source_path: source.path().to_path_buf(),
                    source_key: source.key().clone(),
                    resolved_kind: kind,
                    manual_position: position as u64,
                }
            })
            .collect::<Vec<_>>();
        let snapshot = CollectionSnapshot {
            catalog_revision: 3,
            definition: definition.clone(),
            entries: Arc::from(entries),
        };
        app.collection_ui.phase = CollectionRuntimePhase::Ready;
        app.collection_ui.catalog = Some(CollectionCatalogSnapshot {
            catalog_revision: 3,
            definitions: Arc::from(vec![
                definition,
                CollectionDefinition {
                    id: CollectionId::new(),
                    name: "仕事の参考資料".into(),
                    order_mode: CollectionOrderMode::Standard,
                    standard_sort: SortOrder::DateDesc,
                    shuffle_seed: 0,
                    revision: 2,
                },
            ]),
        });
        app.collection_ui.selected_id = Some(collection_id);
        app.collection_ui.selected_entry = Some(snapshot.entries[1].id);
        app.collection_ui
            .checked_entries
            .insert(snapshot.entries[1].id);
        app.collection_ui.snapshot = Some(snapshot);
        app.settings.toolbar_collection_target_id = Some(collection_id.as_uuid());
        app.settings.pinned_collections = vec![collection_id.as_uuid()];
        app.collection_ui.show_manager = true;
        app.collection_ui.message = Some((false, "2 件を追加しました。".into()));
        if preview_import {
            app.collection_ui.show_manager = false;
            app.open_collection_grid(collection_id, None);
            app.top_level_grid_view
                .collection_session_mut()
                .unwrap()
                .installed_items_generation = Some(app.items_generation);
            let preview = parse_collection_text(
                "D:\\Photo\\new-image.png\n\\\\server\\share\\network.png\nNUL\\bad.png\n",
                std::path::Path::new(r"D:\Import\collection.txt"),
            );
            app.collection_ui.operation = CollectionDialogOperation::PreviewImport {
                collection_id,
                expected_revision: 7,
                source_path: PathBuf::from(r"D:\Import\collection.txt"),
                preview,
                origin: CollectionAddOrigin::Grid(
                    crate::app::top_level_grid_view::CollectionGridRequestStamp {
                        context_id: app.collection_grid_context_id(),
                        surface_generation: app.top_level_grid_view.generation(),
                        collection_id,
                    },
                ),
            };
        }
        app
    }

    fn snapshot_collection_manager(
        name: &str,
        theme: crate::os_theme::ResolvedTheme,
        preview: bool,
    ) {
        use egui_kittest::Harness;

        let app = std::rc::Rc::new(std::cell::RefCell::new(snapshot_app(preview)));
        let ui_app = std::rc::Rc::clone(&app);
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1040.0, 760.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, theme);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.heading("画像一覧");
                    ui.label("コレクション管理は独立したウィンドウで表示されます。");
                });
                ui_app.borrow_mut().show_collection_manager(ctx);
            });
        harness.step();
        if preview {
            let mut app = app.borrow_mut();
            let context_id = app.collection_grid_context_id();
            let surface_generation = app.top_level_grid_view.generation();
            if let CollectionDialogOperation::PreviewImport { origin, .. } =
                &mut app.collection_ui.operation
            {
                *origin = CollectionAddOrigin::Grid(
                    crate::app::top_level_grid_view::CollectionGridRequestStamp {
                        context_id,
                        surface_generation,
                        collection_id: origin.grid_stamp().unwrap().collection_id,
                    },
                );
            }
        }
        harness.run();
        harness.snapshot(name);
    }

    fn snapshot_collection_remove_confirm(name: &str) {
        use egui_kittest::Harness;

        let mut app = snapshot_app(false);
        let collection_id = app.collection_ui.selected_id.unwrap();
        let entry_id = app.collection_ui.snapshot.as_ref().unwrap().entries[0].id;
        app.open_collection_grid(collection_id, None);
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .installed_items_generation = Some(app.items_generation);
        app.collection_ui.show_manager = false;
        app.collection_ui.operation = CollectionDialogOperation::ConfirmRemove {
            collection_id,
            expected_revision: 7,
            entry_ids: vec![entry_id],
            origin: CollectionOperationOrigin::Grid(
                crate::app::top_level_grid_view::CollectionGridRequestStamp {
                    context_id: app.collection_grid_context_id(),
                    surface_generation: app.top_level_grid_view.generation(),
                    collection_id,
                },
            ),
        };
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1040.0, 760.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Light);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.heading("画像一覧");
                    ui.label("コレクションの項目を選択しています。");
                });
                app.show_collection_manager(ctx);
            });
        harness.run();
        harness.get_by_label("コレクションから外す");
        harness.snapshot(name);
    }

    fn snapshot_collection_reorder(name: &str) {
        use egui_kittest::Harness;

        let mut app = snapshot_app(false);
        app.collection_ui.show_manager = false;
        let snapshot = app.collection_ui.snapshot.clone().unwrap();
        app.open_collection_reorder(snapshot.clone(), Some(snapshot.entries[0].id));
        if let Some(state) = app.collection_ui.reorder.as_mut() {
            state.selected_ids = HashSet::from([snapshot.entries[0].id, snapshot.entries[2].id]);
            state.selected = Some(2);
            state.selection_anchor = Some(0);
        }
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1040.0, 760.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.heading("画像一覧");
                    ui.label("コレクションの参照順を編集中です。");
                });
                app.draw_collection_reorder(ctx);
            });
        harness.run();
        harness.get_by_label("閉じる");
        harness.snapshot(name);
    }

    #[test]
    fn app_runtime_catalog_conflict_refresh_and_final_shutdown_use_one_owner() {
        let temp = tempfile::tempdir().unwrap();
        {
            let mut inert = crate::app::setup_app_for_test();
            assert!(matches!(
                inert.collection_ui.phase,
                CollectionRuntimePhase::Inert
            ));
            inert.shutdown_collection_runtime_for_exit();
        }

        let (mut app, client) = start_ready_app(&temp);
        let old_snapshot = create_collection(&mut app, "Alpha");
        let collection_id = old_snapshot.collection_id();
        assert_eq!(old_snapshot.revision(), 1);

        let current_folder = PathBuf::from(r"C:\collection-must-not-open-as-folder");
        app.current_folder = Some(current_folder.clone());
        app.select_collection_management_target(collection_id);
        assert_eq!(app.current_folder.as_ref(), Some(&current_folder));
        assert!(app.collection_manager_open());

        let external = client
            .rename_collection(collection_id, old_snapshot.revision(), "External".into())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(external.revision(), 2);
        let operation = app.submit_collection_name(
            NameAction::Rename {
                collection_id,
                expected_revision: old_snapshot.revision(),
            },
            "Stale".into(),
        );
        app.collection_ui.operation = operation;
        wait_for(&mut app, |app| {
            app.collection_ui.operation.is_idle()
                && app.collection_ui.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.revision() == 2 && snapshot.definition.name == "External"
                })
        });
        assert!(
            app.collection_ui
                .message
                .as_ref()
                .is_some_and(|(is_error, text)| *is_error && text.contains("更新"))
        );

        app.shutdown_collection_runtime_for_exit();
        assert!(matches!(
            app.collection_ui.phase,
            CollectionRuntimePhase::Closed
        ));
        assert!(matches!(
            client.list_catalog(),
            Err(CollectionStoreError::Unavailable)
        ));
    }

    #[test]
    fn context_remove_conflict_reloads_the_origin_collection_not_manager_history() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("origin.png");
        std::fs::write(&source, b"source").unwrap();
        let (mut app, client) = start_ready_app(&temp);
        let manager_a = create_collection(&mut app, "Manager A");
        let surface_b = create_collection(&mut app, "Surface B");
        let added_b = client
            .add_batch(
                surface_b.collection_id(),
                surface_b.revision(),
                vec![
                    CollectionRegistration::from_trusted_path(
                        &source,
                        CollectionResolvedKind::Image,
                    )
                    .unwrap(),
                ],
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .snapshot;

        app.collection_ui
            .select_collection(Some(manager_a.collection_id()));
        wait_for(&mut app, |app| {
            app.collection_ui
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.collection_id() == manager_a.collection_id())
        });
        app.open_collection_grid(surface_b.collection_id(), None);
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while app
            .top_level_grid_view
            .collection_session()
            .is_none_or(|session| session.prepared().is_none())
        {
            assert!(Instant::now() < deadline, "collection Grid did not settle");
            app.poll_collection_ui(&ctx);
            app.poll_collection_grid(&ctx);
            std::thread::sleep(Duration::from_millis(2));
        }
        let target = app
            .collection_grid_remove_target(Some(0), false)
            .expect("context remove target");
        let external = client
            .rename_collection(
                surface_b.collection_id(),
                added_b.revision(),
                "Surface B changed".into(),
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();

        app.request_collection_grid_remove(target);
        assert_eq!(
            app.collection_ui.selected_id,
            Some(manager_a.collection_id()),
            "Grid removal must not change manager selection"
        );
        let (collection_id, expected_revision, entry_ids, origin) = match std::mem::replace(
            &mut app.collection_ui.operation,
            CollectionDialogOperation::Idle,
        ) {
            CollectionDialogOperation::ConfirmRemove {
                collection_id,
                expected_revision,
                entry_ids,
                origin,
            } => (collection_id, expected_revision, entry_ids, origin),
            _ => panic!("context remove confirmation"),
        };
        app.collection_ui.operation =
            app.submit_collection_remove(collection_id, expected_revision, entry_ids, origin);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.collection_ui.operation.is_idle()
            || app
                .top_level_grid_view
                .collection_session()
                .and_then(crate::app::top_level_grid_view::CollectionGridSession::prepared)
                .is_none_or(|prepared| prepared.collection_revision != external.revision())
        {
            assert!(
                Instant::now() < deadline,
                "origin Collection Grid did not converge after conflict"
            );
            app.poll_collection_ui(&ctx);
            app.poll_collection_grid(&ctx);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            app.collection_ui.selected_id,
            Some(manager_a.collection_id())
        );
        assert!(
            app.collection_ui
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| { snapshot.collection_id() == manager_a.collection_id() })
        );
        assert!(!app.collection_ui.show_manager);
        assert!(
            app.collection_ui
                .message
                .as_ref()
                .is_some_and(|(error, text)| { *error && text.contains("更新") })
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn actual_handlers_commit_crud_reorder_relink_remove_and_import_only_after_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, _client) = start_ready_app(&temp);
        let created = create_collection(&mut app, "Handler lifecycle");
        let collection_id = created.collection_id();

        let first = temp.path().join("first.png");
        let second = temp.path().join("second.mp4");
        let replacement = temp.path().join("replacement.png");
        std::fs::write(&first, b"first-source").unwrap();
        std::fs::write(&second, b"second-source").unwrap();
        std::fs::write(&replacement, b"replacement-source").unwrap();
        app.start_collection_classification(
            vec![first.clone(), second.clone()],
            ClassificationTarget::Add {
                collection_id,
                expected_revision: created.revision(),
                import_errors: Vec::new(),
                origin: CollectionAddOrigin::Manager,
            },
        );
        wait_for(&mut app, |app| {
            app.collection_ui.operation.is_idle()
                && app
                    .collection_ui
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.entries.len() == 2)
        });
        assert_eq!(std::fs::read(&first).unwrap(), b"first-source");
        assert_eq!(std::fs::read(&second).unwrap(), b"second-source");

        let added = app.collection_ui.snapshot.clone().unwrap();
        let first_id = added
            .entries
            .iter()
            .find(|entry| entry.source_path == first)
            .unwrap()
            .id;
        let second_id = added
            .entries
            .iter()
            .find(|entry| entry.source_path == second)
            .unwrap()
            .id;
        app.open_collection_reorder(added.clone(), Some(first_id));
        {
            let state = app.collection_ui.reorder.as_mut().unwrap();
            assert!(move_selected_collection_reorder_group(
                state,
                state.entries.len()
            ));
            state.dirty = true;
        }
        app.start_collection_reorder_save();
        wait_for(&mut app, |app| app.collection_ui.reorder.is_none());
        let reordered = app.collection_ui.snapshot.clone().unwrap();
        assert_eq!(reordered.entries[0].id, second_id);
        assert_eq!(reordered.entries[1].id, first_id);

        let receiver = app
            .collection_ui
            .client
            .as_ref()
            .unwrap()
            .set_order(
                reordered.collection_id(),
                reordered.revision(),
                CollectionOrderMode::Standard,
                SortOrder::SizeDesc,
            )
            .unwrap();
        app.collection_ui.operation =
            CollectionDialogOperation::Submitting(ActorTask::SnapshotMutation {
                collection_id,
                origin: CollectionOperationOrigin::Manager,
                receiver,
            });
        wait_for(&mut app, |app| app.collection_ui.operation.is_idle());
        let standard = app.collection_ui.snapshot.clone().unwrap();
        assert_eq!(
            standard.definition.order_mode,
            CollectionOrderMode::Standard
        );
        assert_eq!(standard.definition.standard_sort, SortOrder::SizeDesc);

        app.start_collection_classification(
            vec![replacement.clone()],
            ClassificationTarget::Relink {
                collection_id,
                expected_revision: standard.revision(),
                entry_id: first_id,
                origin: CollectionOperationOrigin::Manager,
            },
        );
        wait_for(&mut app, |app| {
            app.collection_ui.operation.is_idle()
                && app.collection_ui.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot
                        .entries
                        .iter()
                        .any(|entry| entry.id == first_id && entry.source_path == replacement)
                })
        });
        assert_eq!(std::fs::read(&first).unwrap(), b"first-source");
        assert_eq!(std::fs::read(&replacement).unwrap(), b"replacement-source");

        let relinked = app.collection_ui.snapshot.clone().unwrap();
        app.collection_ui.operation = app.submit_collection_remove(
            collection_id,
            relinked.revision(),
            vec![second_id],
            CollectionOperationOrigin::Manager,
        );
        wait_for(&mut app, |app| {
            app.collection_ui.operation.is_idle()
                && app
                    .collection_ui
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.entries.len() == 1)
        });
        assert_eq!(std::fs::read(&second).unwrap(), b"second-source");

        let import_target = temp.path().join("appears-after-preview.png");
        let import_text = temp.path().join("import.txt");
        std::fs::write(&import_text, format!("{}\n", import_target.display())).unwrap();
        let before_import = app.collection_ui.snapshot.clone().unwrap();
        app.start_collection_import_read(collection_id, before_import.revision(), import_text);
        wait_for(&mut app, |app| {
            matches!(
                app.collection_ui.operation,
                CollectionDialogOperation::PreviewImport { .. }
            )
        });
        assert!(!import_target.exists());
        std::fs::write(&import_target, b"created-after-preview").unwrap();
        let preview = match std::mem::replace(
            &mut app.collection_ui.operation,
            CollectionDialogOperation::Idle,
        ) {
            CollectionDialogOperation::PreviewImport {
                expected_revision,
                preview,
                origin,
                ..
            } => (expected_revision, preview, origin),
            _ => unreachable!(),
        };
        app.collection_ui.operation = app.collection_import_classification_operation(
            collection_id,
            preview.0,
            &preview.1,
            preview.2,
        );
        wait_for(&mut app, |app| {
            app.collection_ui.operation.is_idle()
                && app.collection_ui.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.entries.iter().any(|entry| {
                        entry.source_path == import_target
                            && entry.resolved_kind == CollectionResolvedKind::Image
                    })
                })
        });

        let before_delete = app.collection_ui.snapshot.clone().unwrap();
        app.collection_ui.operation =
            app.submit_collection_delete(collection_id, before_delete.revision());
        wait_for(&mut app, |app| {
            app.collection_ui.operation.is_idle()
                && app
                    .collection_ui
                    .catalog
                    .as_ref()
                    .is_some_and(|catalog| catalog.definitions.is_empty())
        });
        assert_eq!(
            std::fs::read(&import_target).unwrap(),
            b"created-after-preview"
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn grid_order_click_does_not_overwrite_a_newer_actor_revision_before_reply_adoption() {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, client) = start_ready_app(&temp);
        let created = create_collection(&mut app, "Exact order");
        app.open_collection_grid(created.collection_id(), None);
        wait_for_collection_grid(&mut app, created.collection_id());
        let clicked = app.collection_grid_root_order().unwrap().unwrap();

        // The mounted root still displays N, while another owner commits N+1. The UI's
        // load_collection reply is deliberately polled only after that commit.
        let newer = client
            .set_order(
                created.collection_id(),
                clicked.content.expected_revision,
                CollectionOrderMode::Standard,
                SortOrder::DateDesc,
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        app.start_collection_grid_content_action(
            clicked.content,
            CollectionGridSnapshotAction::SetOrder {
                mode: CollectionOrderMode::Shuffle,
                sort: SortOrder::FileName,
            },
        );
        wait_for(&mut app, |app| app.collection_ui.operation.is_idle());
        let latest = client
            .load_collection(created.collection_id())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(
            latest, newer,
            "a stale order click must not mutate unseen N+1"
        );
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .is_some_and(|(message, _, _)| message.contains("切り替わった"))
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn grid_reorder_uses_exact_latest_root_and_preserves_separate_owners() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ["first.png", "second.png", "third.png", "fourth.png"]
            .map(|name| temp.path().join(name));
        for (index, path) in paths.iter().enumerate() {
            std::fs::write(path, format!("source-{index}")).unwrap();
        }

        let (mut app, client) = start_ready_app(&temp);
        let manager = create_collection(&mut app, "Manager owner");
        let surface = create_collection(&mut app, "Grid content owner");
        let added = client
            .add_batch(
                surface.collection_id(),
                surface.revision(),
                paths[..3]
                    .iter()
                    .map(|path| {
                        CollectionRegistration::from_trusted_path(
                            path,
                            CollectionResolvedKind::Image,
                        )
                        .unwrap()
                    })
                    .collect(),
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .snapshot;
        let selected_id = added
            .entries
            .iter()
            .find(|entry| entry.source_path == paths[1])
            .unwrap()
            .id;

        app.collection_ui
            .select_collection(Some(manager.collection_id()));
        wait_for(&mut app, |app| {
            app.collection_ui
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.collection_id() == manager.collection_id())
        });
        app.select_collection_toolbar_target(manager.collection_id());
        app.open_collection_grid(surface.collection_id(), None);
        wait_for_collection_grid(&mut app, surface.collection_id());
        let selected_index = app
            .top_level_grid_view
            .collection_session()
            .and_then(crate::app::top_level_grid_view::CollectionGridSession::prepared)
            .unwrap()
            .entries
            .iter()
            .position(|entry| entry.entry_id == selected_id)
            .unwrap();
        app.selected = Some(selected_index);
        let stale_target = app.collection_grid_content_target().unwrap();

        // Reorder opens only from an exact installed Root. An actor update after the
        // click-time target is captured must fail closed instead of editing a stale list.
        let external = client
            .add_batch(
                surface.collection_id(),
                added.revision(),
                vec![
                    CollectionRegistration::from_trusted_path(
                        &paths[3],
                        CollectionResolvedKind::Image,
                    )
                    .unwrap(),
                ],
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .snapshot;
        app.start_collection_grid_content_action(
            stale_target,
            CollectionGridSnapshotAction::OpenReorder,
        );
        wait_for(&mut app, |app| app.collection_ui.operation.is_idle());
        assert!(app.collection_ui.reorder.is_none());
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .is_some_and(|(text, _, _)| { text.contains("切り替わった") })
        );

        wait_for_collection_grid_revision(&mut app, surface.collection_id(), external.revision());
        let selected_index = app
            .top_level_grid_view
            .collection_session()
            .and_then(crate::app::top_level_grid_view::CollectionGridSession::prepared)
            .unwrap()
            .entries
            .iter()
            .position(|entry| entry.entry_id == selected_id)
            .unwrap();
        app.selected = Some(selected_index);
        let reorder_target = app.collection_grid_content_target().unwrap();
        app.start_collection_grid_content_action(
            reorder_target,
            CollectionGridSnapshotAction::OpenReorder,
        );
        wait_for(&mut app, |app| {
            app.collection_ui.operation.is_idle() && app.collection_ui.reorder.is_some()
        });
        {
            let state = app.collection_ui.reorder.as_mut().unwrap();
            assert_eq!(
                state
                    .entries
                    .iter()
                    .map(|entry| entry.entry.id)
                    .collect::<Vec<_>>(),
                external
                    .entries
                    .iter()
                    .map(|entry| entry.id)
                    .collect::<Vec<_>>(),
                "the actor's complete manual sequence is the editor source"
            );
            let end = state.entries.len();
            assert!(move_selected_collection_reorder_group(state, end));
            state.dirty = true;
        }
        app.start_collection_reorder_save();
        wait_for(&mut app, |app| app.collection_ui.reorder.is_none());
        let moved = client
            .load_collection(surface.collection_id())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        let moved_ids = moved
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>();
        assert_eq!(moved_ids.last(), Some(&selected_id));
        assert_eq!(app.collection_ui.selected_id, Some(manager.collection_id()));
        assert_eq!(
            app.settings.toolbar_collection_target_id,
            Some(manager.collection_id().as_uuid())
        );
        assert!(!app.collection_ui.show_manager);
        assert!(app.collection_grid_request_stamp_is_current(reorder_target.stamp));

        wait_for_collection_grid_revision(&mut app, surface.collection_id(), moved.revision());
        let standard = client
            .set_order(
                surface.collection_id(),
                moved.revision(),
                CollectionOrderMode::Standard,
                SortOrder::FileName,
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        wait_for_collection_grid_revision(&mut app, surface.collection_id(), standard.revision());
        let standard_target = app.collection_grid_content_target().unwrap();
        app.start_collection_grid_content_action(
            standard_target,
            CollectionGridSnapshotAction::OpenReorder,
        );
        wait_for(&mut app, |app| app.collection_ui.operation.is_idle());
        assert!(app.collection_ui.reorder.is_none());
        let after_disabled_reorder = client
            .load_collection(surface.collection_id())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(after_disabled_reorder.revision(), standard.revision());
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .is_some_and(|(text, _, _)| { text.contains("手動順") })
        );

        app.open_collection_grid(manager.collection_id(), None);
        app.start_collection_grid_content_action(
            standard_target,
            CollectionGridSnapshotAction::OpenReorder,
        );
        assert!(app.collection_ui.operation.is_idle());
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .is_some_and(|(text, _, _)| { text.contains("更新してから") })
        );
        let after_stale_target = client
            .load_collection(surface.collection_id())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(after_stale_target, after_disabled_reorder);

        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn reorder_conflict_keeps_edits_until_explicit_refresh_or_discard() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ["first.png", "second.png", "external.png"].map(|name| temp.path().join(name));
        for path in &paths {
            std::fs::write(path, b"source").unwrap();
        }
        let (mut app, client) = start_ready_app(&temp);
        let created = create_collection(&mut app, "Conflict recovery");
        let added = client
            .add_batch(
                created.collection_id(),
                created.revision(),
                paths[..2]
                    .iter()
                    .map(|path| {
                        CollectionRegistration::from_trusted_path(
                            path,
                            CollectionResolvedKind::Image,
                        )
                        .unwrap()
                    })
                    .collect(),
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .snapshot;
        app.open_collection_reorder(added.clone(), Some(added.entries[0].id));
        {
            let state = app.collection_ui.reorder.as_mut().unwrap();
            let end = state.entries.len();
            assert!(move_selected_collection_reorder_group(state, end));
            state.dirty = true;
        }
        let edited_order = app
            .collection_ui
            .reorder
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|entry| entry.entry.id)
            .collect::<Vec<_>>();

        let external = client
            .add_batch(
                added.collection_id(),
                added.revision(),
                vec![
                    CollectionRegistration::from_trusted_path(
                        &paths[2],
                        CollectionResolvedKind::Image,
                    )
                    .unwrap(),
                ],
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .snapshot;
        app.start_collection_reorder_save();
        wait_for(&mut app, |app| {
            app.collection_ui.reorder.as_ref().is_some_and(|state| {
                matches!(
                    state.phase,
                    CollectionReorderPhase::Error { conflict: true, .. }
                )
            })
        });
        let state = app.collection_ui.reorder.as_ref().unwrap();
        assert!(state.dirty);
        assert_eq!(
            state
                .entries
                .iter()
                .map(|entry| entry.entry.id)
                .collect::<Vec<_>>(),
            edited_order,
            "a conflict keeps the user's unsaved order"
        );

        app.start_collection_reorder_refresh();
        wait_for(&mut app, |app| {
            app.collection_ui.reorder.as_ref().is_some_and(|state| {
                matches!(state.phase, CollectionReorderPhase::Ready)
                    && state.base_revision == external.revision()
            })
        });
        let state = app.collection_ui.reorder.as_ref().unwrap();
        assert!(!state.dirty);
        assert_eq!(
            state
                .entries
                .iter()
                .map(|entry| entry.entry.id)
                .collect::<Vec<_>>(),
            external
                .entries
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            "explicit refresh discards edits and adopts the latest actor order"
        );

        {
            let state = app.collection_ui.reorder.as_mut().unwrap();
            state.dirty = true;
            state.phase = CollectionReorderPhase::Error {
                message: "競合".into(),
                conflict: true,
            };
        }
        let ui_app = std::rc::Rc::new(std::cell::RefCell::new(app));
        let render_app = std::rc::Rc::clone(&ui_app);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1040.0, 760.0))
            .build(move |ctx| render_app.borrow_mut().draw_collection_reorder(ctx));
        harness.step();
        harness.get_by_label("変更を破棄して閉じる").click();
        harness.step();
        assert!(ui_app.borrow().collection_ui.reorder.is_none());
        drop(harness);
        let mut app = match std::rc::Rc::try_unwrap(ui_app) {
            Ok(app) => app.into_inner(),
            Err(_) => panic!("discard harness retained the App fixture"),
        };
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn dirty_reorder_close_autosaves_before_the_window_closes() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ["first.png", "second.png"].map(|name| temp.path().join(name));
        for path in &paths {
            std::fs::write(path, b"source").unwrap();
        }
        let (mut app, client) = start_ready_app(&temp);
        let created = create_collection(&mut app, "Close autosave");
        let added = client
            .add_batch(
                created.collection_id(),
                created.revision(),
                paths
                    .iter()
                    .map(|path| {
                        CollectionRegistration::from_trusted_path(
                            path,
                            CollectionResolvedKind::Image,
                        )
                        .unwrap()
                    })
                    .collect(),
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .snapshot;
        app.open_collection_reorder(added.clone(), Some(added.entries[0].id));
        {
            let state = app.collection_ui.reorder.as_mut().unwrap();
            let end = state.entries.len();
            assert!(move_selected_collection_reorder_group(state, end));
            state.dirty = true;
        }

        let ui_app = std::rc::Rc::new(std::cell::RefCell::new(app));
        let render_app = std::rc::Rc::clone(&ui_app);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1040.0, 760.0))
            .build(move |ctx| render_app.borrow_mut().draw_collection_reorder(ctx));
        harness.step();
        harness.get_by_label("閉じる").click();
        harness.step();
        assert!(
            ui_app
                .borrow()
                .collection_ui
                .reorder
                .as_ref()
                .is_some_and(|state| matches!(state.phase, CollectionReorderPhase::Saving { .. }))
        );
        drop(harness);
        let mut app = match std::rc::Rc::try_unwrap(ui_app) {
            Ok(app) => app.into_inner(),
            Err(_) => panic!("autosave harness retained the App fixture"),
        };
        wait_for(&mut app, |app| app.collection_ui.reorder.is_none());
        let saved = client
            .load_collection(added.collection_id())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(saved.entries[1].id, added.entries[0].id);
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn toolbar_add_uses_checked_selection_latest_revision_and_preserves_manager_owner() {
        let temp = tempfile::tempdir().unwrap();
        let ignored = temp.path().join("ignored.png");
        let checked = temp.path().join("checked.jpg");
        let existing = temp.path().join("existing.mp4");
        let folder = temp.path().join("selected-folder");
        std::fs::write(&ignored, b"ignored-source").unwrap();
        std::fs::write(&checked, b"checked-source").unwrap();
        std::fs::write(&existing, b"existing-source").unwrap();
        std::fs::create_dir(&folder).unwrap();

        let (mut app, client) = start_ready_app(&temp);
        let manager = create_collection(&mut app, "Manager owner");
        let target = create_collection(&mut app, "Toolbar target");
        app.collection_ui
            .select_collection(Some(manager.collection_id()));
        wait_for(&mut app, |app| {
            app.collection_ui
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.collection_id() == manager.collection_id())
        });
        app.select_collection_toolbar_target(target.collection_id());
        assert_eq!(
            app.settings.toolbar_collection_target_id,
            Some(target.collection_id().as_uuid())
        );
        assert_eq!(app.collection_ui.selected_id, Some(manager.collection_id()));

        let external = client
            .add_batch(
                target.collection_id(),
                target.revision(),
                vec![
                    CollectionRegistration::from_trusted_path(
                        &existing,
                        CollectionResolvedKind::Video,
                    )
                    .unwrap(),
                ],
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .snapshot;
        assert!(external.revision() > target.revision());

        app.items = vec![
            crate::grid_item::GridItem::Image(ignored.clone()),
            crate::grid_item::GridItem::Image(checked.clone()),
        ];
        app.selected = Some(0);
        app.checked.insert(1);
        app.address = "source-folder".into();
        app.scroll_offset_y = 417.5;
        app.pending_grid_scroll = Some(crate::app::GridScrollIntent::Bottom);
        let source_surface = app.top_level_grid_view.surface().clone();
        let source_address = app.address.clone();
        let source_generation = app.items_generation;
        let source_selected = app.selected;
        let source_checked = app.checked.clone();
        let source_scroll = app.scroll_offset_y;
        let source_pending_scroll = app.pending_grid_scroll;
        app.add_grid_selection_to_collection(target.collection_id());
        wait_for(&mut app, |app| app.collection_ui.operation.is_idle());

        let added = client
            .load_collection(target.collection_id())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert!(added.revision() > external.revision());
        assert!(
            added
                .entries
                .iter()
                .any(|entry| entry.source_path == existing)
        );
        assert!(
            added
                .entries
                .iter()
                .any(|entry| entry.source_path == checked)
        );
        assert!(
            added
                .entries
                .iter()
                .all(|entry| entry.source_path != ignored)
        );
        assert_eq!(
            app.collection_ui.selected_id,
            Some(manager.collection_id()),
            "toolbar add must not switch or cancel the manager selection owner"
        );
        assert_eq!(
            app.settings.toolbar_collection_target_id,
            Some(target.collection_id().as_uuid()),
            "manager selection and actor completion must not replace the toolbar target"
        );
        assert_eq!(app.top_level_grid_view.surface(), &source_surface);
        assert_eq!(app.address, source_address);
        assert_eq!(app.items_generation, source_generation);
        assert_eq!(app.selected, source_selected);
        assert_eq!(app.checked, source_checked);
        assert_eq!(app.scroll_offset_y, source_scroll);
        assert_eq!(app.pending_grid_scroll, source_pending_scroll);
        assert_eq!(std::fs::read(&ignored).unwrap(), b"ignored-source");
        assert_eq!(std::fs::read(&checked).unwrap(), b"checked-source");
        assert_eq!(std::fs::read(&existing).unwrap(), b"existing-source");

        app.add_grid_selection_to_collection(target.collection_id());
        wait_for(&mut app, |app| app.collection_ui.operation.is_idle());
        let duplicate = client
            .load_collection(target.collection_id())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(duplicate.entries.len(), added.entries.len());
        assert!(
            app.collection_ui
                .message
                .as_ref()
                .is_some_and(|(_, text)| { text.contains("重複 1 件") })
        );

        app.checked.clear();
        app.items = vec![crate::grid_item::GridItem::Folder(folder.clone())];
        app.selected = Some(0);
        app.add_grid_selection_to_collection(target.collection_id());
        wait_for(&mut app, |app| app.collection_ui.operation.is_idle());
        let with_folder = client
            .load_collection(target.collection_id())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert!(with_folder.entries.iter().any(|entry| {
            entry.source_path == folder && entry.resolved_kind == CollectionResolvedKind::Folder
        }));
        assert!(folder.is_dir());
        assert_eq!(app.collection_ui.selected_id, Some(manager.collection_id()));
        wait_for(&mut app, |app| {
            app.collection_ui.catalog_request.is_none()
                && app.collection_ui.catalog.as_ref().is_some_and(|catalog| {
                    catalog.definitions.iter().any(|definition| {
                        definition.id == target.collection_id()
                            && definition.revision == with_folder.revision()
                    })
                })
        });

        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn toolbar_target_reconciles_only_from_an_authoritative_ready_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, client) = start_ready_app(&temp);
        let first = create_collection(&mut app, "First");
        let second = create_collection(&mut app, "Second");
        app.collection_ui
            .select_collection(Some(first.collection_id()));
        wait_for(&mut app, |app| {
            app.collection_ui
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.collection_id() == first.collection_id())
        });

        app.select_collection_toolbar_target(second.collection_id());
        let stale_pin = CollectionId::new().as_uuid();
        app.settings.pinned_collections = vec![
            second.collection_id().as_uuid(),
            second.collection_id().as_uuid(),
            stale_pin,
            first.collection_id().as_uuid(),
        ];
        let second_target = Some(second.collection_id().as_uuid());
        assert_eq!(app.settings.toolbar_collection_target_id, second_target);
        assert_eq!(app.collection_ui.selected_id, Some(first.collection_id()));

        app.select_collection_toolbar_target(CollectionId::new());
        assert_eq!(app.settings.toolbar_collection_target_id, second_target);

        let retained_catalog = app.collection_ui.catalog.take();
        app.collection_ui.phase = CollectionRuntimePhase::Starting;
        app.reconcile_collection_toolbar_target();
        assert_eq!(app.settings.toolbar_collection_target_id, second_target);
        assert_eq!(app.settings.pinned_collections.len(), 4);
        app.collection_ui.phase = CollectionRuntimePhase::Ready;
        app.reconcile_collection_toolbar_target();
        assert_eq!(
            app.settings.toolbar_collection_target_id, second_target,
            "a transient missing snapshot must not erase the durable target"
        );
        app.collection_ui.catalog = retained_catalog;
        app.reconcile_collection_toolbar_target();
        assert_eq!(
            app.settings.pinned_collections,
            vec![
                second.collection_id().as_uuid(),
                first.collection_id().as_uuid()
            ]
        );

        let without_second = client
            .delete_collection(second.collection_id(), second.revision())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        app.collection_ui.catalog = Some(without_second);
        app.reconcile_collection_toolbar_target();
        assert_eq!(
            app.settings.toolbar_collection_target_id,
            Some(first.collection_id().as_uuid())
        );
        assert_eq!(app.collection_ui.selected_id, Some(first.collection_id()));
        assert_eq!(
            app.settings.pinned_collections,
            vec![first.collection_id().as_uuid()]
        );

        let empty = client
            .delete_collection(first.collection_id(), first.revision())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        app.collection_ui.catalog = Some(empty);
        app.reconcile_collection_toolbar_target();
        assert_eq!(app.settings.toolbar_collection_target_id, None);
        assert!(app.settings.pinned_collections.is_empty());
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn toolbar_add_rejects_mixed_virtual_selection_atomically_and_keeps_busy_owner() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real.png");
        let pdf = temp.path().join("book.pdf");
        std::fs::write(&real, b"real-source").unwrap();
        std::fs::write(&pdf, b"pdf-source").unwrap();
        let (mut app, client) = start_ready_app(&temp);
        let target = create_collection(&mut app, "Virtual refusal");

        app.items = vec![
            crate::grid_item::GridItem::Image(real.clone()),
            crate::grid_item::GridItem::PdfPage {
                pdf_path: pdf.clone(),
                page_num: 0,
                content_type: None,
            },
        ];
        app.checked.extend([0, 1]);
        app.selected = Some(0);
        app.add_grid_selection_to_collection(target.collection_id());
        assert!(app.collection_ui.operation.is_idle());
        assert!(app.fs_feedback_toast.as_ref().is_some_and(|(text, _, _)| {
            text.contains("PDF 内のページ") && text.contains("追加できません")
        }));
        let unchanged = client
            .load_collection(target.collection_id())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert!(unchanged.entries.is_empty());
        assert_eq!(std::fs::read(&real).unwrap(), b"real-source");
        assert_eq!(std::fs::read(&pdf).unwrap(), b"pdf-source");

        app.items
            .push(crate::grid_item::GridItem::CollectionPlaceholder {
                path: temp.path().join("missing.jpg"),
                last_known_kind: CollectionResolvedKind::Image,
                reason: crate::grid_item::CollectionPlaceholderReason::Missing,
            });
        app.checked.clear();
        app.checked.extend([0, 2]);
        app.add_grid_selection_to_collection(target.collection_id());
        assert!(app.collection_ui.operation.is_idle());
        assert!(app.fs_feedback_toast.as_ref().is_some_and(|(text, _, _)| {
            text.contains("見つからないコレクション項目") && text.contains("追加できません")
        }));
        let still_unchanged = client
            .load_collection(target.collection_id())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert!(still_unchanged.entries.is_empty());

        let preview = parse_collection_text(
            &format!("{}\n", real.display()),
            &temp.path().join("import.txt"),
        );
        app.collection_ui.operation = CollectionDialogOperation::PreviewImport {
            collection_id: target.collection_id(),
            expected_revision: target.revision(),
            source_path: temp.path().join("import.txt"),
            preview,
            origin: CollectionAddOrigin::Manager,
        };
        app.items = vec![crate::grid_item::GridItem::Image(real)];
        app.checked.clear();
        app.selected = Some(0);
        app.add_grid_selection_to_collection(target.collection_id());
        assert!(matches!(
            app.collection_ui.operation,
            CollectionDialogOperation::PreviewImport { .. }
        ));
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .is_some_and(|(text, _, _)| { text.contains("別のコレクション処理") })
        );

        let (export_sender, export_receiver) = crossbeam_channel::bounded(1);
        app.collection_ui.operation = CollectionDialogOperation::ExportSnapshot {
            collection_id: target.collection_id(),
            destination: temp.path().join("export.txt"),
            origin: CollectionOperationOrigin::Manager,
            receiver: export_receiver,
        };
        app.add_grid_selection_to_collection(target.collection_id());
        assert!(matches!(
            app.collection_ui.operation,
            CollectionDialogOperation::ExportSnapshot { .. }
        ));
        drop(export_sender);

        let relink_cancel_seen = Arc::new(AtomicBool::new(false));
        let worker_cancel_seen = Arc::clone(&relink_cancel_seen);
        let task = WorkerTask::spawn("toolbar-add-busy-relink-test", move |cancel| {
            while !cancel.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            worker_cancel_seen.store(true, Ordering::Release);
            ClassificationResult {
                prepared: Vec::new(),
                cancelled: true,
            }
        })
        .unwrap();
        app.collection_ui.operation = CollectionDialogOperation::Classifying {
            target: ClassificationTarget::Relink {
                collection_id: target.collection_id(),
                expected_revision: target.revision(),
                entry_id: CollectionEntryId::new(),
                origin: CollectionOperationOrigin::Manager,
            },
            progress: Arc::new((AtomicUsize::new(0), AtomicUsize::new(1))),
            task,
        };
        app.add_grid_selection_to_collection(target.collection_id());
        assert!(matches!(
            app.collection_ui.operation,
            CollectionDialogOperation::Classifying {
                target: ClassificationTarget::Relink { .. },
                ..
            }
        ));
        app.collection_ui.operation.cancel_worker();
        app.collection_ui.operation = CollectionDialogOperation::Idle;
        let deadline = Instant::now() + Duration::from_secs(2);
        while !relink_cancel_seen.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "relink cancel was not delivered");
            std::thread::yield_now();
        }

        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn toolbar_add_operation_detaches_from_manager_window_lifetime() {
        let collection_id = CollectionId::new();
        let (_snapshot_sender, snapshot_receiver) = crossbeam_channel::bounded(1);
        let snapshot = CollectionDialogOperation::ToolbarAddSnapshot(ToolbarAddSnapshotRequest {
            collection_id,
            collection_name: "Detached toolbar".into(),
            paths: vec![PathBuf::from(r"C:\media\page.png")],
            receiver: snapshot_receiver,
        });
        assert_eq!(
            snapshot.manager_close_behavior(),
            CollectionManagerCloseBehavior::Detach
        );

        let classify_cancel_seen = Arc::new(AtomicBool::new(false));
        let worker_cancel_seen = Arc::clone(&classify_cancel_seen);
        let task = WorkerTask::spawn("toolbar-add-detach-classify-test", move |cancel| {
            while !cancel.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            worker_cancel_seen.store(true, Ordering::Release);
            ClassificationResult {
                prepared: Vec::new(),
                cancelled: true,
            }
        })
        .unwrap();
        let classifying = CollectionDialogOperation::Classifying {
            target: ClassificationTarget::Add {
                collection_id,
                expected_revision: 1,
                import_errors: Vec::new(),
                origin: CollectionAddOrigin::Toolbar {
                    collection_name: "Detached toolbar".into(),
                },
            },
            progress: Arc::new((AtomicUsize::new(0), AtomicUsize::new(1))),
            task,
        };
        assert_eq!(
            classifying.manager_close_behavior(),
            CollectionManagerCloseBehavior::Detach
        );
        classifying.cancel_worker();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !classify_cancel_seen.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "classification cancel was not delivered"
            );
            std::thread::yield_now();
        }

        let (_add_sender, add_receiver) = crossbeam_channel::bounded(1);
        let submitting = CollectionDialogOperation::Submitting(ActorTask::Add {
            collection_id,
            errors: Vec::new(),
            origin: CollectionAddOrigin::Toolbar {
                collection_name: "Detached toolbar".into(),
            },
            receiver: add_receiver,
        });
        assert_eq!(
            submitting.manager_close_behavior(),
            CollectionManagerCloseBehavior::Detach
        );
        assert_eq!(
            CollectionDialogOperation::Name {
                action: NameAction::Create,
                draft: String::new(),
            }
            .manager_close_behavior(),
            CollectionManagerCloseBehavior::Cancel
        );
    }

    #[test]
    fn toolbar_add_conflict_reports_terminal_toast_and_refreshes_actor_state() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("conflict.png");
        std::fs::write(&source, b"conflict-source").unwrap();
        let (mut app, client) = start_ready_app(&temp);
        let stale = create_collection(&mut app, "Conflict target");
        let external = client
            .rename_collection(
                stale.collection_id(),
                stale.revision(),
                "Conflict target renamed".into(),
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();

        app.start_collection_classification(
            vec![source.clone()],
            ClassificationTarget::Add {
                collection_id: stale.collection_id(),
                expected_revision: stale.revision(),
                import_errors: Vec::new(),
                origin: CollectionAddOrigin::Toolbar {
                    collection_name: "Conflict target".into(),
                },
            },
        );
        wait_for(&mut app, |app| {
            app.collection_ui.operation.is_idle()
                && app.collection_ui.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.revision() == external.revision()
                        && snapshot.definition.name == "Conflict target renamed"
                })
        });

        assert!(
            app.collection_ui
                .message
                .as_ref()
                .is_some_and(|(error, text)| { *error && text.contains("更新") })
        );
        assert!(app.fs_feedback_toast.as_ref().is_some_and(|(text, _, _)| {
            text.contains("Conflict target") && text.contains("追加できませんでした")
        }));
        let latest = client
            .load_collection(stale.collection_id())
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert!(latest.entries.is_empty());
        assert_eq!(std::fs::read(&source).unwrap(), b"conflict-source");
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn failed_runtime_keeps_stale_snapshot_read_only_and_toolbar_reports_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, _client) = start_ready_app(&temp);
        let snapshot = create_collection(&mut app, "Read only");
        app.collection_ui.phase = CollectionRuntimePhase::Failed("actor stopped".into());

        app.apply_collection_ui_action(CollectionUiAction::Create("No".into()));

        assert!(app.collection_ui.operation.is_idle());
        assert_eq!(
            app.collection_ui.snapshot.as_ref().unwrap().revision(),
            snapshot.revision()
        );
        assert!(
            app.collection_ui
                .message
                .as_ref()
                .is_some_and(|(is_error, text)| {
                    *is_error && text.contains("現在編集できません")
                })
        );
        let (_, _, status) = app.collection_toolbar_catalog();
        assert_eq!(status, CollectionToolbarStatus::Unavailable);
        assert_eq!(status.label(), "利用不可");
        let stale_rows = vec![(snapshot.collection_id(), "Read only".to_owned())];
        assert_eq!(
            CollectionToolbarStatus::Unavailable
                .selected_text(Some(snapshot.collection_id()), &stale_rows),
            "利用不可"
        );
        assert_eq!(
            CollectionToolbarStatus::Starting
                .selected_text(Some(snapshot.collection_id()), &stale_rows),
            "準備中…"
        );
        assert_eq!(
            CollectionToolbarStatus::Ready
                .selected_text(Some(snapshot.collection_id()), &stale_rows),
            "Read only"
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn classification_or_preview_error_does_not_partially_update_collection() {
        let temp = tempfile::tempdir().unwrap();
        let good = temp.path().join("good.png");
        let unsupported = temp.path().join("unsupported.txt");
        std::fs::write(&good, b"image").unwrap();
        std::fs::write(&unsupported, b"text").unwrap();
        let (mut app, _client) = start_ready_app(&temp);
        let snapshot = create_collection(&mut app, "Atomic import");

        app.start_collection_classification(
            vec![good.clone(), unsupported],
            ClassificationTarget::Add {
                collection_id: snapshot.collection_id(),
                expected_revision: snapshot.revision(),
                import_errors: Vec::new(),
                origin: CollectionAddOrigin::Manager,
            },
        );
        wait_for(&mut app, |app| app.collection_ui.operation.is_idle());
        assert!(
            app.collection_ui
                .snapshot
                .as_ref()
                .unwrap()
                .entries
                .is_empty()
        );
        assert!(
            app.collection_ui
                .message
                .as_ref()
                .is_some_and(|(is_error, text)| {
                    *is_error && text.contains("追加していません")
                })
        );

        let prepared = prepare_collection_registrations(
            std::slice::from_ref(&good),
            &AtomicBool::new(false),
            |_, _| {},
        );
        let operation = app.submit_collection_classification(
            ClassificationTarget::Add {
                collection_id: snapshot.collection_id(),
                expected_revision: snapshot.revision(),
                import_errors: vec!["2 行: invalid path".into()],
                origin: CollectionAddOrigin::Manager,
            },
            prepared,
        );
        assert!(operation.is_idle());
        assert!(
            app.collection_ui
                .snapshot
                .as_ref()
                .unwrap()
                .entries
                .is_empty()
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn export_handler_loads_latest_actor_snapshot_instead_of_stale_dialog_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.png");
        let second = temp.path().join("second.mp4");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let destination = temp.path().join("export.txt");
        let (mut app, client) = start_ready_app(&temp);
        let stale = create_collection(&mut app, "Latest export");
        let registrations = prepare_collection_registrations(
            &[first.clone(), second.clone()],
            &AtomicBool::new(false),
            |_, _| {},
        )
        .into_iter()
        .map(|item| item.result.unwrap())
        .collect::<Vec<_>>();
        let latest = client
            .add_batch(stale.collection_id(), stale.revision(), registrations)
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .snapshot;
        assert!(latest.revision() > app.collection_ui.snapshot.as_ref().unwrap().revision());

        app.start_collection_export_snapshot_request(stale.collection_id(), destination.clone());
        wait_for(&mut app, |app| {
            app.collection_ui.operation.is_idle() && destination.exists()
        });

        let exported = std::fs::read_to_string(&destination).unwrap();
        assert!(exported.contains(&first.display().to_string()));
        assert!(exported.contains(&second.display().to_string()));
        assert!(app.collection_ui.message.as_ref().is_some_and(|(_, text)| {
            text.contains(&format!("revision {}", latest.revision()))
        }));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn selection_change_cancels_worker_and_late_result_has_no_operation_owner() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let task = WorkerTask::spawn("collection-cancel-test", move |cancel| {
            while !cancel.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            worker_cancelled.store(true, Ordering::Release);
            ClassificationResult {
                prepared: Vec::new(),
                cancelled: true,
            }
        })
        .unwrap();
        let mut state = CollectionUiState::default();
        state.operation = CollectionDialogOperation::Classifying {
            target: ClassificationTarget::Add {
                collection_id: CollectionId::new(),
                expected_revision: 1,
                import_errors: Vec::new(),
                origin: CollectionAddOrigin::Manager,
            },
            progress: Arc::new((AtomicUsize::new(0), AtomicUsize::new(1))),
            task,
        };

        state.select_collection(Some(CollectionId::new()));
        assert!(state.operation.is_idle());
        let deadline = Instant::now() + Duration::from_secs(2);
        while !cancelled.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "cancel was not delivered to worker"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn cancellable_worker_status_has_explicit_cancel_and_drops_late_result_owner() {
        let cancel_seen = Arc::new(AtomicBool::new(false));
        let worker_seen = Arc::clone(&cancel_seen);
        let task = WorkerTask::spawn("collection-ui-cancel-test", move |cancel| {
            while !cancel.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            worker_seen.store(true, Ordering::Release);
            Err("late import result".to_owned())
        })
        .unwrap();
        let app = std::rc::Rc::new(std::cell::RefCell::new(snapshot_app(false)));
        let collection_id = app.borrow().collection_ui.selected_id.unwrap();
        app.borrow_mut().collection_ui.operation = CollectionDialogOperation::ReadingImport {
            collection_id,
            expected_revision: 7,
            source_path: PathBuf::from(r"D:\Import\collection.txt"),
            origin: CollectionAddOrigin::Manager,
            task,
        };
        let ui_app = std::rc::Rc::clone(&app);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1040.0, 760.0))
            .build(move |ctx| ui_app.borrow_mut().show_collection_manager(ctx));
        harness.step();
        harness.get_by_label("取り消す").click();
        harness.step();
        assert!(app.borrow().collection_ui.operation.is_idle());
        assert!(
            app.borrow()
                .collection_ui
                .message
                .as_ref()
                .is_some_and(|(_, text)| { text.contains("取り消しました") })
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while !cancel_seen.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "cancel was not delivered");
            std::thread::yield_now();
        }
    }

    #[test]
    fn manager_explicit_cancel_reports_toolbar_add_target_and_drops_worker_owner() {
        let cancel_seen = Arc::new(AtomicBool::new(false));
        let worker_seen = Arc::clone(&cancel_seen);
        let task = WorkerTask::spawn("toolbar-add-ui-cancel-test", move |cancel| {
            while !cancel.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            worker_seen.store(true, Ordering::Release);
            ClassificationResult {
                prepared: Vec::new(),
                cancelled: true,
            }
        })
        .unwrap();
        let app = std::rc::Rc::new(std::cell::RefCell::new(snapshot_app(false)));
        let collection_id = app.borrow().collection_ui.selected_id.unwrap();
        app.borrow_mut().collection_ui.operation = CollectionDialogOperation::Classifying {
            target: ClassificationTarget::Add {
                collection_id,
                expected_revision: 7,
                import_errors: Vec::new(),
                origin: CollectionAddOrigin::Toolbar {
                    collection_name: "Toolbar target".into(),
                },
            },
            progress: Arc::new((AtomicUsize::new(0), AtomicUsize::new(1))),
            task,
        };
        let ui_app = std::rc::Rc::clone(&app);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1040.0, 760.0))
            .build(move |ctx| ui_app.borrow_mut().show_collection_manager(ctx));
        harness.step();
        harness.get_by_label("取り消す").click();
        harness.step();
        assert!(app.borrow().collection_ui.operation.is_idle());
        assert!(
            app.borrow()
                .collection_ui
                .message
                .as_ref()
                .is_some_and(|(_, text)| text.contains("追加を取り消しました"))
        );
        assert!(
            app.borrow()
                .fs_feedback_toast
                .as_ref()
                .is_some_and(|(text, _, _)| {
                    text.contains("Toolbar target") && text.contains("取り消しました")
                })
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while !cancel_seen.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "cancel was not delivered");
            std::thread::yield_now();
        }
    }

    #[test]
    fn collection_manager_populated_dark_snapshot() {
        snapshot_collection_manager(
            "collection_manager_populated_dark",
            crate::os_theme::ResolvedTheme::Dark,
            false,
        );
    }

    #[test]
    fn collection_manager_import_preview_light_snapshot() {
        snapshot_collection_manager(
            "collection_manager_import_preview_light",
            crate::os_theme::ResolvedTheme::Light,
            true,
        );
    }

    #[test]
    fn collection_grid_remove_confirm_light_snapshot() {
        snapshot_collection_remove_confirm("collection_grid_remove_confirm_light");
    }

    #[test]
    fn collection_reorder_dark_snapshot() {
        snapshot_collection_reorder("collection_reorder_dark");
    }
}
