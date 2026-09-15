//! 名前付きコレクションのPC管理UI。
//!
//! Phase 2 はcatalog/edit/import/exportだけを接続する。collection gridをまだ通常folderへ
//! 擬装しないため、toolbarの名前選択はこの管理windowの対象選択にだけ使う。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crossbeam_channel::Receiver;
use eframe::egui;

use crate::app::App;
use crate::collection_store::{
    CollectionBatchAddOutcome, CollectionCatalogSnapshot, CollectionEntryId, CollectionId,
    CollectionImportLineStatus, CollectionImportPreview, CollectionOrderMode,
    CollectionPrepareError, CollectionPreparedRegistration, CollectionRevisionWatch,
    CollectionRuntimeEvent, CollectionRuntimeEventStream, CollectionSnapshot,
    CollectionStoreClient, CollectionStoreError, CollectionStoreRuntime, parse_collection_text,
    prepare_collection_export, prepare_collection_registrations, serialize_collection_paths,
    write_collection_export_atomic,
};

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
        active_id: Option<CollectionId>,
        rows: &[(CollectionId, String)],
    ) -> String {
        if self != Self::Ready {
            return self.label().to_owned();
        }
        rows.iter()
            .find(|(id, _)| Some(*id) == active_id)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| self.label().to_owned())
    }
}

struct CatalogRequest {
    minimum_revision: u64,
    receiver: Receiver<Result<CollectionCatalogSnapshot, CollectionStoreError>>,
}

struct SnapshotRequest {
    collection_id: CollectionId,
    minimum_revision: u64,
    receiver: Receiver<Result<CollectionSnapshot, CollectionStoreError>>,
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
enum ClassificationTarget {
    Add {
        collection_id: CollectionId,
        expected_revision: u64,
        import_errors: Vec<String>,
    },
    Relink {
        collection_id: CollectionId,
        expected_revision: u64,
        entry_id: CollectionEntryId,
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
        receiver: Receiver<Result<CollectionBatchAddOutcome, CollectionStoreError>>,
    },
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
        origin: Option<crate::app::top_level_grid_view::CollectionGridRequestStamp>,
    },
    ReadingImport {
        collection_id: CollectionId,
        expected_revision: u64,
        source_path: PathBuf,
        task: WorkerTask<Result<CollectionImportPreview, String>>,
    },
    PreviewImport {
        collection_id: CollectionId,
        expected_revision: u64,
        source_path: PathBuf,
        preview: CollectionImportPreview,
    },
    Classifying {
        target: ClassificationTarget,
        progress: Arc<(AtomicUsize, AtomicUsize)>,
        task: WorkerTask<ClassificationResult>,
    },
    Submitting(ActorTask),
    ExportSnapshot {
        collection_id: CollectionId,
        destination: PathBuf,
        receiver: Receiver<Result<CollectionSnapshot, CollectionStoreError>>,
    },
    ExportWriting {
        collection_id: CollectionId,
        collection_revision: u64,
        destination: PathBuf,
        progress: Arc<(AtomicUsize, AtomicUsize)>,
        task: WorkerTask<Result<(), CollectionPrepareError>>,
    },
}

impl CollectionDialogOperation {
    fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    fn blocks_window_close(&self) -> bool {
        matches!(self, Self::Submitting(_) | Self::ExportSnapshot { .. })
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
        }
    }
}

impl CollectionUiState {
    fn can_edit(&self) -> bool {
        matches!(self.phase, CollectionRuntimePhase::Ready)
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
        match client.list_catalog() {
            Ok(receiver) => {
                self.catalog_request = Some(CatalogRequest {
                    minimum_revision: self.wanted_catalog_revision,
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
        match client.load_collection(collection_id) {
            Ok(receiver) => {
                self.snapshot_request = Some(SnapshotRequest {
                    collection_id,
                    minimum_revision: self.wanted_collection_revision,
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

impl App {
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
        let rows = self
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
        (
            self.collection_ui.selected_id,
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

        if let Some(request) = self.collection_ui.catalog_request.take() {
            let minimum_revision = request.minimum_revision;
            match request.receiver.try_recv() {
                Ok(Ok(catalog)) => self.collection_ui.install_catalog(catalog),
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

        if let Some(request) = self.collection_ui.snapshot_request.take() {
            let collection_id = request.collection_id;
            let minimum_revision = request.minimum_revision;
            match request.receiver.try_recv() {
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
                mut task,
            } => match task.poll_finished() {
                None => CollectionDialogOperation::ReadingImport {
                    collection_id,
                    expected_revision,
                    source_path,
                    task,
                },
                Some(Ok(Ok(preview))) => CollectionDialogOperation::PreviewImport {
                    collection_id,
                    expected_revision,
                    source_path,
                    preview,
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
                    self.collection_ui.message = Some((true, error));
                    CollectionDialogOperation::Idle
                }
                Some(Ok(result)) if result.cancelled => {
                    self.collection_ui.message = Some((false, "追加を取り消しました。".into()));
                    CollectionDialogOperation::Idle
                }
                Some(Ok(result)) => self.submit_collection_classification(target, result.prepared),
            },
            CollectionDialogOperation::Submitting(task) => self.poll_collection_actor_task(task),
            CollectionDialogOperation::ExportSnapshot {
                collection_id,
                destination,
                receiver,
            } => match receiver.try_recv() {
                Ok(Ok(snapshot)) => self.start_collection_export_worker(snapshot, destination),
                Ok(Err(error)) => {
                    self.collection_ui.message = Some((true, collection_error_message(&error)));
                    CollectionDialogOperation::Idle
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    CollectionDialogOperation::ExportSnapshot {
                        collection_id,
                        destination,
                        receiver,
                    }
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.collection_ui.message =
                        Some((true, "エクスポート用一覧の応答が失われました。".into()));
                    CollectionDialogOperation::Idle
                }
            },
            CollectionDialogOperation::ExportWriting {
                collection_id,
                collection_revision,
                destination,
                progress,
                mut task,
            } => match task.poll_finished() {
                None => CollectionDialogOperation::ExportWriting {
                    collection_id,
                    collection_revision,
                    destination,
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
                    CollectionDialogOperation::Idle
                }
                Some(Ok(Err(CollectionPrepareError::Cancelled))) => {
                    self.collection_ui.message =
                        Some((false, "エクスポートを取り消しました。".into()));
                    CollectionDialogOperation::Idle
                }
                Some(Ok(Err(error))) => {
                    self.collection_ui.message = Some((true, error.to_string()));
                    CollectionDialogOperation::Idle
                }
                Some(Err(error)) => {
                    self.collection_ui.message = Some((true, error));
                    CollectionDialogOperation::Idle
                }
            },
            operation => operation,
        };
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
        let Some(client) = self.collection_ui.client.as_ref() else {
            self.collection_ui.message = Some((true, "コレクションを利用できません。".into()));
            return CollectionDialogOperation::Idle;
        };
        match target {
            ClassificationTarget::Add {
                collection_id,
                expected_revision,
                import_errors,
            } => {
                errors.extend(import_errors);
                if !errors.is_empty() {
                    self.collection_ui.message = Some((
                        true,
                        format!(
                            "確認できない項目があるため、追加していません。\n{}",
                            errors.join("\n")
                        ),
                    ));
                    return CollectionDialogOperation::Idle;
                }
                if registrations.is_empty() {
                    self.collection_ui.message = Some((false, "追加する項目がありません。".into()));
                    return CollectionDialogOperation::Idle;
                }
                match client.add_batch(collection_id, expected_revision, registrations) {
                    Ok(receiver) => CollectionDialogOperation::Submitting(ActorTask::Add {
                        collection_id,
                        errors,
                        receiver,
                    }),
                    Err(error) => {
                        self.collection_ui.message = Some((true, collection_error_message(&error)));
                        CollectionDialogOperation::Idle
                    }
                }
            }
            ClassificationTarget::Relink {
                collection_id,
                expected_revision,
                entry_id,
            } => {
                if registrations.len() != 1 || !errors.is_empty() {
                    self.collection_ui.message = Some((
                        true,
                        errors
                            .into_iter()
                            .next()
                            .unwrap_or_else(|| "再リンク先を確認できませんでした。".into()),
                    ));
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
                            receiver,
                        })
                    }
                    Err(error) => {
                        self.collection_ui.message = Some((true, collection_error_message(&error)));
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
                    CollectionDialogOperation::Idle
                }
                Ok(Err(error)) => self.finish_actor_error(error),
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    CollectionDialogOperation::Submitting(ActorTask::SnapshotMutation {
                        collection_id,
                        receiver,
                    })
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
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
                    // The actor commit is global. The origin stamp is deliberately consumed only
                    // as response provenance; Grid convergence comes from the revision watch.
                    self.apply_collection_grid_remove_success(origin, &entry_ids);
                    CollectionDialogOperation::Idle
                }
                Ok(Err(error)) => self.finish_actor_error(error),
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    CollectionDialogOperation::Submitting(ActorTask::ContextRemove {
                        origin,
                        collection_id,
                        entry_ids,
                        receiver,
                    })
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
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
                    self.collection_ui.message = Some((!errors.is_empty(), message));
                    CollectionDialogOperation::Idle
                }
                Ok(Err(error)) => self.finish_actor_error(error),
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    CollectionDialogOperation::Submitting(ActorTask::Add {
                        collection_id,
                        errors,
                        receiver,
                    })
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.finish_actor_disconnect()
                }
            },
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
            let prepared =
                prepare_collection_export(&snapshot, &display_order, &cancel, |done, total| {
                    worker_progress.0.store(done, Ordering::Release);
                    worker_progress.1.store(total, Ordering::Release);
                })?;
            let contents =
                serialize_collection_paths(prepared.ordered_paths.iter().map(PathBuf::as_path));
            write_collection_export_atomic(&worker_destination, contents.as_bytes(), &cancel)
        });
        match task {
            Ok(task) => CollectionDialogOperation::ExportWriting {
                collection_id,
                collection_revision,
                destination,
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
            return;
        }
        let catalog = self.collection_ui.catalog.clone();
        let snapshot = self.collection_ui.snapshot.clone();
        let selected_id = self.collection_ui.selected_id;
        let phase = self.collection_ui.phase.clone();
        let busy = !self.collection_ui.operation.is_idle();
        let can_edit = matches!(phase, CollectionRuntimePhase::Ready);
        let mut open = true;
        let mut action = None;
        let mut cancel_worker = false;
        egui::Window::new("コレクションの管理")
            .id(egui::Id::new("collection_manager"))
            .open(&mut open)
            .default_size(egui::vec2(820.0, 620.0))
            .min_size(egui::vec2(620.0, 420.0))
            .resizable(true)
            .show(ctx, |ui| {
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
                    ui.label("コレクション:");
                    let selected_name = catalog
                        .as_ref()
                        .and_then(|catalog| {
                            catalog.definitions.iter().find(|item| Some(item.id) == selected_id)
                        })
                        .map(|item| item.name.as_str())
                        .unwrap_or("（なし）");
                    ui.add_enabled_ui(
                        !busy && can_edit,
                        |ui| {
                            egui::ComboBox::from_id_salt("collection_manager_target")
                            .width(260.0)
                            .selected_text(selected_name)
                            .show_ui(ui, |ui| {
                                if let Some(catalog) = &catalog {
                                    for definition in catalog.definitions.iter() {
                                        if ui
                                            .selectable_label(
                                                Some(definition.id) == selected_id,
                                                &definition.name,
                                            )
                                            .clicked()
                                        {
                                            action =
                                                Some(CollectionUiAction::Select(definition.id));
                                            ui.close();
                                        }
                                    }
                                }
                            });
                        },
                    );
                    if ui.add_enabled(!busy && can_edit, egui::Button::new("作成…")).clicked() {
                        action = Some(CollectionUiAction::OpenCreate);
                    }
                    if ui.add_enabled(!busy && can_edit && snapshot.is_some(), egui::Button::new("名前変更…")).clicked() {
                        action = Some(CollectionUiAction::OpenRename);
                    }
                    if ui.add_enabled(!busy && can_edit && snapshot.is_some(), egui::Button::new("削除…")).on_hover_text("定義と参照だけを削除します。元ファイルは削除しません。" ).clicked() {
                        action = Some(CollectionUiAction::OpenDelete);
                    }
                });
                ui.weak("コレクションの作成・編集・インポート・エクスポートを行えます。一覧表示は準備中です。");
                ui.separator();

                if let Some(snapshot) = &snapshot {
                    let mut mode = snapshot.definition.order_mode;
                    let mut sort = snapshot.definition.standard_sort;
                    ui.horizontal(|ui| {
                        ui.label("並び:");
                        ui.add_enabled_ui(!busy && can_edit, |ui| {
                            ui.radio_value(&mut mode, CollectionOrderMode::Manual, "手動順");
                            ui.radio_value(&mut mode, CollectionOrderMode::Standard, "通常ソート");
                            if mode == CollectionOrderMode::Standard {
                                egui::ComboBox::from_id_salt("collection_standard_sort")
                                    .selected_text(sort.label())
                                    .show_ui(ui, |ui| {
                                        for &candidate in crate::settings::SortOrder::all() {
                                            ui.selectable_value(&mut sort, candidate, candidate.label());
                                        }
                                    });
                            }
                        });
                        if (mode, sort)
                            != (snapshot.definition.order_mode, snapshot.definition.standard_sort)
                        {
                            action = Some(CollectionUiAction::SetOrder { mode, sort });
                        }
                        ui.separator();
                        ui.label(format!("{} 件", snapshot.entries.len()));
                    });

                    let selected_entry = self.collection_ui.selected_entry;
                    let checked = &mut self.collection_ui.checked_entries;
                    egui::ScrollArea::vertical()
                        .id_salt("collection_entries")
                        .max_height(360.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if snapshot.entries.is_empty() {
                                ui.weak("参照はまだありません。");
                            }
                            for entry in snapshot.entries.iter() {
                                ui.horizontal(|ui| {
                                    let mut is_checked = checked.contains(&entry.id);
                                    if ui.checkbox(&mut is_checked, "").changed() {
                                        if is_checked {
                                            checked.insert(entry.id);
                                        } else {
                                            checked.remove(&entry.id);
                                        }
                                    }
                                    let missing = entry.resolved_kind == crate::collection_store::CollectionResolvedKind::Unresolved;
                                    let label = if missing {
                                        format!("{}  （見つかりません）", entry.source_path.display())
                                    } else {
                                        entry.source_path.display().to_string()
                                    };
                                    if ui.selectable_label(selected_entry == Some(entry.id), label).clicked() {
                                        action = Some(CollectionUiAction::SelectEntry(entry.id));
                                    }
                                });
                            }
                        });

                    ui.horizontal_wrapped(|ui| {
                        if ui.add_enabled(!busy && can_edit, egui::Button::new("ファイルを追加…")).clicked() {
                            action = Some(CollectionUiAction::PickFiles);
                        }
                        if ui.add_enabled(!busy && can_edit, egui::Button::new("フォルダを追加…")).clicked() {
                            action = Some(CollectionUiAction::PickFolder);
                        }
                        if ui.add_enabled(!busy && can_edit, egui::Button::new("テキストをインポート…")).clicked() {
                            action = Some(CollectionUiAction::PickImport);
                        }
                        if ui.add_enabled(!busy && can_edit, egui::Button::new("エクスポート…")).clicked() {
                            action = Some(CollectionUiAction::PickExport);
                        }
                        let has_checked = !self.collection_ui.checked_entries.is_empty();
                        if ui.add_enabled(!busy && can_edit && has_checked, egui::Button::new("コレクションから外す…")).clicked() {
                            action = Some(CollectionUiAction::OpenRemove);
                        }
                        let selected = self.collection_ui.selected_entry;
                        if ui.add_enabled(!busy && can_edit && selected.is_some(), egui::Button::new("再リンク（ファイル）…")).clicked() {
                            action = Some(CollectionUiAction::PickRelinkFile);
                        }
                        if ui.add_enabled(!busy && can_edit && selected.is_some(), egui::Button::new("再リンク（フォルダ）…")).clicked() {
                            action = Some(CollectionUiAction::PickRelinkFolder);
                        }
                    });
                    if snapshot.definition.order_mode == CollectionOrderMode::Manual {
                        ui.horizontal(|ui| {
                            ui.label("手動順:");
                            let selected = self.collection_ui.selected_entry;
                            if ui.add_enabled(!busy && can_edit && selected.is_some(), egui::Button::new("先頭")).clicked() {
                                action = Some(CollectionUiAction::MoveEntry(MoveEntry::First));
                            }
                            if ui.add_enabled(!busy && can_edit && selected.is_some(), egui::Button::new("上へ")).clicked() {
                                action = Some(CollectionUiAction::MoveEntry(MoveEntry::Up));
                            }
                            if ui.add_enabled(!busy && can_edit && selected.is_some(), egui::Button::new("下へ")).clicked() {
                                action = Some(CollectionUiAction::MoveEntry(MoveEntry::Down));
                            }
                            if ui.add_enabled(!busy && can_edit && selected.is_some(), egui::Button::new("末尾")).clicked() {
                                action = Some(CollectionUiAction::MoveEntry(MoveEntry::Last));
                            }
                        });
                    }
                } else if matches!(phase, CollectionRuntimePhase::Ready) {
                    if catalog.as_ref().is_some_and(|catalog| catalog.definitions.is_empty()) {
                        ui.weak("コレクションはまだありません。「作成…」から追加できます。");
                    } else {
                        ui.spinner();
                        ui.label("内容を読み込んでいます…");
                    }
                }

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
            self.collection_ui.operation.cancel_worker();
            self.collection_ui.operation = CollectionDialogOperation::Idle;
            self.collection_ui.message = Some((false, "処理を取り消しました。".into()));
        }

        if !open {
            if self.collection_ui.operation.blocks_window_close() {
                self.collection_ui.show_manager = true;
            } else {
                self.collection_ui.operation.cancel_worker();
                self.collection_ui.operation = CollectionDialogOperation::Idle;
                self.collection_ui.show_manager = false;
            }
        }
        if let Some(action) = action {
            self.apply_collection_ui_action(action);
        }
        self.show_collection_operation_modal(ctx);
    }

    fn apply_collection_ui_action(&mut self, action: CollectionUiAction) {
        if action.requires_ready() && !self.collection_ui.can_edit() {
            self.collection_ui.message = Some((true, "コレクションを現在編集できません。".into()));
            return;
        }
        let Some(snapshot) = self.collection_ui.snapshot.clone() else {
            match action {
                CollectionUiAction::Select(id) => self.collection_ui.select_collection(Some(id)),
                CollectionUiAction::OpenCreate => {
                    self.collection_ui.operation = CollectionDialogOperation::Name {
                        action: NameAction::Create,
                        draft: String::new(),
                    }
                }
                _ => {}
            }
            return;
        };
        match action {
            CollectionUiAction::Select(id) => self.collection_ui.select_collection(Some(id)),
            CollectionUiAction::SelectEntry(id) => self.collection_ui.selected_entry = Some(id),
            CollectionUiAction::OpenCreate => {
                self.collection_ui.operation = CollectionDialogOperation::Name {
                    action: NameAction::Create,
                    draft: String::new(),
                };
            }
            CollectionUiAction::OpenRename => {
                self.collection_ui.operation = CollectionDialogOperation::Name {
                    action: NameAction::Rename {
                        collection_id: snapshot.collection_id(),
                        expected_revision: snapshot.revision(),
                    },
                    draft: snapshot.definition.name.clone(),
                };
            }
            CollectionUiAction::OpenDelete => {
                self.collection_ui.operation = CollectionDialogOperation::ConfirmDelete {
                    collection_id: snapshot.collection_id(),
                    expected_revision: snapshot.revision(),
                    name: snapshot.definition.name.clone(),
                };
            }
            CollectionUiAction::OpenRemove => {
                self.collection_ui.operation = CollectionDialogOperation::ConfirmRemove {
                    collection_id: snapshot.collection_id(),
                    expected_revision: snapshot.revision(),
                    entry_ids: self.collection_ui.checked_entries.iter().copied().collect(),
                    origin: None,
                };
            }
            CollectionUiAction::SetOrder { mode, sort } => {
                let Some(client) = &self.collection_ui.client else {
                    return;
                };
                match client.set_order(snapshot.collection_id(), snapshot.revision(), mode, sort) {
                    Ok(receiver) => {
                        self.collection_ui.operation =
                            CollectionDialogOperation::Submitting(ActorTask::SnapshotMutation {
                                collection_id: snapshot.collection_id(),
                                receiver,
                            });
                    }
                    Err(error) => {
                        self.collection_ui.message = Some((true, collection_error_message(&error)))
                    }
                }
            }
            CollectionUiAction::MoveEntry(direction) => {
                self.reorder_selected_collection_entry(&snapshot, direction)
            }
            CollectionUiAction::PickFiles => {
                if let Some(paths) = rfd::FileDialog::new().pick_files() {
                    self.start_collection_classification(
                        paths,
                        ClassificationTarget::Add {
                            collection_id: snapshot.collection_id(),
                            expected_revision: snapshot.revision(),
                            import_errors: Vec::new(),
                        },
                    );
                }
            }
            CollectionUiAction::PickFolder => {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.start_collection_classification(
                        vec![path],
                        ClassificationTarget::Add {
                            collection_id: snapshot.collection_id(),
                            expected_revision: snapshot.revision(),
                            import_errors: Vec::new(),
                        },
                    );
                }
            }
            CollectionUiAction::PickImport => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("テキスト", &["txt"])
                    .pick_file()
                {
                    self.start_collection_import_read(
                        snapshot.collection_id(),
                        snapshot.revision(),
                        path,
                    );
                }
            }
            CollectionUiAction::PickExport => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("テキスト", &["txt"])
                    .set_file_name("collection.txt")
                    .save_file()
                {
                    self.start_collection_export_snapshot_request(snapshot.collection_id(), path);
                }
            }
            CollectionUiAction::PickRelinkFile | CollectionUiAction::PickRelinkFolder => {
                let Some(entry_id) = self.collection_ui.selected_entry else {
                    return;
                };
                let path = if matches!(action, CollectionUiAction::PickRelinkFile) {
                    rfd::FileDialog::new().pick_file()
                } else {
                    rfd::FileDialog::new().pick_folder()
                };
                if let Some(path) = path {
                    self.start_collection_classification(
                        vec![path],
                        ClassificationTarget::Relink {
                            collection_id: snapshot.collection_id(),
                            expected_revision: snapshot.revision(),
                            entry_id,
                        },
                    );
                }
            }
        }
    }

    fn reorder_selected_collection_entry(
        &mut self,
        snapshot: &CollectionSnapshot,
        direction: MoveEntry,
    ) {
        let Some(selected) = self.collection_ui.selected_entry else {
            return;
        };
        let mut order = snapshot
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>();
        let Some(from) = order.iter().position(|id| *id == selected) else {
            return;
        };
        let to = match direction {
            MoveEntry::First => 0,
            MoveEntry::Up => from.saturating_sub(1),
            MoveEntry::Down => (from + 1).min(order.len().saturating_sub(1)),
            MoveEntry::Last => order.len().saturating_sub(1),
        };
        if from == to {
            return;
        }
        let entry = order.remove(from);
        order.insert(to, entry);
        let Some(client) = &self.collection_ui.client else {
            return;
        };
        match client.reorder_manual(snapshot.collection_id(), snapshot.revision(), order) {
            Ok(receiver) => {
                self.collection_ui.operation =
                    CollectionDialogOperation::Submitting(ActorTask::SnapshotMutation {
                        collection_id: snapshot.collection_id(),
                        receiver,
                    })
            }
            Err(error) => {
                self.collection_ui.message = Some((true, collection_error_message(&error)))
            }
        }
    }

    fn start_collection_import_read(
        &mut self,
        collection_id: CollectionId,
        expected_revision: u64,
        source_path: PathBuf,
    ) {
        let worker_path = source_path.clone();
        match WorkerTask::spawn("collection-import-read", move |_| {
            std::fs::read_to_string(&worker_path)
                .map(|text| parse_collection_text(&text, &worker_path))
                .map_err(|error| format!("インポートファイルを読めませんでした: {error}"))
        }) {
            Ok(task) => {
                self.collection_ui.operation = CollectionDialogOperation::ReadingImport {
                    collection_id,
                    expected_revision,
                    source_path,
                    task,
                }
            }
            Err(error) => self.collection_ui.message = Some((true, error)),
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
        match WorkerTask::spawn("collection-source-prepare", move |cancel| {
            let prepared = prepare_collection_registrations(&paths, &cancel, |done, total| {
                worker_progress.0.store(done, Ordering::Release);
                worker_progress.1.store(total, Ordering::Release);
            });
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
                self.collection_ui.message = Some((true, error));
                CollectionDialogOperation::Idle
            }
        }
    }

    fn collection_import_classification_operation(
        &mut self,
        collection_id: CollectionId,
        expected_revision: u64,
        preview: &CollectionImportPreview,
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
            } => {
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
                    ),
                    Some(false) => CollectionDialogOperation::Idle,
                    None => CollectionDialogOperation::PreviewImport {
                        collection_id,
                        expected_revision,
                        source_path,
                        preview,
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
        origin: Option<crate::app::top_level_grid_view::CollectionGridRequestStamp>,
    ) -> CollectionDialogOperation {
        let Some(client) = &self.collection_ui.client else {
            return CollectionDialogOperation::Idle;
        };
        match client.remove_entries(collection_id, expected_revision, entry_ids.clone()) {
            Ok(receiver) => CollectionDialogOperation::Submitting(match origin {
                Some(origin) => ActorTask::ContextRemove {
                    origin,
                    collection_id,
                    entry_ids,
                    receiver,
                },
                None => ActorTask::SnapshotMutation {
                    collection_id,
                    receiver,
                },
            }),
            Err(error) => {
                self.collection_ui.message = Some((true, collection_error_message(&error)));
                CollectionDialogOperation::Idle
            }
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
        self.collection_ui.show_manager = true;
        // The stamped Grid origin owns this mutation even when the manager last displayed another
        // collection. Conflict reload/retry must therefore target the origin collection.
        self.collection_ui
            .select_collection(Some(request.stamp.collection_id));
        self.collection_ui.operation = CollectionDialogOperation::ConfirmRemove {
            collection_id: request.stamp.collection_id,
            expected_revision: request.expected_revision,
            entry_ids: request.entry_ids,
            origin: Some(request.stamp),
        };
    }
}

#[derive(Clone, Copy)]
enum MoveEntry {
    First,
    Up,
    Down,
    Last,
}

enum CollectionUiAction {
    Select(CollectionId),
    SelectEntry(CollectionEntryId),
    OpenCreate,
    OpenRename,
    OpenDelete,
    OpenRemove,
    SetOrder {
        mode: CollectionOrderMode,
        sort: crate::settings::SortOrder,
    },
    MoveEntry(MoveEntry),
    PickFiles,
    PickFolder,
    PickImport,
    PickExport,
    PickRelinkFile,
    PickRelinkFolder,
}

impl CollectionUiAction {
    fn requires_ready(&self) -> bool {
        match self {
            Self::Select(_) | Self::SelectEntry(_) => false,
            Self::OpenCreate
            | Self::OpenRename
            | Self::OpenDelete
            | Self::OpenRemove
            | Self::SetOrder { .. }
            | Self::MoveEntry(_)
            | Self::PickFiles
            | Self::PickFolder
            | Self::PickImport
            | Self::PickExport
            | Self::PickRelinkFile
            | Self::PickRelinkFolder => true,
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
    use crate::settings::{Settings, SortOrder};

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

    fn start_ready_app(temp: &tempfile::TempDir) -> (App, CollectionStoreClient) {
        let runtime = CollectionStoreRuntime::start_at(temp.path().join("collection.db")).unwrap();
        let client = runtime.client();
        let mut app = App::new_from_settings(Settings::default());
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

    fn snapshot_app(preview_import: bool) -> App {
        let mut app = App::new_from_settings(Settings::default());
        let collection_id = CollectionId::new();
        let definition = CollectionDefinition {
            id: collection_id,
            name: "長い名前の旅行写真と資料コレクション".into(),
            order_mode: CollectionOrderMode::Manual,
            standard_sort: SortOrder::FileName,
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
        app.collection_ui.show_manager = true;
        app.collection_ui.message = Some((false, "2 件を追加しました。".into()));
        if preview_import {
            let preview = parse_collection_text(
                "D:\\Photo\\new-image.png\n\\\\server\\share\\network.png\nNUL\\bad.png\n",
                std::path::Path::new(r"D:\Import\collection.txt"),
            );
            app.collection_ui.operation = CollectionDialogOperation::PreviewImport {
                collection_id,
                expected_revision: 7,
                source_path: PathBuf::from(r"D:\Import\collection.txt"),
                preview,
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

        let mut app = snapshot_app(preview);
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
                app.show_collection_manager(ctx);
            });
        harness.run();
        harness.snapshot(name);
    }

    #[test]
    fn app_runtime_catalog_conflict_refresh_and_final_shutdown_use_one_owner() {
        let temp = tempfile::tempdir().unwrap();
        let mut inert = App::new_from_settings(Settings::default());
        assert!(matches!(
            inert.collection_ui.phase,
            CollectionRuntimePhase::Inert
        ));
        inert.shutdown_collection_runtime_for_exit();

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
            Some(surface_b.collection_id())
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
        wait_for(&mut app, |app| {
            app.collection_ui.operation.is_idle()
                && app.collection_ui.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.collection_id() == surface_b.collection_id()
                        && snapshot.revision() == external.revision()
                })
        });
        assert_eq!(
            app.collection_ui.selected_id,
            Some(surface_b.collection_id())
        );
        assert!(
            app.collection_ui
                .message
                .as_ref()
                .is_some_and(|(error, text)| { *error && text.contains("更新") })
        );
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn actual_handlers_commit_crud_relink_remove_and_import_only_after_confirmation() {
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
        app.collection_ui.selected_entry = Some(first_id);
        app.reorder_selected_collection_entry(&added, MoveEntry::Last);
        wait_for(&mut app, |app| app.collection_ui.operation.is_idle());
        let reordered = app.collection_ui.snapshot.clone().unwrap();
        assert_eq!(reordered.entries[0].id, second_id);
        assert_eq!(reordered.entries[1].id, first_id);

        app.apply_collection_ui_action(CollectionUiAction::SetOrder {
            mode: CollectionOrderMode::Standard,
            sort: SortOrder::SizeDesc,
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
        app.collection_ui.operation =
            app.submit_collection_remove(collection_id, relinked.revision(), vec![second_id], None);
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
                ..
            } => (expected_revision, preview),
            _ => unreachable!(),
        };
        app.collection_ui.operation =
            app.collection_import_classification_operation(collection_id, preview.0, &preview.1);
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
    fn failed_runtime_keeps_stale_snapshot_read_only_and_toolbar_reports_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, _client) = start_ready_app(&temp);
        let snapshot = create_collection(&mut app, "Read only");
        app.collection_ui.phase = CollectionRuntimePhase::Failed("actor stopped".into());

        app.apply_collection_ui_action(CollectionUiAction::SetOrder {
            mode: CollectionOrderMode::Standard,
            sort: SortOrder::DateDesc,
        });

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
}
