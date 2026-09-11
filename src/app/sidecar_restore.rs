//! Non-blocking coordinator for the sidecar recovery that precedes first display.
//!
//! The worker engine lives in `sidecar` / `sidecar_import`. This module owns the
//! App-global request identity and the continuation that starts DB hydration and
//! thumbnail workers only after recovery reaches a terminal state.

use super::*;
use std::sync::mpsc::{Receiver, TryRecvError};

const SIDECAR_WRITER_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const SIDECAR_RESTORE_MODAL_GRACE: std::time::Duration = std::time::Duration::from_millis(100);

pub(super) struct SidecarLoadContinuation {
    pub(super) source_path: PathBuf,
    /// Captured from the same metadata read that initialized folder-watch state. Extensions are
    /// not source kinds: a real directory may legitimately be named `photos.zip`.
    pub(super) source_is_directory: bool,
    pub(super) prepared_subfolder: Option<subfolder_expansion::PreparedSubfolderMetadata>,
    pub(super) prepared_aggregate: Option<subfolder_expansion::PreparedAggregateMetadata>,
    pub(super) catalog_existing_keys: HashSet<String>,
    pub(super) video_items: Vec<(usize, PathBuf, u64)>,
    pub(super) sli_seq: u64,
    pub(super) sli_t0: std::time::Instant,
    pub(super) items_len: usize,
    pub(super) detached_physical: bool,
    pub(super) tx: mpsc::Sender<ThumbMsg>,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) restore_started_at: std::time::Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequiredAction {
    Import { clear_preview: bool },
    ClearMissingMarkers,
    Resume,
    Reprobe,
    RefreshCache,
}

struct Common {
    request_id: u64,
    /// Presentation grace starts with the typed restore owner.  It never delays the
    /// quiescence/probe state machine or relaxes its input gate.
    started_at: std::time::Instant,
    target_context: ViewerContextId,
    items_generation: u64,
    folder: PathBuf,
    data_dir: PathBuf,
    families: crate::sidecar_import::ImportFamilies,
    tag_item_keys: Vec<String>,
    source_path: PathBuf,
    continuation: ContinuationOwner,
    deferred_fullscreen: Option<DeferredFullscreen>,
    favorite_failures: Vec<crate::favorite_view_state::FavoriteViewStoreCommand>,
    warning: Option<String>,
    source_change_reprobe: bool,
    /// A main-context folder transition historically flushed and dropped every old sidecar
    /// cache value. Checking leaves them live while its worker owns immutable reservations;
    /// successful completion evicts only exact captured clean owners, while detached opens and
    /// failed flushes keep the live cache unchanged.
    clear_sidecar_cache_after_flush: bool,
    effects: RestoreEffects,
}

struct DeferredFullscreen {
    idx: usize,
    trigger: HistoryTrigger,
    item_key: String,
    requested_materialization: FsOpenMaterialization,
    load_contract: FsPageLoadContract,
}

enum ContinuationOwner {
    Live(SidecarLoadContinuation),
    Discarded { cancel: Arc<AtomicBool> },
}

impl ContinuationOwner {
    fn cancel(&self) -> &Arc<AtomicBool> {
        match self {
            Self::Live(continuation) => &continuation.cancel,
            Self::Discarded { cancel } => cancel,
        }
    }

    fn discard(&mut self) -> bool {
        let cancel = Arc::clone(self.cancel());
        cancel.store(true, Ordering::Relaxed);
        let was_live = matches!(self, Self::Live(_));
        if was_live {
            *self = Self::Discarded { cancel };
        }
        was_live
    }

    fn take_live(self) -> Option<SidecarLoadContinuation> {
        match self {
            Self::Live(continuation) => Some(continuation),
            Self::Discarded { .. } => None,
        }
    }
}

impl Common {
    fn discard_target(&mut self) -> bool {
        self.deferred_fullscreen = None;
        self.continuation.discard()
    }
}

struct Checking {
    reservations: Option<crate::sidecar::SidecarFlushReservations>,
    rx: Receiver<CheckingWorkerResult>,
    handle: Option<std::thread::JoinHandle<()>>,
    /// The worker may publish its result before its thread has fully returned. Keep that result
    /// owned here until `JoinHandle::is_finished()` proves a normal-frame join cannot wait.
    terminal: Option<Result<CheckingWorkerResult, String>>,
}

struct CheckingWorkerResult {
    flush: Result<crate::sidecar::SidecarFlushReport, String>,
    probe: Option<crate::sidecar_import::SidecarImportProbe>,
}

struct Quiescing {
    local_adjust_fence: LocalAdjustFence,
    favorite_started: bool,
}

enum LocalAdjustFence {
    NotStarted,
    Waiting(crossbeam_channel::Receiver<()>),
    Reached,
}

enum RunningWorkerResult {
    Import(Result<crate::sidecar_import::SidecarImportCompletion, String>),
    MarkerClear(crate::sidecar_import::MissingMarkerClearCompletion),
    SourceChanged(String),
}

struct Running {
    rx: Receiver<RunningWorkerResult>,
}

struct CacheRefreshing {
    rx: Receiver<CacheRefreshCompletion>,
    mode: CacheRefreshMode,
}

#[derive(Clone, Copy)]
enum CacheRefreshMode {
    SidecarOnly,
    RecoverUnknownCommit,
}

struct CacheRefreshCompletion {
    sidecar: CacheRefreshResult,
    recovered_db_state: Option<Result<RecoveredDbState, String>>,
}

struct RecoveredDbState {
    edit_rollup: Option<EditRollupSnapshot>,
    tag_cache: Option<std::collections::HashMap<String, Vec<String>>>,
}

enum CacheRefreshResult {
    Current(crate::sidecar::SidecarFile),
    Disabled {
        sidecar: crate::sidecar::SidecarFile,
        error: String,
    },
}

struct InvalidatingPreview {
    action: RequiredAction,
    rx: std::sync::mpsc::Receiver<Result<(), String>>,
}

#[derive(Default)]
struct RestoreEffects {
    edits_changed: bool,
    tags_changed: bool,
    comic_changed: bool,
    edit_delta: crate::sidecar_import::EditImportDelta,
    tag_delta: Vec<(String, Vec<String>)>,
    edit_rollup: Option<EditRollupSnapshot>,
    tag_cache: Option<std::collections::HashMap<String, Vec<String>>>,
}

impl RestoreEffects {
    fn merge(&mut self, other: Self) {
        self.edits_changed |= other.edits_changed;
        self.tags_changed |= other.tags_changed;
        self.comic_changed |= other.comic_changed;
        self.edit_delta.adjusted.extend(other.edit_delta.adjusted);
        self.edit_delta.masked.extend(other.edit_delta.masked);
        self.edit_delta.concealed.extend(other.edit_delta.concealed);
        self.edit_delta
            .local_adjusted
            .extend(other.edit_delta.local_adjusted);
        self.edit_delta.cropped.extend(other.edit_delta.cropped);
        self.edit_delta.comic.extend(other.edit_delta.comic);
        self.tag_delta.extend(other.tag_delta);
        if other.edit_rollup.is_some() {
            self.edit_rollup = other.edit_rollup;
        }
        if other.tag_cache.is_some() {
            self.tag_cache = other.tag_cache;
        }
    }
}

type EditRollupSnapshot = crate::metadata_transfer::ImportPageStateSnapshot;

struct RestoreTerminal {
    sidecar: Option<crate::sidecar::SidecarFile>,
    effects: RestoreEffects,
    source_changed: Option<String>,
    warning: Option<String>,
}

enum Phase {
    Checking(Checking),
    Quiescing(Quiescing),
    Running(Running),
    CacheRefreshing(CacheRefreshing),
    InvalidatingPreview(InvalidatingPreview),
    Resuming,
}

#[derive(Debug, PartialEq, Eq)]
enum PreviewClearPoll {
    Pending,
    Cleared,
    Failed(String),
}

enum CheckingPoll {
    Pending,
    Complete(Result<CheckingWorkerResult, String>),
}

fn quiescence_barrier_ready(
    local_fence_reached: bool,
    local_pending_empty: bool,
    favorite_busy_before_drain: bool,
    conflicting_work: bool,
) -> bool {
    local_fence_reached && local_pending_empty && !favorite_busy_before_drain && !conflicting_work
}

fn poll_preview_clear_completion(
    rx: &std::sync::mpsc::Receiver<Result<(), String>>,
) -> PreviewClearPoll {
    match rx.try_recv() {
        Ok(Ok(())) => PreviewClearPoll::Cleared,
        Ok(Err(error)) => PreviewClearPoll::Failed(error),
        Err(TryRecvError::Empty) => PreviewClearPoll::Pending,
        Err(TryRecvError::Disconnected) => PreviewClearPoll::Failed(
            "編集プレビューキャッシュの消去完了を確認できませんでした".into(),
        ),
    }
}

fn poll_checking_worker(checking: &mut Checking) -> CheckingPoll {
    if checking.terminal.is_none() {
        checking.terminal = match checking.rx.try_recv() {
            Ok(result) => Some(Ok(result)),
            Err(TryRecvError::Disconnected) => Some(Err(
                "sidecar restore checking result channel disconnected".into(),
            )),
            Err(TryRecvError::Empty) => None,
        };
    }

    let Some(handle) = checking.handle.as_ref() else {
        return CheckingPoll::Complete(Err(
            "sidecar restore checking worker handle is missing".into()
        ));
    };
    if !handle.is_finished() {
        return CheckingPoll::Pending;
    }

    let join = checking
        .handle
        .take()
        .expect("checking worker handle checked above")
        .join()
        .map_err(|_| "sidecar restore checking worker panicked".to_string());
    if let Err(error) = join {
        return CheckingPoll::Complete(Err(error));
    }
    if checking.terminal.is_none() {
        checking.terminal = Some(checking.rx.try_recv().map_err(|error| {
            format!("sidecar restore checking result unavailable after worker exit: {error}")
        }));
    }
    CheckingPoll::Complete(
        checking
            .terminal
            .take()
            .expect("finished checking worker has a terminal channel outcome"),
    )
}

pub(crate) struct SidecarRestoreState {
    common: Common,
    phase: Phase,
}

impl SidecarRestoreState {
    pub(crate) fn target_context(&self) -> ViewerContextId {
        self.common.target_context
    }

    pub(crate) fn target_matches(&self, context: ViewerContextId, generation: u64) -> bool {
        self.common.target_context == context && self.common.items_generation == generation
    }

    pub(crate) fn defer_fullscreen(
        &mut self,
        context: ViewerContextId,
        generation: u64,
        idx: usize,
        trigger: HistoryTrigger,
        item_key: String,
        requested_materialization: FsOpenMaterialization,
        load_contract: FsPageLoadContract,
    ) -> bool {
        if !self.target_matches(context, generation) {
            return false;
        }
        self.common.deferred_fullscreen = Some(DeferredFullscreen {
            idx,
            trigger,
            item_key,
            requested_materialization,
            load_contract,
        });
        true
    }

    pub(crate) fn label(&self) -> &'static str {
        match self.phase {
            Phase::Checking(_) => "サイドカーを確認中",
            Phase::Quiescing(_) => "保存中の設定を確定中",
            Phase::Running(_) => "サイドカーから設定を復元中",
            Phase::CacheRefreshing(_) => "サイドカー状態を更新中",
            Phase::InvalidatingPreview(_) => "表示キャッシュを更新中",
            Phase::Resuming => "復元した設定を反映中",
        }
    }

    pub(crate) fn modal_delay_remaining(&self, now: std::time::Instant) -> std::time::Duration {
        SIDECAR_RESTORE_MODAL_GRACE
            .saturating_sub(now.saturating_duration_since(self.common.started_at))
    }
}

impl App {
    pub(crate) fn sidecar_restore_active(&self) -> bool {
        self.sidecar_restore.is_some()
    }

    /// Whether the currently projected viewer is the restore target.
    ///
    /// The coordinator and persistence barriers remain App-global, but only the target
    /// viewer owns the wait modal and semantic-input block. Independent native-video
    /// windows continue their normal event path while another context is restoring.
    pub(crate) fn sidecar_restore_blocks_projected_context(&self) -> bool {
        self.sidecar_restore.as_ref().is_some_and(|state| {
            state.common.target_context == self.sidecar_restore_projected_context()
        })
    }

    #[cfg(windows)]
    pub(crate) fn sidecar_restore_target_window_id(&self) -> Option<u64> {
        self.sidecar_restore
            .as_ref()
            .and_then(|state| self.viewer_context_window(state.target_context()))
    }

    #[cfg(windows)]
    pub(crate) fn sidecar_restore_blocks_window(&self, window_id: u64) -> bool {
        self.sidecar_restore_target_window_id() == Some(window_id)
    }

    #[cfg(test)]
    pub(crate) fn activate_sidecar_restore_modal_for_test(&mut self, folder: PathBuf) {
        let target_context = self.sidecar_restore_projected_context();
        self.sidecar_restore = Some(SidecarRestoreState {
            common: Common {
                request_id: 1,
                started_at: std::time::Instant::now(),
                target_context,
                items_generation: self.items_generation,
                folder: folder.clone(),
                data_dir: folder.clone(),
                families: crate::sidecar_import::ImportFamilies {
                    edits: true,
                    tags: true,
                },
                tag_item_keys: Vec::new(),
                source_path: folder,
                continuation: ContinuationOwner::Discarded {
                    cancel: Arc::new(AtomicBool::new(false)),
                },
                deferred_fullscreen: None,
                favorite_failures: Vec::new(),
                warning: None,
                source_change_reprobe: false,
                clear_sidecar_cache_after_flush: false,
                effects: RestoreEffects::default(),
            },
            phase: Phase::Resuming,
        });
    }

    /// Discard semantic input while keeping release edges available to the
    /// existing hold owners. Nothing retained here is queued for replay after
    /// the restore terminal.
    pub(crate) fn consume_input_during_sidecar_restore(&mut self, ctx: &egui::Context) {
        if !self.sidecar_restore_blocks_projected_context() {
            return;
        }
        Self::sanitize_sidecar_restore_viewport_input(
            ctx,
            !self.sidecar_restore_root_lifecycle_close_requested(),
        );
        // These owners were derived before the modal began, so filtering RawInput alone cannot
        // release them.  End the gestures explicitly on every mounted-context pass; none of the
        // transient pointer deltas is replayed after restore.
        self.compare_wipe_dragging = false;
        self.fs_middle_zoom_drag = None;
        self.mouse_middle_click_start = None;
        self.analysis_guide_drag = None;
        self.pending_native_drag = None;
        self.fs_seek_drag_active = false;
        self.fs_seek_gesture = crate::ui_fullscreen::StillSeekGesture::Idle;
        self.fullscreen_navigator_interaction
            .end_pointer_gesture_for_park();
        self.capture_region_selection = None;
        self.fs_secondary_press.cancel();
        self.finish_gamepad_input_for_sidecar_restore(ctx);
    }

    /// Sanitize one egui viewport before any of its direct input consumers run.
    ///
    /// `egui::Modal` captures widgets after it is drawn, but child viewports own independent
    /// input state and the top modal layer initially reflects the previous pass.  Raw event
    /// filtering therefore is insufficient: scroll and pointer button state have already been
    /// derived by egui.  Reset those derived values at the viewport boundary as well.
    pub(crate) fn consume_sidecar_restore_viewport_input(ctx: &egui::Context) {
        Self::sanitize_sidecar_restore_viewport_input(ctx, true);
    }

    fn sanitize_sidecar_restore_viewport_input(ctx: &egui::Context, cancel_close: bool) {
        if cancel_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        ctx.input_mut(|input| {
            input.events.retain(sidecar_restore_retains_egui_event);
            input.raw_scroll_delta = egui::Vec2::ZERO;
            input.smooth_scroll_delta = egui::Vec2::ZERO;
            input.pointer = egui::PointerState::default();
            input.keys_down.clear();
        });
        ctx.stop_dragging();
    }

    /// User close is part of the modal's semantic-input block. Tray/installer shutdown is an
    /// unavoidable process lifecycle request and must retain the existing shutdown path.
    pub(crate) fn sidecar_restore_root_lifecycle_close_requested(&self) -> bool {
        self.tray_controller
            .as_ref()
            .is_some_and(|controller| controller.is_quit_requested())
            || self.shutdown_requested.load(Ordering::SeqCst)
    }

    pub(crate) fn sidecar_restore_blocks_root_tray_hide(&self) -> bool {
        self.sidecar_restore_active() && !self.sidecar_restore_root_lifecycle_close_requested()
    }

    /// Resolve App-owned recovery state before the existing process-exit persistence boundary.
    ///
    /// During `Checking`, the worker owns immutable snapshots while their live cache owners
    /// remain in `self.sidecars`. Join this worker and reconcile its exact reservations before
    /// the existing exit flush; otherwise a disconnected writer can make both the worker and
    /// exit fallback write the same fixed temporary path concurrently. A cache owner mutated
    /// after capture keeps its newer dirty state for the established exit writer contract.
    pub(crate) fn resolve_sidecar_restore_for_exit(&mut self) {
        let Some(mut state) = self.sidecar_restore.take() else {
            return;
        };
        state.common.continuation.discard();
        if let Phase::Checking(mut checking) = state.phase {
            let join_result = checking
                .handle
                .take()
                .expect("checking owns its worker handle")
                .join()
                .map_err(|_| "sidecar restore checking worker panicked during exit".to_string());
            let worker_result = join_result.and_then(|()| match checking.terminal.take() {
                Some(result) => result,
                None => checking.rx.try_recv().map_err(|error| {
                    format!("sidecar restore checking result unavailable: {error}")
                }),
            });
            let reservations = checking
                .reservations
                .take()
                .expect("checking owns flush reservations");
            let flush_result = match worker_result {
                Ok(worker) => worker.flush,
                Err(error) => Err(error),
            };
            if let Err(error) =
                reservations.resolve_in_place(&mut self.sidecars, flush_result, false)
            {
                crate::logger::log(format!(
                    "sidecar restore exit flush reconciliation failed: {error}"
                ));
            }
        }
        for command in std::mem::take(&mut state.common.favorite_failures) {
            self.restore_failed_favorite_command(command);
        }
        crate::logger::log(format!(
            "sidecar restore cancelled for process exit request={}",
            state.common.request_id
        ));
    }

    pub(crate) fn defer_sidecar_restore_fullscreen(
        &mut self,
        idx: usize,
        trigger: HistoryTrigger,
        requested_materialization: FsOpenMaterialization,
        load_contract: FsPageLoadContract,
    ) -> bool {
        if self.sidecar_restore.is_none() {
            return false;
        }
        let context = self.sidecar_restore_projected_context();
        let generation = self.items_generation;
        if let Some(item_key) = self.items.get(idx).map(GridItem::perf_key) {
            if let Some(state) = self.sidecar_restore.as_mut() {
                let _ = state.defer_fullscreen(
                    context,
                    generation,
                    idx,
                    trigger,
                    item_key,
                    requested_materialization,
                    load_contract,
                );
            }
        }
        // Active means every fullscreen mutation is consumed. Only an exact target request is
        // retained above; an invalid/sibling request must not fall through to raw/final/video work.
        true
    }

    #[cfg(windows)]
    fn sidecar_restore_projected_context(&self) -> ViewerContextId {
        self.projected_viewer_context_id()
    }

    #[cfg(not(windows))]
    fn sidecar_restore_projected_context(&self) -> ViewerContextId {
        ViewerContextId::single_context()
    }

    fn sidecar_restore_eligible(&self, source_path: &Path, has_prepared_aggregate: bool) -> bool {
        !has_prepared_aggregate
            && !is_synthetic_view_path(source_path)
            && (self.settings.sidecar_backup_enabled || self.settings.tag_sidecar_backup_enabled)
    }

    fn sidecar_restore_folder(source_path: &Path, source_is_directory: bool) -> PathBuf {
        if !source_is_directory {
            source_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| source_path.to_path_buf())
        } else {
            source_path.to_path_buf()
        }
    }

    pub(super) fn begin_sidecar_restore(
        &mut self,
        continuation: SidecarLoadContinuation,
        has_prepared_aggregate: bool,
    ) -> Result<(), SidecarLoadContinuation> {
        if !self.sidecar_restore_eligible(&continuation.source_path, has_prepared_aggregate) {
            return Err(continuation);
        }
        if self.sidecar_restore.is_some() {
            crate::logger::log(
                "sidecar restore request rejected: another restore already owns the App"
                    .to_string(),
            );
            return Err(continuation);
        }

        let target_context = self.sidecar_restore_projected_context();
        let items_generation = self.items_generation;
        let folder = Self::sidecar_restore_folder(
            &continuation.source_path,
            continuation.source_is_directory,
        );
        let families = crate::sidecar_import::ImportFamilies {
            edits: self.settings.sidecar_backup_enabled,
            tags: self.settings.tag_sidecar_backup_enabled,
        };
        let mut tag_item_keys = self
            .items
            .iter()
            .filter_map(tag_item_path)
            .map(crate::tags_db::item_key_for_path)
            .collect::<Vec<_>>();
        tag_item_keys.sort();
        tag_item_keys.dedup();
        let data_dir = crate::data_dir::get();
        let source_path = continuation.source_path.clone();
        let clear_sidecar_cache_after_flush = !continuation.detached_physical;

        let request_id = self
            .frame_counter
            .wrapping_add(self.input_seq)
            .wrapping_add(1);
        let started_at = continuation.restore_started_at;
        self.sidecar_restore = Some(SidecarRestoreState {
            common: Common {
                request_id,
                started_at,
                target_context,
                items_generation,
                folder,
                data_dir,
                families,
                tag_item_keys,
                source_path,
                continuation: ContinuationOwner::Live(continuation),
                deferred_fullscreen: None,
                favorite_failures: Vec::new(),
                warning: None,
                source_change_reprobe: false,
                clear_sidecar_cache_after_flush,
                effects: RestoreEffects::default(),
            },
            // Keep every cache owner in the App throughout recovery. Checking only borrows
            // immutable Arc snapshots after accepted DB completions and their sidecar mirrors
            // have crossed the quiescence barriers.
            phase: Phase::Quiescing(Quiescing {
                local_adjust_fence: LocalAdjustFence::NotStarted,
                favorite_started: false,
            }),
        });
        #[cfg(windows)]
        {
            let discarded = self.activation_open_path_rx.try_iter().count();
            if discarded > 0 {
                crate::logger::log(format!(
                    "sidecar restore discarded {discarded} queued activation open request(s)"
                ));
            }
        }
        crate::logger::log(format!(
            "sidecar restore started request={request_id} context={target_context:?} generation={items_generation} folder={}",
            self.sidecar_restore
                .as_ref()
                .unwrap()
                .common
                .folder
                .display()
        ));
        // The restore can start after the root pass' ordinary input gate.  Sanitize that same
        // pass immediately so the first modal frame has no pointer/scroll layer hole.
        if let Some(ctx) = self.edit_preview_repaint_ctx.clone() {
            self.consume_input_during_sidecar_restore(&ctx);
        }
        Ok(())
    }

    fn report_sidecar_restore_flush_failure(&mut self, error: String) {
        crate::logger::log(format!("sidecar restore flush failed: {error}"));
        self.show_feedback_toast(format!("サイドカーの保存を完了できませんでした: {error}"));
    }

    fn sidecar_restore_context_current(&self, common: &Common) -> bool {
        self.sidecar_restore_projected_context() == common.target_context
            && self.items_generation == common.items_generation
            && self
                .current_folder
                .as_deref()
                .is_some_and(|folder| crate::folder_tree::path_eq(folder, &common.source_path))
    }

    fn sidecar_restore_has_conflicting_work(
        &mut self,
        release_tag_db_for_import: bool,
    ) -> Result<bool, String> {
        let transfer_busy =
            self.metadata_transfer_writers_busy_for_sidecar_restore(release_tag_db_for_import)?;
        Ok(transfer_busy
            || self.edit_bundle_paste_pending.is_some()
            || self.edit_bundle_apply_pending.is_some()
            || self.edit_bundle_bulk_pending.is_some()
            || self.content_identity_restore_pending.is_some()
            || self.metadata_transfer_active_for_sidecar_restore())
    }

    fn sidecar_restore_start_checking(&mut self, state: &mut SidecarRestoreState) -> Phase {
        let (reservations, batch) =
            crate::sidecar::prepare_worker_flush_in_place(self.sidecars.values());
        let (tx, rx) = mpsc::channel();
        let folder = state.common.folder.clone();
        let data_dir = state.common.data_dir.clone();
        let families = state.common.families;
        let cancel = Arc::clone(state.common.continuation.cancel());
        let spawned = std::thread::Builder::new()
            .name("sidecar-restore-recheck".to_owned())
            .spawn(move || {
                let flush = batch.run_on_worker(SIDECAR_WRITER_IDLE_TIMEOUT);
                let probe = flush
                    .as_ref()
                    .ok()
                    .map(|_| crate::sidecar_import::probe(&folder, &data_dir, families, &cancel));
                let _ = tx.send(CheckingWorkerResult { flush, probe });
            });
        match spawned {
            Err(error) => {
                state.common.warning =
                    Some(format!("復元前の保存確認を開始できませんでした: {error}"));
                Phase::Resuming
            }
            Ok(handle) => Phase::Checking(Checking {
                reservations: Some(reservations),
                rx,
                handle: Some(handle),
                terminal: None,
            }),
        }
    }

    fn sidecar_restore_start_running(
        &mut self,
        state: &mut SidecarRestoreState,
        action: RequiredAction,
    ) -> Phase {
        let (tx, rx) = mpsc::channel();
        let folder = state.common.folder.clone();
        let data_dir = state.common.data_dir.clone();
        let families = state.common.families;
        let cancel = Arc::clone(state.common.continuation.cancel());
        let spawned = std::thread::Builder::new()
            .name("sidecar-restore-commit".to_owned())
            .spawn(move || {
                let result = match action {
                    RequiredAction::Import { .. } => {
                        match crate::sidecar::SidecarFile::load_for_import(&folder) {
                            crate::sidecar::SidecarImportLoad::Loaded(loaded) => {
                                RunningWorkerResult::Import(
                                    crate::sidecar_import::prepare(loaded, families, &cancel).map(
                                        |prepared| {
                                            crate::sidecar_import::commit(
                                                &data_dir, prepared, &cancel,
                                            )
                                        },
                                    ),
                                )
                            }
                            _ => RunningWorkerResult::SourceChanged(
                                "sidecar source changed before the import worker loaded it"
                                    .to_string(),
                            ),
                        }
                    }
                    RequiredAction::ClearMissingMarkers => RunningWorkerResult::MarkerClear(
                        crate::sidecar_import::clear_missing_markers(
                            &folder, &data_dir, families, &cancel,
                        ),
                    ),
                    RequiredAction::Resume
                    | RequiredAction::Reprobe
                    | RequiredAction::RefreshCache => {
                        unreachable!("non-commit actions never start a commit worker")
                    }
                };
                let _ = tx.send(result);
            });
        if let Err(error) = spawned {
            state.common.warning = Some(format!(
                "サイドカー復元workerを開始できませんでした: {error}"
            ));
            self.sidecar_restore_start_cache_refresh(state)
        } else {
            Phase::Running(Running { rx })
        }
    }

    fn sidecar_restore_start_preview_clear(
        &mut self,
        state: &mut SidecarRestoreState,
        action: RequiredAction,
    ) -> Phase {
        let Some(cache) = self.edit_preview_cache.as_ref() else {
            append_warning(
                &mut state.common.warning,
                "編集プレビューDBを開けないため、サイドカー復元を開始しませんでした".to_string(),
            );
            return self.sidecar_restore_start_cache_refresh(state);
        };
        match cache.clear_with_completion() {
            Ok(rx) => Phase::InvalidatingPreview(InvalidatingPreview { action, rx }),
            Err(error) => {
                append_warning(&mut state.common.warning, error);
                self.sidecar_restore_start_cache_refresh(state)
            }
        }
    }

    fn sidecar_restore_start_cache_refresh(&mut self, state: &mut SidecarRestoreState) -> Phase {
        self.sidecar_restore_start_cache_refresh_with_mode(state, CacheRefreshMode::SidecarOnly)
    }

    fn sidecar_restore_start_unknown_commit_recovery(
        &mut self,
        state: &mut SidecarRestoreState,
    ) -> Phase {
        self.sidecar_restore_start_cache_refresh_with_mode(
            state,
            CacheRefreshMode::RecoverUnknownCommit,
        )
    }

    fn sidecar_restore_start_cache_refresh_with_mode(
        &mut self,
        state: &mut SidecarRestoreState,
        mode: CacheRefreshMode,
    ) -> Phase {
        let folder = state.common.folder.clone();
        let data_dir = state.common.data_dir.clone();
        let families = state.common.families;
        let tag_item_keys = state.common.tag_item_keys.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("sidecar-restore-cache-refresh".to_owned())
            .spawn(move || {
                let sidecar = load_current_sidecar_cache(&folder, families);
                let recovered_db_state = matches!(mode, CacheRefreshMode::RecoverUnknownCommit)
                    .then(|| load_recovered_db_state(&data_dir, families, &tag_item_keys));
                let _ = tx.send(CacheRefreshCompletion {
                    sidecar,
                    recovered_db_state,
                });
            });
        if let Err(error) = spawned {
            append_warning(
                &mut state.common.warning,
                format!("サイドカー状態の更新workerを開始できませんでした: {error}"),
            );
            if let Some(warning) = self.disable_sidecar_restore_cache_owner(&state.common.folder) {
                append_warning(&mut state.common.warning, warning);
            }
            if matches!(mode, CacheRefreshMode::RecoverUnknownCommit) && state.common.families.edits
            {
                state.common.effects.comic_changed = true;
            }
            Phase::Resuming
        } else {
            Phase::CacheRefreshing(CacheRefreshing { rx, mode })
        }
    }

    fn apply_sidecar_cache_refresh(
        &mut self,
        common: &mut Common,
        completion: CacheRefreshCompletion,
    ) {
        match completion.sidecar {
            CacheRefreshResult::Current(sidecar) => {
                if let Some(warning) = self.install_sidecar_restore_cache_owner(sidecar) {
                    append_warning(&mut common.warning, warning);
                }
            }
            CacheRefreshResult::Disabled { sidecar, error } => {
                append_warning(&mut common.warning, error);
                if let Some(warning) = self.install_sidecar_restore_cache_owner(sidecar) {
                    append_warning(&mut common.warning, warning);
                }
            }
        }
        if let Some(recovered) = completion.recovered_db_state {
            // A disconnected commit worker leaves the transaction outcome unknown.  A successful
            // worker snapshot restores authoritative global/index state; a finite read failure is
            // reported without fabricating empty caches or claiming a corrected first display.
            common.effects.comic_changed |= common.families.edits;
            match recovered {
                Ok(recovered) => {
                    if let Some(edit_rollup) = recovered.edit_rollup {
                        common.effects.edits_changed = true;
                        common.effects.edit_rollup = Some(edit_rollup);
                    }
                    if let Some(tag_cache) = recovered.tag_cache {
                        common.effects.tags_changed = true;
                        common.effects.tag_cache = Some(tag_cache);
                    }
                }
                Err(error) => append_warning(&mut common.warning, error),
            }
        }
    }

    fn install_sidecar_restore_cache_owner(
        &mut self,
        sidecar: crate::sidecar::SidecarFile,
    ) -> Option<String> {
        let folder = sidecar.folder().to_path_buf();
        if self
            .sidecars
            .get(&folder)
            .is_some_and(crate::sidecar::SidecarFile::is_dirty)
        {
            return Some(
                "サイドカー原本保護中の未保存変更を保持したため、復元スナップショットをキャッシュへ反映できませんでした"
                    .to_string(),
            );
        }
        self.sidecars.insert(folder, sidecar);
        None
    }

    fn disable_sidecar_restore_cache_owner(&mut self, folder: &Path) -> Option<String> {
        self.install_sidecar_restore_cache_owner(crate::sidecar::SidecarFile::disabled_placeholder(
            folder.to_path_buf(),
        ))
    }

    fn sidecar_restore_finish_terminal(
        &mut self,
        state: &mut SidecarRestoreState,
        sidecar: Option<crate::sidecar::SidecarFile>,
    ) -> Phase {
        let effects = std::mem::take(&mut state.common.effects);
        if let Some(sidecar) = sidecar {
            if let Some(warning) = self.install_sidecar_restore_result(Some(sidecar), effects) {
                append_warning(&mut state.common.warning, warning);
            }
        } else {
            if let Some(warning) = self.disable_sidecar_restore_cache_owner(&state.common.folder) {
                append_warning(&mut state.common.warning, warning);
            }
            debug_assert!(self.install_sidecar_restore_result(None, effects).is_none());
        }
        Phase::Resuming
    }

    /// Advance the restore state without waiting. Called once per root frame.
    pub(crate) fn poll_sidecar_restore(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.sidecar_restore.take() else {
            return;
        };
        let target = state.common.target_context;
        #[cfg(windows)]
        {
            if self.projected_viewer_context_id() != target {
                match self.viewer_context_residence(target) {
                    ContextResidence::Mounted | ContextResidence::AtRest => {
                        self.sidecar_restore = Some(state);
                        if let Err(error) = self.with_viewer_context(target, |app| {
                            app.poll_sidecar_restore_projected(ctx)
                        }) {
                            crate::logger::log(format!(
                                "sidecar restore target mount failed context={target:?}: {error:?}"
                            ));
                        }
                    }
                    ContextResidence::Building | ContextResidence::Retiring => {
                        self.sidecar_restore = Some(state);
                        ctx.request_repaint_after(std::time::Duration::from_millis(16));
                    }
                    ContextResidence::Retired | ContextResidence::Unknown => {
                        if state.common.discard_target() {
                            crate::logger::log(format!(
                                "sidecar restore target disappeared context={target:?} residence={:?}",
                                self.viewer_context_residence(target)
                            ));
                        }
                        self.poll_discarded_sidecar_restore(ctx, state);
                    }
                }
                return;
            }
        }
        self.sidecar_restore = Some(state);
        self.poll_sidecar_restore_projected(ctx);
    }

    fn poll_sidecar_restore_projected(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.sidecar_restore.take() else {
            return;
        };
        if !self.sidecar_restore_context_current(&state.common) {
            if state.common.discard_target() {
                crate::logger::log(format!(
                    "sidecar restore target changed request={} context={:?} generation={}",
                    state.common.request_id,
                    state.common.target_context,
                    state.common.items_generation
                ));
            }
            self.poll_discarded_sidecar_restore(ctx, state);
            return;
        }

        let phase = std::mem::replace(&mut state.phase, Phase::Resuming);
        state.phase = match phase {
            Phase::Checking(mut checking) => match poll_checking_worker(&mut checking) {
                CheckingPoll::Pending => Phase::Checking(checking),
                CheckingPoll::Complete(Err(error)) => {
                    let reservations = checking
                        .reservations
                        .take()
                        .expect("checking owns flush reservations");
                    let error = reservations
                        .resolve_in_place(&mut self.sidecars, Err(error), false)
                        .expect_err("an explicit checking error cannot resolve successfully");
                    self.report_sidecar_restore_flush_failure(error);
                    state.common.warning = Some("サイドカーの確認workerが停止しました".into());
                    Phase::Resuming
                }
                CheckingPoll::Complete(Ok(worker)) => {
                    let reservations = checking
                        .reservations
                        .take()
                        .expect("checking owns flush reservations");
                    if let Err(error) = reservations.resolve_in_place(
                        &mut self.sidecars,
                        worker.flush,
                        state.common.clear_sidecar_cache_after_flush,
                    ) {
                        self.report_sidecar_restore_flush_failure(error);
                        state.common.warning = Some(
                            "サイドカー保存が完了しなかったため、中央DBから表示を続けます"
                                .to_string(),
                        );
                        Phase::Resuming
                    } else {
                        let probe = worker.probe.expect("successful flush includes a probe");
                        let action = match probe {
                            crate::sidecar_import::SidecarImportProbe::Current {
                                sidecar,
                                result,
                            } => {
                                if let Some(warning) =
                                    self.install_sidecar_restore_cache_owner(sidecar)
                                {
                                    append_warning(&mut state.common.warning, warning);
                                }
                                if probe_result_has_failure(&result) {
                                    append_warning(
                                        &mut state.common.warning,
                                        probe_result_warning(&result),
                                    );
                                }
                                RequiredAction::Resume
                            }
                            crate::sidecar_import::SidecarImportProbe::ImportRequired {
                                result,
                            } => RequiredAction::Import {
                                clear_preview: probe_requires_preview_clear(&result),
                            },
                            crate::sidecar_import::SidecarImportProbe::MarkerClearRequired {
                                ..
                            } => RequiredAction::ClearMissingMarkers,
                            crate::sidecar_import::SidecarImportProbe::SourceChanged {
                                error,
                                ..
                            } => {
                                state.common.warning = Some(error);
                                if state.common.source_change_reprobe {
                                    if let Some(warning) = self
                                        .disable_sidecar_restore_cache_owner(&state.common.folder)
                                    {
                                        append_warning(&mut state.common.warning, warning);
                                    }
                                    RequiredAction::Resume
                                } else {
                                    state.common.source_change_reprobe = true;
                                    RequiredAction::Reprobe
                                }
                            }
                            crate::sidecar_import::SidecarImportProbe::Cancelled { .. } => {
                                state.common.warning =
                                    Some("サイドカー復元が取り消されました".into());
                                RequiredAction::RefreshCache
                            }
                            crate::sidecar_import::SidecarImportProbe::Failed { error, .. } => {
                                state.common.warning = Some(error);
                                RequiredAction::RefreshCache
                            }
                        };
                        match action {
                            RequiredAction::Resume => Phase::Resuming,
                            RequiredAction::Import {
                                clear_preview: true,
                            } => self.sidecar_restore_start_preview_clear(&mut state, action),
                            RequiredAction::Import {
                                clear_preview: false,
                            }
                            | RequiredAction::ClearMissingMarkers => {
                                self.sidecar_restore_start_running(&mut state, action)
                            }
                            RequiredAction::Reprobe => {
                                self.sidecar_restore_start_checking(&mut state)
                            }
                            RequiredAction::RefreshCache => {
                                self.sidecar_restore_start_cache_refresh(&mut state)
                            }
                        }
                    }
                }
            },
            Phase::Quiescing(mut quiescing) => {
                let mut terminal_error = None;
                if matches!(quiescing.local_adjust_fence, LocalAdjustFence::NotStarted) {
                    quiescing.local_adjust_fence = match self.local_adjust_write_handle.as_ref() {
                        Some(handle) => match handle.enqueue_fence() {
                            Ok(rx) => LocalAdjustFence::Waiting(rx),
                            Err(error) => {
                                terminal_error = Some(error);
                                LocalAdjustFence::Reached
                            }
                        },
                        None => LocalAdjustFence::Reached,
                    };
                }
                if let LocalAdjustFence::Waiting(rx) = &quiescing.local_adjust_fence {
                    match rx.try_recv() {
                        Ok(()) => quiescing.local_adjust_fence = LocalAdjustFence::Reached,
                        Err(crossbeam_channel::TryRecvError::Empty) => {}
                        Err(crossbeam_channel::TryRecvError::Disconnected) => {
                            terminal_error =
                                Some("補正レイヤー保存の完了を確認できませんでした".into());
                        }
                    }
                }
                if !quiescing.favorite_started {
                    // Results already queued belong to commands that predate this restore.
                    // Drain them before appending the debounced Set commands so failures retain
                    // the writer's FIFO order in the state-owned recovery list.
                    let prior_results: Vec<_> = self
                        .favorite_view_store_writer
                        .as_ref()
                        .map(|writer| std::iter::from_fn(|| writer.try_recv()).collect())
                        .unwrap_or_default();
                    for result in prior_results {
                        if result.result.is_err() {
                            state.common.favorite_failures.push(result.command);
                        }
                    }
                    for (id, favorite_state) in self.favorite_view_writes.take_all() {
                        let command = crate::favorite_view_state::FavoriteViewStoreCommand::Set {
                            id,
                            state: favorite_state,
                        };
                        if let Err(error) = self.submit_favorite_view_store(command.clone()) {
                            crate::logger::log(format!(
                                "favorite view state submit failed during sidecar restore: {error}"
                            ));
                            state.common.favorite_failures.push(command);
                        }
                    }
                    quiescing.favorite_started = true;
                }
                // The worker publishes its result before advancing `done`.  Observe busy with
                // Acquire first, then drain: a false observation guarantees every prior result
                // is already visible and therefore consumed in this frame.
                let favorite_busy_before_drain = self
                    .favorite_view_store_writer
                    .as_ref()
                    .is_some_and(|writer| writer.is_busy());
                let results: Vec<_> = self
                    .favorite_view_store_writer
                    .as_ref()
                    .map(|writer| std::iter::from_fn(|| writer.try_recv()).collect())
                    .unwrap_or_default();
                for result in results {
                    if result.result.is_err() {
                        state.common.favorite_failures.push(result.command);
                    }
                }
                if let Some(error) = terminal_error {
                    state.common.warning = Some(error);
                    Phase::Resuming
                } else {
                    match self.sidecar_restore_has_conflicting_work(state.common.families.tags) {
                        Ok(conflicting)
                            if quiescence_barrier_ready(
                                matches!(quiescing.local_adjust_fence, LocalAdjustFence::Reached),
                                self.local_adjust_write_pending.is_empty(),
                                favorite_busy_before_drain,
                                conflicting,
                            ) =>
                        {
                            self.sidecar_restore_start_checking(&mut state)
                        }
                        Ok(_) => Phase::Quiescing(quiescing),
                        Err(error) => {
                            state.common.warning = Some(error);
                            Phase::Resuming
                        }
                    }
                }
            }
            Phase::Running(running) => match running.rx.try_recv() {
                Err(TryRecvError::Empty) => Phase::Running(running),
                Err(TryRecvError::Disconnected) => {
                    state.common.warning = Some("サイドカー復元workerが停止しました".into());
                    self.sidecar_restore_start_unknown_commit_recovery(&mut state)
                }
                Ok(result) => {
                    let terminal = restore_terminal(result);
                    state.common.effects.merge(terminal.effects);
                    if let Some(warning) = terminal.warning {
                        append_warning(&mut state.common.warning, warning);
                    }
                    if let Some(error) = terminal.source_changed {
                        append_warning(&mut state.common.warning, error);
                        if state.common.source_change_reprobe {
                            self.sidecar_restore_start_cache_refresh(&mut state)
                        } else {
                            // The pre-import cache owner no longer proves the disk source. Disable
                            // it before the strict re-probe so no intervening save can roll the
                            // sidecar back if the refresh itself fails.
                            if let Some(warning) =
                                self.disable_sidecar_restore_cache_owner(&state.common.folder)
                            {
                                append_warning(&mut state.common.warning, warning);
                            }
                            state.common.source_change_reprobe = true;
                            self.sidecar_restore_start_checking(&mut state)
                        }
                    } else {
                        match terminal.sidecar {
                            Some(sidecar) => {
                                self.sidecar_restore_finish_terminal(&mut state, Some(sidecar))
                            }
                            None => self.sidecar_restore_start_cache_refresh(&mut state),
                        }
                    }
                }
            },
            Phase::CacheRefreshing(refreshing) => match refreshing.rx.try_recv() {
                Err(TryRecvError::Empty) => Phase::CacheRefreshing(refreshing),
                Err(TryRecvError::Disconnected) => {
                    append_warning(
                        &mut state.common.warning,
                        "サイドカー状態の更新workerが停止しました".into(),
                    );
                    if let Some(warning) =
                        self.disable_sidecar_restore_cache_owner(&state.common.folder)
                    {
                        append_warning(&mut state.common.warning, warning);
                    }
                    if matches!(refreshing.mode, CacheRefreshMode::RecoverUnknownCommit)
                        && state.common.families.edits
                    {
                        state.common.effects.comic_changed = true;
                    }
                    Phase::Resuming
                }
                Ok(result) => {
                    self.apply_sidecar_cache_refresh(&mut state.common, result);
                    Phase::Resuming
                }
            },
            Phase::InvalidatingPreview(invalidating) => {
                match poll_preview_clear_completion(&invalidating.rx) {
                    PreviewClearPoll::Pending => Phase::InvalidatingPreview(invalidating),
                    PreviewClearPoll::Cleared => {
                        // The service publishes Cleared before this ACK. Drain it now so no
                        // continuation thumbnail can be installed and then evicted next frame.
                        self.poll_edit_preview_cache(ctx);
                        self.sidecar_restore_start_running(&mut state, invalidating.action)
                    }
                    PreviewClearPoll::Failed(error) => {
                        append_warning(&mut state.common.warning, error);
                        self.sidecar_restore_start_cache_refresh(&mut state)
                    }
                }
            }
            Phase::Resuming => Phase::Resuming,
        };
        let keep = !matches!(state.phase, Phase::Resuming);

        if keep {
            self.sidecar_restore = Some(state);
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
            return;
        }

        // A family transaction may already be committed when the other family reports
        // SourceChanged/Cancelled. Preserve those concrete effects across the one-shot re-probe
        // and apply them before the DB-hydration continuation even when no snapshot is installable.
        let effects = std::mem::take(&mut state.common.effects);
        debug_assert!(self.install_sidecar_restore_result(None, effects).is_none());
        if let Some(warning) = state.common.warning.take() {
            crate::logger::log(format!("sidecar restore terminal warning: {warning}"));
            self.show_feedback_toast(format!("サイドカー復元: {warning}"));
        }
        let deferred = state.common.deferred_fullscreen.take();
        let favorite_failures = std::mem::take(&mut state.common.favorite_failures);
        let continuation = state.common.continuation.take_live();
        self.resume_metadata_transfer_context_readers();
        if let Some(continuation) = continuation {
            self.resume_loading_items_after_sidecar(continuation);
        }
        for command in favorite_failures {
            self.restore_failed_favorite_command(command);
        }
        if let Some(deferred) = deferred
            && self
                .items
                .get(deferred.idx)
                .is_some_and(|item| item.perf_key() == deferred.item_key)
        {
            self.open_fullscreen_with_materialization_and_contract(
                deferred.idx,
                deferred.trigger,
                deferred.requested_materialization,
                deferred.load_contract,
                None,
            );
        }
        ctx.request_repaint();
    }

    /// Finish an orphaned restore without ever applying its snapshot or continuation to another
    /// viewer generation.  Quiescing owns no moved sidecar cache values, while later phases keep
    /// their App-global worker/owner pair alive until its terminal result has been observed.
    fn poll_discarded_sidecar_restore(
        &mut self,
        ctx: &egui::Context,
        mut state: SidecarRestoreState,
    ) {
        let phase = std::mem::replace(&mut state.phase, Phase::Resuming);
        let terminal = match phase {
            Phase::Quiescing(_) | Phase::Resuming => true,
            Phase::Checking(mut checking) => match poll_checking_worker(&mut checking) {
                CheckingPoll::Pending => {
                    state.phase = Phase::Checking(checking);
                    false
                }
                CheckingPoll::Complete(Err(error)) => {
                    let reservations = checking
                        .reservations
                        .take()
                        .expect("checking owns flush reservations");
                    let error = reservations
                        .resolve_in_place(
                            &mut self.sidecars,
                            Err(format!(
                                "sidecar restore checking worker stopped after target disposal: {error}"
                            )),
                            false,
                        )
                        .expect_err("an explicit checking error cannot resolve successfully");
                    self.report_sidecar_restore_flush_failure(error);
                    true
                }
                CheckingPoll::Complete(Ok(worker)) => {
                    let reservations = checking
                        .reservations
                        .take()
                        .expect("checking owns flush reservations");
                    if let Err(error) = reservations.resolve_in_place(
                        &mut self.sidecars,
                        worker.flush,
                        state.common.clear_sidecar_cache_after_flush,
                    ) {
                        self.report_sidecar_restore_flush_failure(error);
                        true
                    } else {
                        match worker.probe {
                            Some(crate::sidecar_import::SidecarImportProbe::Current {
                                sidecar,
                                ..
                            }) => {
                                if let Some(warning) =
                                    self.install_sidecar_restore_cache_owner(sidecar)
                                {
                                    append_warning(&mut state.common.warning, warning);
                                }
                                true
                            }
                            Some(_) | None => {
                                state.phase = self.sidecar_restore_start_cache_refresh(&mut state);
                                false
                            }
                        }
                    }
                }
            },
            Phase::Running(running) => match running.rx.try_recv() {
                Err(TryRecvError::Empty) => {
                    state.phase = Phase::Running(running);
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    append_warning(
                        &mut state.common.warning,
                        "サイドカー復元workerの取消完了を確認できませんでした".into(),
                    );
                    state.phase = self.sidecar_restore_start_unknown_commit_recovery(&mut state);
                    false
                }
                Ok(result) => {
                    let result = restore_terminal(result);
                    state.common.effects.merge(result.effects);
                    // A Current snapshot is the folder-global sidecar cache owner even when its
                    // viewer continuation disappeared.  Keep it so a later ordinary save cannot
                    // rewrite the just-committed disk/marker state from the pre-import owner.
                    if let Some(sidecar) = result.sidecar {
                        if let Some(warning) = self.install_sidecar_restore_cache_owner(sidecar) {
                            append_warning(&mut state.common.warning, warning);
                        }
                        if let Some(warning) = result.warning {
                            append_warning(&mut state.common.warning, warning);
                        }
                        if let Some(error) = result.source_changed {
                            append_warning(&mut state.common.warning, error);
                        }
                        true
                    } else {
                        if let Some(warning) = result.warning {
                            append_warning(&mut state.common.warning, warning);
                        }
                        if let Some(error) = result.source_changed {
                            append_warning(&mut state.common.warning, error);
                        }
                        state.phase = self.sidecar_restore_start_cache_refresh(&mut state);
                        false
                    }
                }
            },
            Phase::CacheRefreshing(refreshing) => match refreshing.rx.try_recv() {
                Err(TryRecvError::Empty) => {
                    state.phase = Phase::CacheRefreshing(refreshing);
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    append_warning(
                        &mut state.common.warning,
                        "サイドカー状態の更新workerが停止しました".into(),
                    );
                    if let Some(warning) =
                        self.disable_sidecar_restore_cache_owner(&state.common.folder)
                    {
                        append_warning(&mut state.common.warning, warning);
                    }
                    if matches!(refreshing.mode, CacheRefreshMode::RecoverUnknownCommit)
                        && state.common.families.edits
                    {
                        state.common.effects.comic_changed = true;
                    }
                    true
                }
                Ok(result) => {
                    self.apply_sidecar_cache_refresh(&mut state.common, result);
                    true
                }
            },
            Phase::InvalidatingPreview(invalidating) => {
                match poll_preview_clear_completion(&invalidating.rx) {
                    PreviewClearPoll::Pending => {
                        state.phase = Phase::InvalidatingPreview(invalidating);
                        false
                    }
                    PreviewClearPoll::Cleared => {
                        self.poll_edit_preview_cache(ctx);
                        state.phase = self.sidecar_restore_start_cache_refresh(&mut state);
                        false
                    }
                    PreviewClearPoll::Failed(error) => {
                        append_warning(&mut state.common.warning, error);
                        state.phase = self.sidecar_restore_start_cache_refresh(&mut state);
                        false
                    }
                }
            }
        };

        if !terminal {
            self.sidecar_restore = Some(state);
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
            return;
        }

        // DB transactions are folder-global even if their viewer disappears before the worker
        // result is consumed.  Invalidate only the process-global consumers here; target grid/tag
        // hydration and smart-folder work belong to the discarded continuation and must not leak
        // into whichever sibling context happens to be mounted now.
        let effects = std::mem::take(&mut state.common.effects);
        self.apply_sidecar_restore_global_effects(effects);
        self.resume_metadata_transfer_context_readers();
        for command in std::mem::take(&mut state.common.favorite_failures) {
            self.restore_failed_favorite_command(command);
        }
        if let Some(warning) = state.common.warning.take() {
            crate::logger::log(format!(
                "sidecar restore discarded target terminal warning: {warning}"
            ));
        }
        ctx.request_repaint();
    }

    fn install_sidecar_restore_result(
        &mut self,
        sidecar: Option<crate::sidecar::SidecarFile>,
        mut effects: RestoreEffects,
    ) -> Option<String> {
        let warning = sidecar.and_then(|sidecar| self.install_sidecar_restore_cache_owner(sidecar));
        let live_effects = RestoreEffects {
            edits_changed: effects.edits_changed,
            tags_changed: effects.tags_changed,
            comic_changed: false,
            edit_delta: crate::sidecar_import::EditImportDelta::default(),
            tag_delta: std::mem::take(&mut effects.tag_delta),
            edit_rollup: None,
            tag_cache: effects.tag_cache.take(),
        };
        self.apply_sidecar_restore_global_effects(effects);
        self.apply_sidecar_restore_live_context_effects(live_effects);
        warning
    }

    fn apply_sidecar_restore_global_effects(&mut self, mut effects: RestoreEffects) {
        if effects.tags_changed {
            // Tag catalog, facet counts, picker suggestions, and native-overlay conversions are
            // process-global. Invalidate them even when the target viewer disappeared after the
            // transaction committed; target-local tag hydration stays in the live continuation.
            self.invalidate_tag_apply_suggestions();
        }
        if effects.comic_changed {
            self.comic_docs.clear();
        }
        if let Some(snapshot) = effects.edit_rollup.take() {
            self.adjusted_page_keys = snapshot.adjusted;
            self.local_adjust_page_keys = snapshot.local_adjusted;
            self.mask_page_keys = snapshot.masked;
            self.conceal_page_keys = snapshot.concealed;
            self.comic_page_keys = snapshot.comic;
            self.rotation_page_keys = snapshot.rotated;
            self.export_crop_page_keys = snapshot.cropped;
        }
        self.adjusted_page_keys.extend(effects.edit_delta.adjusted);
        self.mask_page_keys.extend(effects.edit_delta.masked);
        self.conceal_page_keys.extend(effects.edit_delta.concealed);
        self.local_adjust_page_keys
            .extend(effects.edit_delta.local_adjusted);
        self.export_crop_page_keys
            .extend(effects.edit_delta.cropped);
        self.comic_page_keys.extend(effects.edit_delta.comic);
    }

    fn apply_sidecar_restore_live_context_effects(&mut self, mut effects: RestoreEffects) {
        let rebuild_visible = effects.edits_changed || effects.tags_changed;
        if effects.edits_changed {
            self.schedule_current_smart_folder_metadata_refresh(
                smart_folder::SmartFolderMetadataDependency::Edits,
            );
        }
        if effects.tags_changed {
            if let Some(tags) = effects.tag_cache.take() {
                self.replace_tags_cache(tags);
            }
            for (item_key, tags) in effects.tag_delta {
                self.set_tags_cache_entry(item_key, tags);
            }
            self.schedule_current_smart_folder_metadata_refresh(
                smart_folder::SmartFolderMetadataDependency::Tags,
            );
        }
        if rebuild_visible {
            // The facet result was built before recovery. Re-evaluate it from the imported
            // in-memory keys before the first-display continuation restores selection/scroll.
            self.rebuild_visible_indices_preserving_facet_scope();
        }
    }

    fn restore_failed_favorite_command(
        &mut self,
        command: crate::favorite_view_state::FavoriteViewStoreCommand,
    ) {
        match command {
            crate::favorite_view_state::FavoriteViewStoreCommand::Set { id, state } => {
                if self.favorite_view_states.get(&id) == Some(&state) {
                    self.favorite_view_writes.defer_command_at(
                        crate::favorite_view_state::FavoriteViewStoreCommand::Set { id, state },
                        std::time::Instant::now(),
                    );
                }
            }
            command @ crate::favorite_view_state::FavoriteViewStoreCommand::Remove { id } => {
                if !self.favorite_view_states.contains_key(&id) {
                    self.favorite_view_writes
                        .defer_command_at(command, std::time::Instant::now());
                }
            }
            command @ crate::favorite_view_state::FavoriteViewStoreCommand::Clear => {
                if self.favorite_view_states.is_empty() {
                    self.favorite_view_writes
                        .defer_command_at(command, std::time::Instant::now());
                }
            }
        }
    }
}

fn load_current_sidecar_cache(
    folder: &Path,
    families: crate::sidecar_import::ImportFamilies,
) -> CacheRefreshResult {
    let disabled = |error: String| CacheRefreshResult::Disabled {
        sidecar: crate::sidecar::SidecarFile::disabled_placeholder(folder.to_path_buf()),
        error,
    };
    match crate::sidecar::SidecarFile::load_for_import(folder) {
        crate::sidecar::SidecarImportLoad::Loaded(loaded) => {
            // Cache recovery is a self-contained cleanup operation.  It must outlive a target
            // continuation cancellation so an orphaned restore still resolves the folder-global
            // owner instead of unnecessarily disabling a stable sidecar.
            let cleanup_cancel = AtomicBool::new(false);
            match crate::sidecar_import::validate_cache_snapshot(loaded, families, &cleanup_cancel)
            {
                crate::sidecar_import::SidecarCacheValidation::Current(sidecar) => {
                    CacheRefreshResult::Current(sidecar)
                }
                crate::sidecar_import::SidecarCacheValidation::ValidationFailed {
                    sidecar,
                    error,
                } => CacheRefreshResult::Disabled { sidecar, error },
                crate::sidecar_import::SidecarCacheValidation::SourceChanged(error) => disabled(
                    format!("サイドカー状態の更新中にsourceが変わりました: {error}"),
                ),
                crate::sidecar_import::SidecarCacheValidation::Cancelled => {
                    disabled("サイドカー状態の更新が取り消されました".to_string())
                }
                crate::sidecar_import::SidecarCacheValidation::Failed(error) => disabled(format!(
                    "サイドカー状態の検証を完了できませんでした: {error}"
                )),
            }
        }
        crate::sidecar::SidecarImportLoad::Missing { sidecar } => {
            match crate::sidecar::revalidate_missing_import_source(folder) {
                Ok(()) => CacheRefreshResult::Current(sidecar),
                Err(error) => disabled(format!(
                    "サイドカー状態の更新中にmissing sourceが変わりました: {error}"
                )),
            }
        }
        crate::sidecar::SidecarImportLoad::Unreadable { error, .. }
        | crate::sidecar::SidecarImportLoad::Corrupt { error, .. } => {
            disabled(format!("サイドカー状態を読み直せませんでした: {error}"))
        }
        crate::sidecar::SidecarImportLoad::UnsupportedVersion { version, .. } => disabled(format!(
            "サイドカーはこの版より新しい形式です (version={version})"
        )),
        crate::sidecar::SidecarImportLoad::WriterFailed { .. } => {
            disabled("サイドカーwriterが以前の保存に失敗しています".to_string())
        }
        crate::sidecar::SidecarImportLoad::ChangedDuringRead { .. } => {
            disabled("サイドカー状態の読取中にsourceが変わりました".to_string())
        }
    }
}

fn probe_result_has_failure(result: &crate::sidecar_import::SidecarProbeResult) -> bool {
    result.source_validation_error.is_some()
        || matches!(
            result.edits,
            crate::sidecar_import::SidecarProbeFamilyOutcome::Failed(_)
        )
        || matches!(
            result.tags,
            crate::sidecar_import::SidecarProbeFamilyOutcome::Failed(_)
        )
}

fn probe_requires_preview_clear(result: &crate::sidecar_import::SidecarProbeResult) -> bool {
    matches!(
        result.edits,
        crate::sidecar_import::SidecarProbeFamilyOutcome::ImportRequired
    )
}

fn sidecar_restore_retains_egui_event(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Key { pressed: false, .. }
            | egui::Event::PointerButton { pressed: false, .. }
            | egui::Event::PointerGone
            | egui::Event::Touch {
                phase: egui::TouchPhase::End | egui::TouchPhase::Cancel,
                ..
            }
            | egui::Event::Ime(egui::ImeEvent::Disabled)
            | egui::Event::Screenshot { .. }
    )
}

fn probe_result_warning(result: &crate::sidecar_import::SidecarProbeResult) -> String {
    if let Some(error) = &result.source_validation_error {
        return error.clone();
    }
    [&result.edits, &result.tags]
        .into_iter()
        .filter_map(|outcome| match outcome {
            crate::sidecar_import::SidecarProbeFamilyOutcome::Failed(error) => Some(error.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn load_edit_rollup_snapshot(data_dir: &Path) -> Result<EditRollupSnapshot, String> {
    crate::metadata_transfer::load_import_page_state_snapshot(data_dir)
        .map_err(|error| error.to_string())
}

fn load_tag_cache_snapshot(
    data_dir: &Path,
    item_keys: &[String],
) -> Result<std::collections::HashMap<String, Vec<String>>, String> {
    let connection = rusqlite::Connection::open_with_flags(
        data_dir.join("tags.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| error.to_string())?;
    let mut loaded = std::collections::HashMap::<String, Vec<String>>::new();
    for chunk in item_keys.chunks(500) {
        let placeholders = (0..chunk.len())
            .map(|idx| format!("?{}", idx + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT item_key, tag FROM item_tags WHERE item_key IN ({placeholders}) \
             ORDER BY item_key ASC, applied_at ASC, tag COLLATE NOCASE ASC"
        );
        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let params = chunk
            .iter()
            .map(|key| key as &dyn rusqlite::ToSql)
            .collect::<Vec<_>>();
        let rows = statement
            .query_map(rusqlite::params_from_iter(params), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?;
        for row in rows {
            let (item_key, tag) = row.map_err(|error| error.to_string())?;
            loaded
                .entry(item_key)
                .or_default()
                .push(crate::tags_db::format_display_tag(&tag));
        }
    }
    for key in item_keys {
        loaded.entry(key.clone()).or_default();
    }
    Ok(loaded)
}

fn load_recovered_db_state(
    data_dir: &Path,
    families: crate::sidecar_import::ImportFamilies,
    item_keys: &[String],
) -> Result<RecoveredDbState, String> {
    const ATTEMPTS: usize = 3;
    let mut last_error = String::new();
    for attempt in 0..ATTEMPTS {
        match (
            families
                .edits
                .then(|| load_edit_rollup_snapshot(data_dir))
                .transpose(),
            families
                .tags
                .then(|| load_tag_cache_snapshot(data_dir, item_keys))
                .transpose(),
        ) {
            (Ok(edit_rollup), Ok(tag_cache)) => {
                return Ok(RecoveredDbState {
                    edit_rollup,
                    tag_cache,
                });
            }
            (edit, tags) => {
                let edit = edit.err().map(|error| format!("edit index: {error}"));
                let tags = tags.err().map(|error| format!("tag cache: {error}"));
                last_error = [edit, tags]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("; ");
            }
        }
        if attempt + 1 < ATTEMPTS {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
    Err(format!(
        "復元workerの結果が不明で、中央DB状態を{ATTEMPTS}回読み直しても確認できませんでした: {last_error}"
    ))
}

fn restore_terminal(result: RunningWorkerResult) -> RestoreTerminal {
    match result {
        RunningWorkerResult::Import(Ok(
            crate::sidecar_import::SidecarImportCompletion::Current { sidecar, result },
        )) => {
            let warning = import_result_warning(&result);
            RestoreTerminal {
                effects: import_result_effects(result),
                warning,
                sidecar: Some(sidecar),
                source_changed: None,
            }
        }
        RunningWorkerResult::Import(Ok(
            crate::sidecar_import::SidecarImportCompletion::SourceChanged { error, result },
        )) => {
            let warning = import_result_warning(&result);
            RestoreTerminal {
                effects: import_result_effects(result),
                warning,
                sidecar: None,
                source_changed: Some(error),
            }
        }
        RunningWorkerResult::Import(Ok(
            crate::sidecar_import::SidecarImportCompletion::Cancelled { result },
        )) => RestoreTerminal {
            effects: import_result_effects(result),
            warning: Some("サイドカー復元が取り消されました".into()),
            sidecar: None,
            source_changed: None,
        },
        RunningWorkerResult::Import(Err(error)) => RestoreTerminal {
            sidecar: None,
            effects: RestoreEffects::default(),
            source_changed: None,
            warning: Some(error),
        },
        RunningWorkerResult::MarkerClear(
            crate::sidecar_import::MissingMarkerClearCompletion::Current { sidecar, result },
        ) => RestoreTerminal {
            sidecar: Some(sidecar),
            effects: RestoreEffects::default(),
            source_changed: None,
            warning: marker_clear_warning(&result),
        },
        RunningWorkerResult::MarkerClear(
            crate::sidecar_import::MissingMarkerClearCompletion::SourceChanged { error, result },
        ) => RestoreTerminal {
            sidecar: None,
            effects: RestoreEffects::default(),
            source_changed: Some(error),
            warning: marker_clear_warning(&result),
        },
        RunningWorkerResult::MarkerClear(
            crate::sidecar_import::MissingMarkerClearCompletion::Cancelled { .. },
        ) => RestoreTerminal {
            sidecar: None,
            effects: RestoreEffects::default(),
            source_changed: None,
            warning: Some("サイドカーmarker更新が取り消されました".into()),
        },
        RunningWorkerResult::SourceChanged(error) => RestoreTerminal {
            sidecar: None,
            effects: RestoreEffects::default(),
            source_changed: Some(error),
            warning: None,
        },
    }
}

fn import_result_effects(result: crate::sidecar_import::SidecarImportResult) -> RestoreEffects {
    let mut effects = RestoreEffects::default();
    if let crate::sidecar_import::ImportFamilyOutcome::Applied(report) = result.edits {
        effects.edits_changed = report.stats.imported_adjust > 0
            || report.stats.imported_mask > 0
            || report.stats.imported_conceal > 0
            || report.stats.imported_local_adjust > 0
            || report.stats.imported_export_crop > 0
            || report.stats.imported_comic > 0;
        effects.comic_changed = report.stats.imported_comic > 0;
        effects.edit_delta = report.delta;
    }
    if let crate::sidecar_import::ImportFamilyOutcome::Applied(report) = result.tags {
        effects.tags_changed = report.imported_items > 0;
        effects.tag_delta = report.imported;
    }
    effects
}

fn append_warning(target: &mut Option<String>, warning: String) {
    match target {
        Some(existing) if !existing.contains(&warning) => {
            existing.push_str("; ");
            existing.push_str(&warning);
        }
        None => *target = Some(warning),
        _ => {}
    }
}

fn import_result_warning(result: &crate::sidecar_import::SidecarImportResult) -> Option<String> {
    if let Some(error) = &result.source_validation_error {
        return Some(error.clone());
    }
    let warning = [
        import_family_warning(&result.edits),
        import_family_warning(&result.tags),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("; ");
    (!warning.is_empty()).then_some(warning)
}

fn import_family_warning<T>(
    outcome: &crate::sidecar_import::ImportFamilyOutcome<T>,
) -> Option<&str> {
    match outcome {
        crate::sidecar_import::ImportFamilyOutcome::Failed(error)
        | crate::sidecar_import::ImportFamilyOutcome::SourceChanged(error) => Some(error.as_str()),
        _ => None,
    }
}

fn marker_clear_warning(
    result: &crate::sidecar_import::MissingMarkerClearResult,
) -> Option<String> {
    let warning = [&result.edits, &result.tags]
        .into_iter()
        .filter_map(|outcome| match outcome {
            crate::sidecar_import::ImportFamilyOutcome::Failed(error)
            | crate::sidecar_import::ImportFamilyOutcome::SourceChanged(error) => {
                Some(error.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("; ");
    (!warning.is_empty()).then_some(warning)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_edit_rollup() -> EditRollupSnapshot {
        EditRollupSnapshot {
            adjusted: Default::default(),
            local_adjusted: Default::default(),
            masked: Default::default(),
            concealed: Default::default(),
            comic: Default::default(),
            rotated: Default::default(),
            cropped: Default::default(),
        }
    }

    fn discarded_restore_state(folder: PathBuf) -> SidecarRestoreState {
        SidecarRestoreState {
            common: Common {
                request_id: 1,
                started_at: std::time::Instant::now(),
                target_context: ViewerContextId::for_test(1),
                items_generation: 1,
                folder: folder.clone(),
                data_dir: folder.clone(),
                families: crate::sidecar_import::ImportFamilies {
                    edits: true,
                    tags: true,
                },
                tag_item_keys: Vec::new(),
                source_path: folder,
                continuation: ContinuationOwner::Discarded {
                    cancel: Arc::new(AtomicBool::new(false)),
                },
                deferred_fullscreen: None,
                favorite_failures: Vec::new(),
                warning: None,
                source_change_reprobe: false,
                clear_sidecar_cache_after_flush: false,
                effects: RestoreEffects::default(),
            },
            phase: Phase::Resuming,
        }
    }

    fn probe_result(
        edits: crate::sidecar_import::SidecarProbeFamilyOutcome,
        tags: crate::sidecar_import::SidecarProbeFamilyOutcome,
    ) -> crate::sidecar_import::SidecarProbeResult {
        crate::sidecar_import::SidecarProbeResult {
            edits,
            tags,
            source_validation_error: None,
            load_elapsed: std::time::Duration::ZERO,
            prepare_elapsed: std::time::Duration::ZERO,
            probe_elapsed: std::time::Duration::ZERO,
        }
    }

    #[test]
    fn product_settings_keep_sidecar_restore_enabled_by_default() {
        let settings = crate::settings::Settings::default();

        assert!(settings.sidecar_backup_enabled);
        assert!(!settings.tag_sidecar_backup_enabled);
    }

    #[test]
    fn modal_grace_delays_only_presentation_while_the_target_gate_is_active() {
        let mut app = crate::app::setup_app_for_test();
        let folder = app.tmp.path().join("book");
        app.activate_sidecar_restore_modal_for_test(folder);
        let started_at = app.sidecar_restore.as_ref().unwrap().common.started_at;

        assert!(app.sidecar_restore_blocks_projected_context());
        assert_eq!(
            app.sidecar_restore
                .as_ref()
                .unwrap()
                .modal_delay_remaining(started_at),
            SIDECAR_RESTORE_MODAL_GRACE
        );
        assert_eq!(
            app.sidecar_restore
                .as_ref()
                .unwrap()
                .modal_delay_remaining(started_at + SIDECAR_RESTORE_MODAL_GRACE),
            std::time::Duration::ZERO
        );

        app.sidecar_restore.as_mut().unwrap().common.started_at =
            std::time::Instant::now() + SIDECAR_RESTORE_MODAL_GRACE;
        let early = egui::Context::default().run(egui::RawInput::default(), |ctx| {
            app.show_sidecar_restore_dialog(ctx);
        });
        assert!(
            early.shapes.is_empty(),
            "presentation alone must stay hidden during the grace"
        );

        app.sidecar_restore.as_mut().unwrap().common.started_at =
            std::time::Instant::now() - SIDECAR_RESTORE_MODAL_GRACE;
        let visible = egui::Context::default().run(egui::RawInput::default(), |ctx| {
            app.show_sidecar_restore_dialog(ctx);
        });
        assert!(
            !visible.shapes.is_empty(),
            "an active restore must draw its progress modal after the grace"
        );
        assert!(app.sidecar_restore_blocks_projected_context());
    }

    #[test]
    fn fast_current_terminal_finishes_inside_the_modal_grace_and_runs_the_tail_once() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.sidecar_backup_enabled = true;
        app.settings.tag_sidecar_backup_enabled = false;
        let folder = app.tmp.path().join("book");
        std::fs::create_dir_all(&folder).unwrap();
        app.current_folder = Some(folder.clone());
        let (thumb_tx, _thumb_rx) = mpsc::channel::<ThumbMsg>();
        let started_at = std::time::Instant::now();
        let continuation = SidecarLoadContinuation {
            source_path: folder.clone(),
            source_is_directory: true,
            prepared_subfolder: None,
            prepared_aggregate: None,
            catalog_existing_keys: HashSet::new(),
            video_items: Vec::new(),
            sli_seq: 0,
            sli_t0: started_at,
            items_len: 0,
            detached_physical: false,
            tx: thumb_tx,
            cancel: Arc::new(AtomicBool::new(false)),
            restore_started_at: started_at,
        };
        assert!(app.begin_sidecar_restore(continuation, false).is_ok());
        assert!(app.sidecar_restore_blocks_projected_context());
        assert_eq!(
            app.sidecar_restore
                .as_ref()
                .unwrap()
                .modal_delay_remaining(started_at + std::time::Duration::from_millis(99)),
            std::time::Duration::from_millis(1),
            "a fast Missing/no-marker or synchronized Current terminal must not flash the modal"
        );

        // Current is the terminal used by both Missing/no-marker and synchronized probes.
        // Put that completed probe at the Resuming boundary without bypassing begin's typed owner.
        app.sidecar_restore.as_mut().unwrap().phase = Phase::Resuming;
        let ctx = egui::Context::default();
        app.poll_sidecar_restore(&ctx);
        assert!(app.sidecar_restore.is_none());
        assert_eq!(
            app.settings.last_folder.as_ref(),
            Some(&folder),
            "the owned load tail must run at the fast Current terminal"
        );

        let sentinel = app.tmp.path().join("second-poll-sentinel");
        app.settings.last_folder = Some(sentinel.clone());
        app.poll_sidecar_restore(&ctx);
        assert_eq!(app.settings.last_folder.as_ref(), Some(&sentinel));
    }

    #[test]
    fn preview_clear_is_required_only_before_an_edit_import() {
        use crate::sidecar_import::SidecarProbeFamilyOutcome as Outcome;

        assert!(probe_requires_preview_clear(&probe_result(
            Outcome::ImportRequired,
            Outcome::AlreadySynchronized,
        )));
        assert!(!probe_requires_preview_clear(&probe_result(
            Outcome::AlreadySynchronized,
            Outcome::ImportRequired,
        )));
        assert!(!probe_requires_preview_clear(&probe_result(
            Outcome::MarkerClearRequired,
            Outcome::AlreadySynchronized,
        )));
    }

    #[test]
    fn sidecar_folder_uses_captured_source_kind_instead_of_extension() {
        assert_eq!(
            App::sidecar_restore_folder(Path::new("C:/books/photos.zip"), true),
            PathBuf::from("C:/books/photos.zip")
        );
        for container in ["book.zip", "book.pdf", "book.rar"] {
            assert_eq!(
                App::sidecar_restore_folder(Path::new("C:/books").join(container).as_path(), false),
                PathBuf::from("C:/books")
            );
        }
    }

    #[test]
    fn quiescence_barrier_opens_only_after_every_owner_is_settled() {
        assert!(!quiescence_barrier_ready(false, true, false, false));
        assert!(!quiescence_barrier_ready(true, false, false, false));
        assert!(!quiescence_barrier_ready(true, true, true, false));
        assert!(!quiescence_barrier_ready(true, true, false, true));
        assert!(quiescence_barrier_ready(true, true, false, false));
    }

    #[test]
    fn preview_clear_completion_cannot_start_import_before_success_ack() {
        let (tx, rx) = mpsc::channel();
        assert_eq!(
            poll_preview_clear_completion(&rx),
            PreviewClearPoll::Pending
        );

        tx.send(Ok(())).unwrap();
        assert_eq!(
            poll_preview_clear_completion(&rx),
            PreviewClearPoll::Cleared
        );

        let (tx, rx) = mpsc::channel();
        tx.send(Err("delete failed".to_string())).unwrap();
        assert_eq!(
            poll_preview_clear_completion(&rx),
            PreviewClearPoll::Failed("delete failed".to_string())
        );
    }

    #[test]
    fn checking_result_waits_for_finished_thread_before_joining_on_ui_poll() {
        let (result_tx, result_rx) = mpsc::channel();
        let (sent_tx, sent_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            result_tx
                .send(CheckingWorkerResult {
                    flush: Err("synthetic flush result".to_string()),
                    probe: None,
                })
                .unwrap();
            sent_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        sent_rx.recv().unwrap();
        let mut checking = Checking {
            reservations: None,
            rx: result_rx,
            handle: Some(handle),
            terminal: None,
        };

        assert!(matches!(
            poll_checking_worker(&mut checking),
            CheckingPoll::Pending
        ));
        assert!(checking.handle.is_some());
        assert!(checking.terminal.is_some());

        release_tx.send(()).unwrap();
        while !checking.handle.as_ref().unwrap().is_finished() {
            std::thread::yield_now();
        }
        match poll_checking_worker(&mut checking) {
            CheckingPoll::Complete(Ok(result)) => {
                assert_eq!(result.flush.unwrap_err(), "synthetic flush result")
            }
            _ => panic!("finished worker must be joined and resolved exactly once"),
        }
        assert!(checking.handle.is_none());
        assert!(checking.terminal.is_none());
    }

    #[test]
    fn semantic_input_is_discarded_but_release_edges_are_retained() {
        let key = |pressed| egui::Event::Key {
            key: egui::Key::A,
            physical_key: Some(egui::Key::A),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let button = |pressed| egui::Event::PointerButton {
            pos: egui::pos2(1.0, 2.0),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };

        assert!(!sidecar_restore_retains_egui_event(&key(true)));
        assert!(sidecar_restore_retains_egui_event(&key(false)));
        assert!(!sidecar_restore_retains_egui_event(&button(true)));
        assert!(sidecar_restore_retains_egui_event(&button(false)));
        assert!(!sidecar_restore_retains_egui_event(&egui::Event::Copy));
        assert!(!sidecar_restore_retains_egui_event(&egui::Event::Paste(
            "ignored".into()
        )));
        assert!(!sidecar_restore_retains_egui_event(
            &egui::Event::WindowFocused(true)
        ));
        assert!(sidecar_restore_retains_egui_event(
            &egui::Event::PointerGone
        ));
        assert!(!sidecar_restore_retains_egui_event(&egui::Event::Ime(
            egui::ImeEvent::Enabled
        )));
        assert!(!sidecar_restore_retains_egui_event(&egui::Event::Ime(
            egui::ImeEvent::Preedit("未確定".into())
        )));
        assert!(!sidecar_restore_retains_egui_event(&egui::Event::Ime(
            egui::ImeEvent::Commit("入力しない".into())
        )));
        assert!(sidecar_restore_retains_egui_event(&egui::Event::Ime(
            egui::ImeEvent::Disabled
        )));
    }

    #[test]
    fn restore_modal_and_semantic_gate_belong_only_to_the_target_context() {
        let mut app = crate::app::setup_app_for_test();
        let folder = app.tmp.path().join("book");
        app.sidecar_restore = Some(discarded_restore_state(folder.clone()));
        assert!(
            !app.sidecar_restore_blocks_projected_context(),
            "a sibling projection must not inherit the target modal"
        );

        app.activate_sidecar_restore_modal_for_test(folder);
        assert!(app.sidecar_restore_blocks_projected_context());
    }

    #[cfg(windows)]
    #[test]
    fn native_pointer_hold_survives_the_target_restore_input_sanitize() {
        let mut app = crate::app::setup_app_for_test();
        let folder = app.tmp.path().join("book");
        app.activate_sidecar_restore_modal_for_test(folder);
        app.native_video_pointer_down = Some(NativeVideoPointerDown::PlaybackClick {
            fs_idx: 7,
            x: 10,
            y: 20,
            at: std::time::Instant::now(),
        });

        assert!(app.sidecar_restore_blocks_projected_context());
        app.consume_input_during_sidecar_restore(&egui::Context::default());

        assert!(matches!(
            app.native_video_pointer_down,
            Some(NativeVideoPointerDown::PlaybackClick {
                fs_idx: 7,
                x: 10,
                y: 20,
                ..
            })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn passive_modal_blocks_only_the_window_bound_to_the_restore_target() {
        let mut app = crate::app::setup_app_for_test();
        let target = app.build_window_context_for_test(701, |_viewer| {});
        let _sibling = app.build_window_context_for_test(702, |_viewer| {});
        let mut state = discarded_restore_state(app.tmp.path().join("book"));
        state.common.target_context = target;
        app.sidecar_restore = Some(state);

        assert!(app.sidecar_restore_blocks_window(701));
        assert!(!app.sidecar_restore_blocks_window(702));
    }

    #[test]
    fn edit_import_does_not_start_when_preview_db_service_is_unavailable() {
        let mut app = crate::app::setup_app_for_test();
        app.edit_preview_cache = None;
        let folder = app.tmp.path().join("book");
        std::fs::create_dir_all(&folder).unwrap();
        let mut state = discarded_restore_state(folder);
        state.common.data_dir = app.tmp.path().to_path_buf();
        state.common.families.tags = false;

        let phase = app.sidecar_restore_start_preview_clear(
            &mut state,
            RequiredAction::Import {
                clear_preview: true,
            },
        );

        assert!(matches!(phase, Phase::CacheRefreshing(_)));
        assert!(
            state
                .common
                .warning
                .as_deref()
                .is_some_and(|warning| warning.contains("復元を開始しませんでした"))
        );
    }

    #[test]
    fn viewport_gate_clears_already_derived_pointer_and_scroll_input() {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.events = vec![
            egui::Event::PointerMoved(egui::pos2(10.0, 20.0)),
            egui::Event::PointerButton {
                pos: egui::pos2(10.0, 20.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, 24.0),
                modifiers: egui::Modifiers::CTRL,
            },
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Key {
                key: egui::Key::B,
                physical_key: Some(egui::Key::B),
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];

        let _ = ctx.run(raw, |ctx| {
            assert!(ctx.input(|input| input.pointer.primary_down()));
            assert_ne!(ctx.input(|input| input.raw_scroll_delta), egui::Vec2::ZERO);
            App::consume_sidecar_restore_viewport_input(ctx);

            ctx.input(|input| {
                assert!(!input.pointer.any_down());
                assert!(input.pointer.latest_pos().is_none());
                assert_eq!(input.raw_scroll_delta, egui::Vec2::ZERO);
                assert_eq!(input.smooth_scroll_delta, egui::Vec2::ZERO);
                assert!(input.keys_down.is_empty());
                assert_eq!(input.events.len(), 1);
                assert!(matches!(
                    input.events[0],
                    egui::Event::Key {
                        key: egui::Key::B,
                        pressed: false,
                        ..
                    }
                ));
            });
        });
    }

    #[test]
    fn source_changed_terminal_preserves_an_already_committed_edit_effect() {
        let mut report = crate::sidecar_import::EditImportReport::default();
        report.stats.imported_mask = 1;
        let result = crate::sidecar_import::SidecarImportResult {
            edits: crate::sidecar_import::ImportFamilyOutcome::Applied(report),
            tags: crate::sidecar_import::ImportFamilyOutcome::SourceChanged("changed".into()),
            source_validation_error: None,
            prepare_elapsed: std::time::Duration::ZERO,
            commit_elapsed: std::time::Duration::ZERO,
        };
        let terminal = restore_terminal(RunningWorkerResult::Import(Ok(
            crate::sidecar_import::SidecarImportCompletion::SourceChanged {
                error: "changed".into(),
                result,
            },
        )));

        assert!(terminal.sidecar.is_none());
        assert_eq!(terminal.source_changed.as_deref(), Some("changed"));
        assert!(terminal.effects.edits_changed);
        assert!(!terminal.effects.tags_changed);
    }

    #[test]
    fn discarding_a_target_keeps_the_cancel_owner_but_drops_the_continuation() {
        let (tx, _rx) = mpsc::channel::<ThumbMsg>();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut owner = ContinuationOwner::Live(SidecarLoadContinuation {
            source_path: PathBuf::from("C:/book"),
            source_is_directory: true,
            prepared_subfolder: None,
            prepared_aggregate: None,
            catalog_existing_keys: HashSet::new(),
            video_items: Vec::new(),
            sli_seq: 0,
            sli_t0: std::time::Instant::now(),
            items_len: 0,
            detached_physical: false,
            tx,
            cancel: Arc::clone(&cancel),
            restore_started_at: std::time::Instant::now(),
        });

        assert!(owner.discard());
        assert!(cancel.load(Ordering::Relaxed));
        assert!(!owner.discard());
        assert!(owner.take_live().is_none());
    }

    #[test]
    fn deferred_fullscreen_preserves_virtual_page_open_contract() {
        let deferred = DeferredFullscreen {
            idx: 7,
            trigger: HistoryTrigger::UserChosen,
            item_key: "page-key".into(),
            requested_materialization: FsOpenMaterialization::DeferredPageTurn,
            load_contract: FsPageLoadContract::LatestSeek,
        };

        assert_eq!(
            deferred.requested_materialization,
            FsOpenMaterialization::DeferredPageTurn
        );
        assert_eq!(deferred.load_contract, FsPageLoadContract::LatestSeek);
    }

    #[test]
    fn active_restore_consumes_invalid_and_sibling_fullscreen_requests() {
        let mut app = crate::app::setup_app_for_test();
        let folder = app.tmp.path().join("book");
        app.sidecar_restore = Some(discarded_restore_state(folder.clone()));

        assert!(app.defer_sidecar_restore_fullscreen(
            usize::MAX,
            HistoryTrigger::UserChosen,
            FsOpenMaterialization::Eager,
            FsPageLoadContract::Sequential,
        ));
        assert!(
            app.sidecar_restore
                .as_ref()
                .unwrap()
                .common
                .deferred_fullscreen
                .is_none(),
            "an invalid request is consumed without preserving an intent"
        );

        app.items.push(GridItem::Image(folder.join("page.jpg")));
        assert!(app.defer_sidecar_restore_fullscreen(
            0,
            HistoryTrigger::UserChosen,
            FsOpenMaterialization::DeferredPageTurn,
            FsPageLoadContract::LatestSeek,
        ));
        assert!(
            app.sidecar_restore
                .as_ref()
                .unwrap()
                .common
                .deferred_fullscreen
                .is_none(),
            "a sibling-context request is consumed without mutating fullscreen state"
        );
        assert!(app.fullscreen_idx.is_none());
    }

    #[test]
    fn exit_recovers_dirty_sidecar_owners_from_an_inflight_check() {
        let mut app = crate::app::setup_app_for_test();
        let folder = app.tmp.path().join("book");
        let mut sidecar = crate::sidecar::SidecarFile::new(folder.clone());
        sidecar.set_tags("page.jpg", ["saved-before-restore"]);
        assert!(sidecar.is_dirty());
        app.sidecars.insert(folder.clone(), sidecar);
        let (reservations, _batch) =
            crate::sidecar::prepare_worker_flush_in_place(app.sidecars.values());
        let (tx, rx) = mpsc::channel();
        let worker_completed = Arc::new(AtomicBool::new(false));
        let worker_completed_in_thread = Arc::clone(&worker_completed);
        let (entered_tx, entered_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            entered_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
            worker_completed_in_thread.store(true, Ordering::Release);
            drop(tx);
        });
        entered_rx.recv().unwrap();
        let mut state = discarded_restore_state(folder.clone());
        state.phase = Phase::Checking(Checking {
            reservations: Some(reservations),
            rx,
            handle: Some(handle),
            terminal: None,
        });
        app.sidecar_restore = Some(state);

        app.resolve_sidecar_restore_for_exit();

        assert!(app.sidecar_restore.is_none());
        assert!(
            worker_completed.load(Ordering::Acquire),
            "the exit owner cannot be resolved before its checking worker is joined"
        );
        assert!(
            app.sidecars
                .get(&folder)
                .is_some_and(|owner| owner.is_dirty()),
            "the established exit flush must regain the exact pre-restore dirty owner"
        );
    }

    #[test]
    fn root_user_close_is_blocked_before_tray_hide_during_restore() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.minimize_to_tray_on_close = true;
        app.window_visible = true;
        let folder = app.tmp.path().join("book");
        app.activate_sidecar_restore_modal_for_test(folder);
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);

        let output = ctx.run(raw, |ctx| {
            app.consume_input_during_sidecar_restore(ctx);
            assert!(ctx.input(|input| input.viewport().close_requested()));
            let tray_hide_started = if app.sidecar_restore_blocks_root_tray_hide() {
                false
            } else {
                app.maybe_intercept_close(ctx)
            };
            assert!(!tray_hide_started);
            app.show_sidecar_restore_dialog(ctx);
        });
        assert!(app.window_visible);
        assert!(
            output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .is_some_and(|viewport| viewport
                    .commands
                    .iter()
                    .any(|command| matches!(command, egui::ViewportCommand::CancelClose))),
            "the root input boundary must reject a semantic user close"
        );
    }

    #[test]
    fn lifecycle_close_is_not_cancelled_by_the_restore_dialog() {
        let mut app = crate::app::setup_app_for_test();
        let folder = app.tmp.path().join("book");
        app.activate_sidecar_restore_modal_for_test(folder);
        app.shutdown_requested.store(true, Ordering::SeqCst);
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);

        let output = ctx.run(raw, |ctx| {
            app.consume_input_during_sidecar_restore(ctx);
            app.show_sidecar_restore_dialog(ctx);
        });

        assert!(
            output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .is_none_or(|viewport| !viewport
                    .commands
                    .iter()
                    .any(|command| matches!(command, egui::ViewportCommand::CancelClose))),
            "the dialog renderer must leave unavoidable process-exit close policy to the root input boundary"
        );
    }

    #[test]
    fn discarded_target_keeps_only_a_current_global_sidecar_snapshot() {
        let current = restore_terminal(RunningWorkerResult::Import(Ok(
            crate::sidecar_import::SidecarImportCompletion::Current {
                sidecar: crate::sidecar::SidecarFile::new(PathBuf::from("C:/book")),
                result: crate::sidecar_import::SidecarImportResult {
                    edits: crate::sidecar_import::ImportFamilyOutcome::NotRequested,
                    tags: crate::sidecar_import::ImportFamilyOutcome::NotRequested,
                    source_validation_error: None,
                    prepare_elapsed: std::time::Duration::ZERO,
                    commit_elapsed: std::time::Duration::ZERO,
                },
            },
        )));
        assert!(current.sidecar.is_some());

        let changed = restore_terminal(RunningWorkerResult::Import(Ok(
            crate::sidecar_import::SidecarImportCompletion::SourceChanged {
                error: "changed".into(),
                result: crate::sidecar_import::SidecarImportResult {
                    edits: crate::sidecar_import::ImportFamilyOutcome::SourceChanged(
                        "changed".into(),
                    ),
                    tags: crate::sidecar_import::ImportFamilyOutcome::NotRequested,
                    source_validation_error: None,
                    prepare_elapsed: std::time::Duration::ZERO,
                    commit_elapsed: std::time::Duration::ZERO,
                },
            },
        )));
        assert!(changed.sidecar.is_none());

        let cancelled = restore_terminal(RunningWorkerResult::Import(Ok(
            crate::sidecar_import::SidecarImportCompletion::Cancelled {
                result: crate::sidecar_import::SidecarImportResult {
                    edits: crate::sidecar_import::ImportFamilyOutcome::Cancelled,
                    tags: crate::sidecar_import::ImportFamilyOutcome::NotRequested,
                    source_validation_error: None,
                    prepare_elapsed: std::time::Duration::ZERO,
                    commit_elapsed: std::time::Duration::ZERO,
                },
            },
        )));
        assert!(cancelled.sidecar.is_none());
    }

    #[test]
    fn discarded_terminal_applies_global_edit_effects_without_hydrating_a_sibling() {
        let mut app = crate::app::setup_app_for_test();
        app.comic_docs.insert("stale-comic".into(), Vec::new());
        app.adjusted_page_keys.insert("stale-adjust".into());
        app.mask_page_keys.insert("stale-mask".into());
        app.tags_cache
            .insert("sibling-item".into(), vec!["keep".into()]);
        app.tag_apply_suggestion_key = Some("stale-key".into());
        app.tag_apply_suggestions
            .push(("stale".into(), "stale".into(), 1, false, 1));
        app.tag_choice_catalog_cache = Some(vec![("stale".into(), "stale".into(), 1, false, 1)]);
        app.tag_apply_selection_cache = Some(((vec![0], Some(0), None), Vec::new(), Vec::new()));
        app.facet_tag_suggestion_cache = Some(("stale".into(), Vec::new()));
        app.facet_tag_counts_cache = Some((std::collections::BTreeMap::new(), 1));
        #[cfg(windows)]
        {
            let stale = crate::video::native_presenter::NativeOverlayTagDef {
                name: "stale".into(),
                tag_key: "stale".into(),
                count: 1,
                pinned: false,
                last_applied_at: 1,
            };
            app.native_overlay_tag_choices_cache = Some(Arc::from([stale.clone()]));
            app.native_overlay_shortcut_tags_cache = Some(Arc::from([stale]));
        }
        let existing_due = std::time::Instant::now() + std::time::Duration::from_secs(10);
        app.smart_folder_metadata_refresh_due = Some(existing_due);
        let mut snapshot = empty_edit_rollup();
        snapshot.adjusted.insert("fresh-adjust".into());
        snapshot.masked.insert("fresh-mask".into());

        app.apply_sidecar_restore_global_effects(RestoreEffects {
            edits_changed: true,
            tags_changed: true,
            comic_changed: true,
            edit_rollup: Some(snapshot),
            tag_cache: Some(std::collections::HashMap::from([(
                "target-item".into(),
                vec!["new".into()],
            )])),
            ..RestoreEffects::default()
        });

        assert!(app.comic_docs.is_empty());
        assert_eq!(
            app.adjusted_page_keys,
            std::collections::BTreeSet::from(["fresh-adjust".to_string()])
        );
        assert_eq!(
            app.mask_page_keys,
            std::collections::BTreeSet::from(["fresh-mask".to_string()])
        );
        assert_eq!(
            app.tags_cache.get("sibling-item"),
            Some(&vec!["keep".to_string()]),
            "discarded target must not hydrate or clear the mounted sibling tag cache"
        );
        assert_eq!(
            app.smart_folder_metadata_refresh_due,
            Some(existing_due),
            "discarded target must not schedule its smart-folder tail on a sibling"
        );
        assert!(app.tag_apply_suggestion_key.is_none());
        assert!(app.tag_apply_suggestions.is_empty());
        assert!(app.tag_choice_catalog_cache.is_none());
        assert!(app.tag_apply_selection_cache.is_none());
        assert!(app.facet_tag_suggestion_cache.is_none());
        assert!(app.facet_tag_counts_cache.is_none());
        #[cfg(windows)]
        {
            assert!(app.native_overlay_tag_choices_cache.is_none());
            assert!(app.native_overlay_shortcut_tags_cache.is_none());
        }
    }

    #[test]
    fn committed_deltas_preserve_existing_indexes_and_rebuild_the_live_facet() {
        use crate::grid_item::ThumbnailState;
        use crate::settings::FacetEditFlag;

        let mut app = crate::app::setup_app_for_test();
        let folder = app.tmp.path().join("book");
        let image = folder.join("page.jpg");
        let page_key = crate::adjustment_db::normalize_path(&image);
        let tag_key = crate::tags_db::item_key_for_path(&image);
        app.current_folder = Some(folder);
        app.items = vec![GridItem::Image(image)];
        app.image_metas = vec![None];
        app.thumbnails = vec![ThumbnailState::Pending];
        app.selected = Some(0);
        app.adjusted_page_keys.insert("keep-existing".into());
        app.tags_cache
            .insert("sibling-item".into(), vec!["#keep".into()]);
        app.settings
            .facet_filter
            .edits
            .insert(FacetEditFlag::Adjustment);
        app.rebuild_visible_indices_preserving_facet_scope();
        assert!(app.visible_indices.is_empty());
        app.tag_apply_suggestion_key = Some("stale".into());

        assert!(
            app.install_sidecar_restore_result(
                None,
                RestoreEffects {
                    edits_changed: true,
                    tags_changed: true,
                    edit_delta: crate::sidecar_import::EditImportDelta {
                        adjusted: vec![page_key.clone()],
                        ..Default::default()
                    },
                    tag_delta: vec![(tag_key.clone(), vec!["#imported".into()])],
                    ..RestoreEffects::default()
                },
            )
            .is_none()
        );

        assert!(app.adjusted_page_keys.contains("keep-existing"));
        assert!(app.adjusted_page_keys.contains(&page_key));
        assert_eq!(
            app.tags_cache.get("sibling-item"),
            Some(&vec!["#keep".to_string()])
        );
        assert_eq!(
            app.tags_cache.get(&tag_key),
            Some(&vec!["#imported".to_string()])
        );
        assert_eq!(app.visible_indices, vec![0]);
        assert_eq!(app.selected, Some(0));
        assert!(app.tag_apply_suggestion_key.is_none());
    }

    #[test]
    fn thumbnail_requests_do_not_enter_the_cancelled_queue_during_restore() {
        use crate::grid_item::ThumbnailState;

        let mut app = crate::app::setup_app_for_test();
        let folder = app.tmp.path().join("book");
        app.current_folder = Some(folder.clone());
        app.items = vec![GridItem::Image(folder.join("page.jpg"))];
        app.image_metas = vec![Some((1, 2))];
        app.thumbnails = vec![ThumbnailState::Pending];
        let queue: Arc<NotifyQueue> = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        app.reload_queue = Some(Arc::clone(&queue));
        app.sidecar_restore = Some(discarded_restore_state(folder));

        app.enqueue_priority_thumbnail(0);

        assert!(app.requested.is_empty());
        assert!(queue.0.lock().unwrap().is_empty());
        app.sidecar_restore = None;
        app.enqueue_priority_thumbnail(0);
        assert_eq!(app.requested.len(), 1);
        assert_eq!(queue.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn stale_preview_result_releases_only_its_exact_request_owner() {
        use crate::grid_item::ThumbnailState;

        let mut requested = ThumbnailRequests::default();
        let mut pending_finalize = std::collections::HashSet::from([0]);
        let mut thumbnails = vec![ThumbnailState::Pending];
        requested.insert_with_edit_preview_epoch(0, false, Some(4));

        cleanup_stale_edit_preview_request(
            &mut requested,
            &mut pending_finalize,
            &mut thumbnails,
            0,
            4,
        );

        assert!(requested.is_empty());
        assert!(pending_finalize.is_empty());
        assert!(matches!(thumbnails[0], ThumbnailState::Evicted));

        requested.insert_with_edit_preview_epoch(0, false, Some(5));
        pending_finalize.insert(0);
        thumbnails[0] = ThumbnailState::Pending;

        cleanup_stale_edit_preview_request(
            &mut requested,
            &mut pending_finalize,
            &mut thumbnails,
            0,
            4,
        );

        assert_eq!(requested.get(&0), Some(&false));
        assert!(requested.owner_matches_edit_preview_epoch(0, 5));
        assert!(pending_finalize.contains(&0));
        assert!(matches!(thumbnails[0], ThumbnailState::Pending));
    }

    #[test]
    fn ordinary_flush_producers_leave_the_dirty_owner_for_the_restore_worker() {
        let mut app = crate::app::setup_app_for_test();
        let folder = app.tmp.path().join("book");
        let mut sidecar = crate::sidecar::SidecarFile::new(folder.clone());
        sidecar.set_tags("page.jpg", ["pending"]);
        sidecar.set_dirty_since_for_test(
            std::time::Instant::now() - crate::sidecar::PERIODIC_FLUSH_INTERVAL,
        );
        app.sidecars.insert(folder.clone(), sidecar);
        app.page_edit_tool_had_canvas = true;
        app.sidecar_restore = Some(discarded_restore_state(folder.clone()));

        app.flush_periodic_sidecars();
        app.flush_sidecars_when_page_edit_ends();

        assert!(app.sidecars.get(&folder).unwrap().is_dirty());
        assert!(!app.page_edit_tool_had_canvas);
    }

    #[test]
    fn refreshed_cache_never_replaces_an_existing_dirty_owner() {
        fn assert_unsaved_owner_is_retained(app: &App, folder: &Path) {
            let retained = app.sidecars.get(folder).expect("dirty owner retained");
            assert!(retained.is_dirty());
            assert_eq!(
                retained
                    .items()
                    .get("page.jpg")
                    .and_then(|entry| entry.tags.as_ref()),
                Some(&vec!["#unsaved".to_string()])
            );
        }

        let mut app = crate::app::setup_app_for_test();
        let folder = app.tmp.path().join("invalid");
        let mut dirty = crate::sidecar::SidecarFile::disabled_placeholder(folder.clone());
        dirty.set_tags("page.jpg", ["unsaved"]);
        app.sidecars.insert(folder.clone(), dirty);

        let warning = app
            .install_sidecar_restore_cache_owner(crate::sidecar::SidecarFile::new(folder.clone()));
        assert!(
            warning.is_some(),
            "probe Current must report the retained owner"
        );
        assert_unsaved_owner_is_retained(&app, &folder);

        let warning = app.disable_sidecar_restore_cache_owner(&folder);
        assert!(
            warning.is_some(),
            "a placeholder must not replace dirty state"
        );
        assert_unsaved_owner_is_retained(&app, &folder);

        let mut state = discarded_restore_state(folder.clone());
        app.apply_sidecar_cache_refresh(
            &mut state.common,
            CacheRefreshCompletion {
                sidecar: CacheRefreshResult::Disabled {
                    sidecar: crate::sidecar::SidecarFile::disabled_placeholder(folder.clone()),
                    error: "invalid source".into(),
                },
                recovered_db_state: None,
            },
        );
        assert!(state.common.warning.as_deref().is_some_and(|warning| {
            warning.contains("未保存変更を保持") && warning.contains("invalid source")
        }));
        assert_unsaved_owner_is_retained(&app, &folder);
    }

    fn create_recovery_edit_stores(data_dir: &Path) {
        drop(crate::adjustment_db::AdjustmentDb::open_at(&data_dir.join("adjustment.db")).unwrap());
        drop(
            crate::local_adjust_db::LocalAdjustDb::open_at(&data_dir.join("local_adjust.db"))
                .unwrap(),
        );
        drop(crate::mask_db::MaskDb::open_at(&data_dir.join("mask.db")).unwrap());
        drop(crate::conceal_db::ConcealDb::open_at(&data_dir.join("conceal.db")).unwrap());
        drop(crate::comic_db::ComicDb::open_at(&data_dir.join("comic.db")).unwrap());
        drop(crate::rotation_db::RotationDb::open_at(&data_dir.join("rotation.db")).unwrap());
        drop(crate::export_crop::CropDb::open_at(&data_dir.join("export_crop.db")).unwrap());
    }

    #[test]
    fn unknown_commit_recovery_reads_authoritative_edit_and_tag_state() {
        let temp = tempfile::TempDir::new().unwrap();
        create_recovery_edit_stores(temp.path());
        let page_key = "c:/book/page.jpg";
        let adjustment =
            crate::adjustment_db::AdjustmentDb::open_at(&temp.path().join("adjustment.db"))
                .unwrap();
        adjustment
            .set_page_params(page_key, &crate::adjustment::AdjustParams::default())
            .unwrap();
        let mut tags = crate::tags_db::TagsDb::open_at(&temp.path().join("tags.db")).unwrap();
        tags.set_item_tags(page_key, ["recovered"], crate::tags_db::source::EDIT)
            .unwrap();
        drop(tags);

        let recovered = load_recovered_db_state(
            temp.path(),
            crate::sidecar_import::ImportFamilies {
                edits: true,
                tags: true,
            },
            &[page_key.to_string()],
        )
        .unwrap();

        assert!(recovered.edit_rollup.unwrap().adjusted.contains(page_key));
        assert_eq!(
            recovered.tag_cache.unwrap().get(page_key),
            Some(&vec!["#recovered".to_string()])
        );
    }

    #[test]
    fn unknown_commit_recovery_strict_tag_failure_is_finite_and_not_empty_success() {
        let temp = tempfile::TempDir::new().unwrap();
        drop(rusqlite::Connection::open(temp.path().join("tags.db")).unwrap());
        let started = std::time::Instant::now();

        let error = match load_recovered_db_state(
            temp.path(),
            crate::sidecar_import::ImportFamilies {
                edits: false,
                tags: true,
            },
            &["c:/book/page.jpg".to_string()],
        ) {
            Ok(_) => panic!("missing item_tags must not become an authoritative empty cache"),
            Err(error) => error,
        };

        assert!(error.contains("3回"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn cache_refresh_recovers_a_stable_missing_owner_after_target_cancellation() {
        let temp = tempfile::TempDir::new().unwrap();
        let folder = temp.path().join("book");
        std::fs::create_dir_all(&folder).unwrap();
        let state = discarded_restore_state(folder.clone());
        state
            .common
            .continuation
            .cancel()
            .store(true, Ordering::Relaxed);
        assert!(state.common.continuation.cancel().load(Ordering::Relaxed));
        match load_current_sidecar_cache(&folder, crate::sidecar_import::ImportFamilies::ALL) {
            CacheRefreshResult::Current(sidecar) => assert_eq!(sidecar.folder(), folder),
            CacheRefreshResult::Disabled { error, .. } => {
                panic!("stable Missing source must remain usable: {error}")
            }
        }
    }

    #[test]
    fn cache_refresh_disables_a_corrupt_source_instead_of_returning_a_stale_owner() {
        let temp = tempfile::TempDir::new().unwrap();
        let folder = temp.path().join("book");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(crate::sidecar::SIDECAR_FILENAME), b"not-json").unwrap();
        match load_current_sidecar_cache(&folder, crate::sidecar_import::ImportFamilies::ALL) {
            CacheRefreshResult::Current(_) => panic!("corrupt source must not be installable"),
            CacheRefreshResult::Disabled { sidecar, error } => {
                assert_eq!(sidecar.folder(), folder);
                assert!(error.contains("読み直せません"), "{error}");
            }
        }
    }

    #[test]
    fn cache_refresh_keeps_a_stable_semantically_invalid_source_write_disabled() {
        let temp = tempfile::TempDir::new().unwrap();
        let folder = temp.path().join("book");
        std::fs::create_dir_all(&folder).unwrap();
        let mut source = crate::sidecar::SidecarFile::new(folder.clone());
        source.set_adjust("../escape.jpg", crate::adjustment::AdjustParams::default());
        source.set_tags("page.jpg", ["#valid"]);
        assert!(source.flush_blocking());
        let source_path = folder.join(crate::sidecar::SIDECAR_FILENAME);
        let original = std::fs::read(&source_path).unwrap();
        let CacheRefreshResult::Disabled { mut sidecar, error } = load_current_sidecar_cache(
            &folder,
            crate::sidecar_import::ImportFamilies {
                edits: false,
                tags: true,
            },
        ) else {
            panic!("an invalid unrequested edit must still disable whole-file writes");
        };
        assert!(error.contains("writes are disabled"), "{error}");
        sidecar.set_tags("page.jpg", ["#changed"]);
        assert!(!sidecar.flush_blocking());
        assert_eq!(std::fs::read(source_path).unwrap(), original);
    }
}
