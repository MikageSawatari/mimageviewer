use mimageviewer_ipc::{
    RemoteAdjustmentReadOnlyState, RemoteAdjustmentScope, RemoteAdjustmentState,
    RemoteAiModelCatalog, RemoteAiModelOption, RemoteItemState, RemoteReadingDirection,
    RemoteSessionIdentity, RemoteSpreadMode, RemoteSubresource, RemoteWebFeatureStatus,
    RemoteWriteError, RemoteWriteErrorCode, RemoteWriteRequest, RemoteWriteResponse,
    RemoteWriteResult, SessionConnectionKind, SessionResponse, SessionStatus,
    VideoStreamControlAction, VideoStreamEndBehavior, VideoStreamError, VideoStreamErrorCode,
};
use qrcode::{Color, QrCode};

use crate::video::stream::session::{
    RemoteVideoStreamingSession, StreamGenerationStatus, StreamReconcile, StreamingGeneration,
};

use super::session::{
    ActiveSessionSnapshot, ClaimedRemoteUiRequest, ClaimedRemoteWrite, ClaimedVideoStreamUiRequest,
    PublishedVideoStream, RemoteStreamingControlError, SessionHandle, UiWriteOutcome,
    VideoStreamPlaybackState, VideoStreamUiOutcome, VideoStreamUiRequest,
};
use super::video_stream::{VideoStreamStartBudget, VideoStreamStartStage};

const REMOTE_ACQUIRE_BARRIER_LOG_AFTER: std::time::Duration = std::time::Duration::from_secs(3);
const REMOTE_ACQUIRE_BARRIER_LOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
const REMOTE_ACQUIRE_BARRIER_ABORT_AFTER: std::time::Duration = std::time::Duration::from_secs(30);
const REMOTE_ENABLE_WARNING_PREFIX: &str = "リモート閲覧を有効にすると、";
const REMOTE_ENABLE_WARNING_EMPHASIS: &str = "すべてのドライブについて、mIV で表示できるファイル";
const REMOTE_ENABLE_WARNING_SUFFIX: &str =
    "が、この PC の Tailscale アドレスへ接続でき、PIN を知っている人から見えるようになります。";
const REMOTE_MANUAL_URL: &str = "https://mikage.to/mimageviewer/manual/tut-remote.html";
const REMOTE_TAILSCALE_DNS_URL: &str = "https://console.tailscale.com/admin/dns";
const REMOTE_TAILSCALE_MACHINES_URL: &str = "https://console.tailscale.com/admin/machines";
const REMOTE_KEY_EXPIRY_WARNING_DAYS: i64 = 30;

#[derive(Default)]
pub(crate) struct RemoteSessionUiState {
    handle: Option<SessionHandle>,
    remote_service_status: Option<super::RemoteServiceStatus>,
    remote_service_control: Option<super::RemoteServiceControl>,
    last_acquisition_sequence: u64,
    last_control_return_sequence: u64,
    pending_fullscreen_restore: Option<PendingFullscreenRestore>,
    paused_animation_restore_key: Option<String>,
    pending_bookmark_writes: std::collections::HashMap<u64, PendingRemoteBookmarkWrite>,
    connection_dialog: Option<RemoteConnectionDialogState>,
    // Unlike the dialog-owned PIN and tailscale receivers, logout completion must outlive the
    // window because a successful secret rotation still has to disconnect local control.
    session_logout: RemoteSessionLogoutState,
    video_stream: Option<AppRemoteVideoStreamState>,
    local_ai_lease: Option<RemoteLocalAiLease>,
    acquire_barrier_diagnostics: RemoteAcquireBarrierDiagnostics,
}

struct RemoteConnectionDialogState {
    pin_editor: RemotePinEditorState,
    tailscale_serve_setup: RemoteTailscaleServeSetupState,
}

enum RemotePinEditorState {
    Hidden,
    Editing {
        input: String,
        error: Option<String>,
        request_focus: bool,
    },
    Saving {
        receiver: super::service::RemotePinUpdateReceiver,
    },
}

enum RemoteTailscaleServeSetupState {
    Idle,
    Running {
        receiver: super::service::RemoteTailscaleServeReceiver,
    },
    Finished(Result<(), String>),
}

enum RemoteSessionLogoutState {
    Idle,
    Confirming,
    Running {
        receiver: super::service::RemoteSessionSecretRotationReceiver,
    },
    Finished(Result<(), String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteSessionLogoutPhase {
    Idle,
    Confirming,
    Running,
    FinishedSuccess,
    FinishedError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteSessionLogoutEvent {
    RequestConfirmation,
    Confirm,
    Cancel,
    StartFailed,
    FinishSuccess,
    FinishError,
    DialogClosed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemoteSessionLogoutTransition {
    phase: RemoteSessionLogoutPhase,
    disconnect_local: bool,
}

fn remote_session_logout_transition(
    phase: RemoteSessionLogoutPhase,
    event: RemoteSessionLogoutEvent,
) -> Option<RemoteSessionLogoutTransition> {
    let transition = |phase| RemoteSessionLogoutTransition {
        phase,
        disconnect_local: false,
    };
    match (phase, event) {
        (
            RemoteSessionLogoutPhase::Idle
            | RemoteSessionLogoutPhase::FinishedSuccess
            | RemoteSessionLogoutPhase::FinishedError,
            RemoteSessionLogoutEvent::RequestConfirmation,
        ) => Some(transition(RemoteSessionLogoutPhase::Confirming)),
        (RemoteSessionLogoutPhase::Confirming, RemoteSessionLogoutEvent::Confirm) => {
            Some(transition(RemoteSessionLogoutPhase::Running))
        }
        (RemoteSessionLogoutPhase::Confirming, RemoteSessionLogoutEvent::Cancel) => {
            Some(transition(RemoteSessionLogoutPhase::Idle))
        }
        (RemoteSessionLogoutPhase::Confirming, RemoteSessionLogoutEvent::StartFailed)
        | (RemoteSessionLogoutPhase::Running, RemoteSessionLogoutEvent::FinishError) => {
            Some(transition(RemoteSessionLogoutPhase::FinishedError))
        }
        (RemoteSessionLogoutPhase::Running, RemoteSessionLogoutEvent::FinishSuccess) => {
            Some(RemoteSessionLogoutTransition {
                phase: RemoteSessionLogoutPhase::FinishedSuccess,
                disconnect_local: true,
            })
        }
        (RemoteSessionLogoutPhase::Idle, RemoteSessionLogoutEvent::DialogClosed) => {
            Some(transition(RemoteSessionLogoutPhase::Idle))
        }
        (RemoteSessionLogoutPhase::Confirming, RemoteSessionLogoutEvent::DialogClosed) => {
            Some(transition(RemoteSessionLogoutPhase::Idle))
        }
        (RemoteSessionLogoutPhase::Running, RemoteSessionLogoutEvent::DialogClosed) => {
            Some(transition(RemoteSessionLogoutPhase::Running))
        }
        (RemoteSessionLogoutPhase::FinishedSuccess, RemoteSessionLogoutEvent::DialogClosed) => {
            Some(transition(RemoteSessionLogoutPhase::Idle))
        }
        // 失敗はここで消さない。「もう一度試す」と一緒に残し、利用者が retry するまで見える。
        (RemoteSessionLogoutPhase::FinishedError, RemoteSessionLogoutEvent::DialogClosed) => {
            Some(transition(RemoteSessionLogoutPhase::FinishedError))
        }
        _ => None,
    }
}

impl Default for RemoteSessionLogoutState {
    fn default() -> Self {
        Self::Idle
    }
}

impl RemoteSessionLogoutState {
    fn phase(&self) -> RemoteSessionLogoutPhase {
        match self {
            Self::Idle => RemoteSessionLogoutPhase::Idle,
            Self::Confirming => RemoteSessionLogoutPhase::Confirming,
            Self::Running { .. } => RemoteSessionLogoutPhase::Running,
            Self::Finished(Ok(())) => RemoteSessionLogoutPhase::FinishedSuccess,
            Self::Finished(Err(_)) => RemoteSessionLogoutPhase::FinishedError,
        }
    }

    fn request_confirmation(&mut self) {
        debug_assert_eq!(
            remote_session_logout_transition(
                self.phase(),
                RemoteSessionLogoutEvent::RequestConfirmation,
            ),
            Some(RemoteSessionLogoutTransition {
                phase: RemoteSessionLogoutPhase::Confirming,
                disconnect_local: false,
            })
        );
        *self = Self::Confirming;
    }

    fn cancel_confirmation(&mut self) {
        debug_assert_eq!(
            remote_session_logout_transition(self.phase(), RemoteSessionLogoutEvent::Cancel,),
            Some(RemoteSessionLogoutTransition {
                phase: RemoteSessionLogoutPhase::Idle,
                disconnect_local: false,
            })
        );
        *self = Self::Idle;
    }

    fn start(
        &mut self,
        result: Result<super::service::RemoteSessionSecretRotationReceiver, String>,
    ) {
        let event = if result.is_ok() {
            RemoteSessionLogoutEvent::Confirm
        } else {
            RemoteSessionLogoutEvent::StartFailed
        };
        debug_assert!(remote_session_logout_transition(self.phase(), event).is_some());
        *self = match result {
            Ok(receiver) => Self::Running { receiver },
            Err(error) => Self::Finished(Err(error)),
        };
    }

    fn finish(&mut self, result: Result<(), String>) -> bool {
        let event = if result.is_ok() {
            RemoteSessionLogoutEvent::FinishSuccess
        } else {
            RemoteSessionLogoutEvent::FinishError
        };
        let transition = remote_session_logout_transition(self.phase(), event)
            .expect("only a running logout can finish");
        *self = Self::Finished(result);
        transition.disconnect_local
    }

    fn dialog_closed(&mut self) {
        let transition =
            remote_session_logout_transition(self.phase(), RemoteSessionLogoutEvent::DialogClosed)
                .expect("every logout state defines dialog-close behavior");
        match transition.phase {
            RemoteSessionLogoutPhase::Idle => *self = Self::Idle,
            RemoteSessionLogoutPhase::Running
            | RemoteSessionLogoutPhase::FinishedError
            | RemoteSessionLogoutPhase::Confirming
            | RemoteSessionLogoutPhase::FinishedSuccess => {}
        }
    }
}

impl RemoteConnectionDialogState {
    fn new(pin_configured: bool) -> Self {
        Self {
            pin_editor: if pin_configured {
                RemotePinEditorState::Hidden
            } else {
                RemotePinEditorState::Editing {
                    input: String::new(),
                    error: None,
                    request_focus: true,
                }
            },
            tailscale_serve_setup: RemoteTailscaleServeSetupState::Idle,
        }
    }
}

fn remote_enable_action_allowed(pin_configured: bool) -> bool {
    pin_configured
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemoteTailscaleServeElements {
    show_setup_button: bool,
    setup_button_enabled: bool,
    show_conflict_warning: bool,
    show_unsupported_path_warning: bool,
    show_unknown_message: bool,
    show_removal_note: bool,
}

fn remote_tailscale_serve_elements(
    serve_status: RemoteWebFeatureStatus,
    https_certificate: RemoteWebFeatureStatus,
    has_conflict: bool,
    has_unsupported_path: bool,
) -> RemoteTailscaleServeElements {
    match serve_status {
        RemoteWebFeatureStatus::Configured => RemoteTailscaleServeElements {
            show_setup_button: false,
            setup_button_enabled: false,
            show_conflict_warning: false,
            show_unsupported_path_warning: false,
            show_unknown_message: false,
            show_removal_note: true,
        },
        RemoteWebFeatureStatus::NotConfigured => RemoteTailscaleServeElements {
            show_setup_button: true,
            setup_button_enabled: https_certificate != RemoteWebFeatureStatus::NotConfigured,
            show_conflict_warning: has_conflict,
            show_unsupported_path_warning: has_unsupported_path,
            show_unknown_message: false,
            show_removal_note: false,
        },
        RemoteWebFeatureStatus::Unknown => RemoteTailscaleServeElements {
            show_setup_button: false,
            setup_button_enabled: false,
            show_conflict_warning: false,
            show_unsupported_path_warning: false,
            show_unknown_message: true,
            show_removal_note: false,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteKeyExpiryDisplay {
    Unavailable,
    Normal { remaining_days: i64 },
    Warning { remaining_days: i64 },
    Expired,
}

fn remote_key_expiry_display(
    expiry_unix_seconds: Option<i64>,
    now_unix_seconds: i64,
) -> RemoteKeyExpiryDisplay {
    let Some(expiry_unix_seconds) = expiry_unix_seconds else {
        return RemoteKeyExpiryDisplay::Unavailable;
    };
    if expiry_unix_seconds <= now_unix_seconds {
        return RemoteKeyExpiryDisplay::Expired;
    }
    let remaining_seconds = expiry_unix_seconds.saturating_sub(now_unix_seconds);
    let remaining_days = remaining_seconds.saturating_add(86_399) / 86_400;
    if remaining_days <= REMOTE_KEY_EXPIRY_WARNING_DAYS {
        RemoteKeyExpiryDisplay::Warning { remaining_days }
    } else {
        RemoteKeyExpiryDisplay::Normal { remaining_days }
    }
}

fn remote_connection_state_label(
    accepting: bool,
    diagnostic: &super::service::RemoteServiceDiagnostic,
) -> &'static str {
    if accepting {
        "受け付けています"
    } else {
        match diagnostic {
            super::service::RemoteServiceDiagnostic::Stopped => "停止しています",
            super::service::RemoteServiceDiagnostic::Starting => "準備しています",
            super::service::RemoteServiceDiagnostic::VersionMismatch
            | super::service::RemoteServiceDiagnostic::Error(_) => "開始できません",
        }
    }
}

fn remote_client_state_label(active: bool) -> &'static str {
    if active { "操作中" } else { "なし" }
}

struct RemoteLocalAiLease {
    acquisition_sequence: u64,
    resume_video_upscale: bool,
}

struct RemoteAcquireBarrierDiagnostics {
    generation: u64,
    next_log_after: std::time::Duration,
}

impl Default for RemoteAcquireBarrierDiagnostics {
    fn default() -> Self {
        Self {
            generation: 0,
            next_log_after: REMOTE_ACQUIRE_BARRIER_LOG_AFTER,
        }
    }
}

impl RemoteAcquireBarrierDiagnostics {
    fn begin(&mut self, generation: u64) {
        self.generation = generation;
        self.next_log_after = REMOTE_ACQUIRE_BARRIER_LOG_AFTER;
    }

    fn should_log(&mut self, generation: u64, elapsed: std::time::Duration) -> bool {
        if self.generation != generation {
            self.begin(generation);
        }
        if elapsed < self.next_log_after {
            return false;
        }
        while self.next_log_after <= elapsed {
            self.next_log_after = self
                .next_log_after
                .saturating_add(REMOTE_ACQUIRE_BARRIER_LOG_INTERVAL);
        }
        true
    }
}

fn coalesced_remote_reacquire(
    phase: super::session::RemoteControlPhase,
    acquisition_changed: bool,
    control_returned: bool,
) -> bool {
    phase.blocks_local_control() && acquisition_changed && control_returned
}

enum AppRemoteVideoStreamState {
    Opening(AppRemoteVideoOpening),
    Starting(AppRemoteVideoStarting),
    Streaming(AppRemoteVideoStreaming),
}

struct AppRemoteVideoOpening {
    claimed: ClaimedVideoStreamUiRequest,
    owner: RemoteSessionIdentity,
    requested_path: std::path::PathBuf,
    quality: crate::video::stream::quality::QualityPreset,
    player: Option<Box<crate::video::VideoPlayer>>,
    budget: VideoStreamStartBudget,
}

struct AppRemoteVideoStreaming {
    requested_path: std::path::PathBuf,
    player: Box<crate::video::VideoPlayer>,
    session: RemoteVideoStreamingSession,
    playback: std::sync::Arc<VideoStreamPlaybackState>,
    end_behavior: VideoStreamEndBehavior,
    jump_catalog: std::sync::Arc<super::video_jump::VideoJumpCatalogSource>,
}

fn resolve_remote_video_end_behavior(
    continuous_mode: crate::video::VideoContinuousMode,
    loop_mode: crate::settings::VideoLoopMode,
    chapter_starts: Vec<f64>,
    bookmark_starts: Vec<f64>,
) -> VideoStreamEndBehavior {
    match continuous_mode {
        crate::video::VideoContinuousMode::Continuous => {
            return VideoStreamEndBehavior::Next { wrap: false };
        }
        crate::video::VideoContinuousMode::ContinuousLoop => {
            return VideoStreamEndBehavior::Next { wrap: true };
        }
        crate::video::VideoContinuousMode::Off => {}
    }

    let effective = crate::settings::effective_loop_mode(
        loop_mode,
        !chapter_starts.is_empty(),
        !bookmark_starts.is_empty(),
    );
    match effective {
        crate::settings::VideoLoopMode::Off => VideoStreamEndBehavior::Stop,
        crate::settings::VideoLoopMode::Full => VideoStreamEndBehavior::Loop {
            boundary_starts_secs: vec![0.0],
        },
        crate::settings::VideoLoopMode::Chapter => VideoStreamEndBehavior::Loop {
            boundary_starts_secs: chapter_starts,
        },
        crate::settings::VideoLoopMode::Bookmark => VideoStreamEndBehavior::Loop {
            boundary_starts_secs: bookmark_starts,
        },
    }
}

fn remote_vst_load_budget(start_budget_remaining: std::time::Duration) -> std::time::Duration {
    start_budget_remaining
        .saturating_sub(std::time::Duration::from_secs(3))
        .min(std::time::Duration::from_secs(10))
}

struct AppRemoteVideoStarting {
    claimed: ClaimedVideoStreamUiRequest,
    streaming: AppRemoteVideoStreaming,
    budget: VideoStreamStartBudget,
    waiting_stage: VideoStreamStartStage,
}

struct PendingRemoteBookmarkWrite {
    claimed: ClaimedRemoteWrite,
    kind: PendingRemoteBookmarkWriteKind,
    container_path: std::path::PathBuf,
}

enum PendingRemoteBookmarkWriteKind {
    ReadState { rating: u8 },
    SetPresence,
    SetTitle,
    Remove,
}

enum RemoteBookmarkRequestAction {
    ReadState { bookmark_supported: bool },
    SetPresence { present: bool },
    SetTitle { id: i64, title: String },
    Remove { id: i64 },
}

struct PendingFullscreenRestore {
    item_key: String,
    view: ReloadedView,
    wait_frames: u8,
}

#[derive(Clone, Copy)]
enum ReloadedView {
    ReadingHistory,
    Rating,
    Bookmarks,
    SmartFolder(uuid::Uuid),
    Other,
}

fn video_stream_ui_failure(message: String) -> VideoStreamUiOutcome {
    VideoStreamUiOutcome::Error(VideoStreamError::new(VideoStreamErrorCode::Failed, message))
}

fn video_stream_session_mismatch() -> VideoStreamUiOutcome {
    VideoStreamUiOutcome::Error(VideoStreamError::new(
        VideoStreamErrorCode::SessionMismatch,
        "動画ストリーミングセッションが一致しません",
    ))
}

fn remote_streaming_control_error(error: RemoteStreamingControlError) -> VideoStreamError {
    if error.is_session_mismatch() {
        VideoStreamError::new(
            VideoStreamErrorCode::SessionMismatch,
            "remote session ownership was lost",
        )
    } else {
        VideoStreamError::new(
            VideoStreamErrorCode::Failed,
            format!("streaming control registration is not current: {error:?}"),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteVideoStartReadiness {
    Stable,
    WaitingForEncoder,
}

impl RemoteVideoStartReadiness {
    fn wait_stage(self) -> VideoStreamStartStage {
        match self {
            Self::Stable => VideoStreamStartStage::Seek,
            Self::WaitingForEncoder => VideoStreamStartStage::Encoder,
        }
    }
}

fn remote_video_start_outcome_for_player(
    session: &mut RemoteVideoStreamingSession,
    playback: &std::sync::Arc<VideoStreamPlaybackState>,
    player: &crate::video::VideoPlayer,
) -> Result<(StreamReconcile, RemoteVideoStartReadiness), String> {
    let snapshot = playback.snapshot();
    playback.update(player.duration(), player.volume(), snapshot.play_intent);
    let reconcile = session.reconcile();
    let readiness = if matches!(
        session.status(),
        StreamGenerationStatus::Ready(_) | StreamGenerationStatus::Ended(_)
    ) {
        RemoteVideoStartReadiness::Stable
    } else {
        RemoteVideoStartReadiness::WaitingForEncoder
    };
    Ok((reconcile, readiness))
}

impl crate::app::App {
    pub(crate) fn set_remote_session_handle(&mut self, handle: SessionHandle) {
        let snapshot = handle.snapshot();
        self.remote_session_ui.last_acquisition_sequence = if snapshot.active.is_some() {
            snapshot.acquisition_sequence.wrapping_sub(1)
        } else {
            snapshot.acquisition_sequence
        };
        self.remote_session_ui.last_control_return_sequence = snapshot.control_return_sequence;
        handle.install_ai_bridge(super::session::RemoteAiExecutionBridge::new(
            self.ai_runtime.clone(),
            std::sync::Arc::clone(&self.ai_model_manager),
            self.fs_transparent_bg_mode,
        ));
        handle.install_archive_cache_db(self.archive_cache_db.clone());
        self.remote_session_ui.handle = Some(handle);
    }

    pub(crate) fn set_remote_service_control(
        &mut self,
        status: super::RemoteServiceStatus,
        control: Option<super::RemoteServiceControl>,
    ) {
        self.remote_session_ui.remote_service_status = Some(status);
        self.remote_session_ui.remote_service_control = control;
    }

    pub(crate) fn open_remote_connection_dialog(&mut self) {
        let pin_configured = self
            .remote_session_ui
            .remote_service_control
            .as_ref()
            .is_some_and(super::RemoteServiceControl::pin_configured);
        self.remote_session_ui.connection_dialog =
            Some(RemoteConnectionDialogState::new(pin_configured));
    }

    pub(crate) fn remote_connection_dialog_open(&self) -> bool {
        self.remote_session_ui.connection_dialog.is_some()
    }

    pub(crate) fn remote_ai_execution_bridge(
        &self,
    ) -> Option<super::session::RemoteAiExecutionBridge> {
        self.remote_session_ui
            .handle
            .as_ref()
            .and_then(SessionHandle::ai_bridge)
    }

    pub(crate) fn consume_remote_animation_pause_restore(&mut self, index: usize) -> bool {
        let Some(expected) = self
            .remote_session_ui
            .paused_animation_restore_key
            .as_deref()
        else {
            return false;
        };
        let matches = self
            .items
            .get(index)
            .is_some_and(|item| item.perf_key() == expected);
        if matches {
            self.remote_session_ui.paused_animation_restore_key = None;
        }
        matches
    }

    pub(crate) fn remote_session_blocks_local_control(&self) -> bool {
        self.remote_session_ui
            .handle
            .as_ref()
            .is_some_and(|handle| handle.snapshot().phase.blocks_local_control())
    }

    #[allow(dead_code)] // 3b-1 remote AI admission uses only the operational phase.
    pub(crate) fn remote_session_operational(&self) -> bool {
        self.remote_session_ui
            .handle
            .as_ref()
            .is_some_and(|handle| {
                handle.snapshot().phase == super::session::RemoteControlPhase::RemoteActive
            })
    }

    pub(crate) fn poll_remote_session(&mut self, ctx: &egui::Context) {
        let handle = self.remote_session_ui.handle.clone();
        if let Some(handle) = handle.as_ref() {
            handle.install_repaint_context(ctx);
        }
        if self.ai_runtime.is_none()
            && let Some(runtime) = self
                .remote_ai_execution_bridge()
                .and_then(|bridge| bridge.ready_runtime())
        {
            self.ai_runtime = Some(runtime);
        }
        let snapshot = self
            .remote_session_ui
            .handle
            .as_ref()
            .map(SessionHandle::snapshot);
        let remote_phase = snapshot
            .as_ref()
            .map_or(super::session::RemoteControlPhase::Local, |value| {
                value.phase
            });
        let blocks_local_control = remote_phase.blocks_local_control();
        let acquisition_changed = snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.acquisition_sequence != self.remote_session_ui.last_acquisition_sequence
        });
        let control_returned = snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.control_return_sequence != self.remote_session_ui.last_control_return_sequence
        });
        // The remote owner is the resource boundary. Tear down its taps and worker before the
        // existing acquire/release path pauses or reloads media; that path remains the one source
        // of truth for playback state transitions.
        if acquisition_changed || control_returned {
            self.cancel_remote_video_stream_state(
                VideoStreamErrorCode::SessionMismatch,
                "リモートセッションの操作権が移動しました",
            );
        }
        let coalesced_reacquire =
            coalesced_remote_reacquire(remote_phase, acquisition_changed, control_returned);
        if let Some(snapshot) = snapshot.as_ref()
            && control_returned
        {
            self.remote_session_ui.last_control_return_sequence = snapshot.control_return_sequence;
            if coalesced_reacquire {
                // The old worker may finish its drain and a new client may acquire between UI
                // frames. In that case local control was never observable: keep the singleton AI
                // barrier held and do not enqueue a stale local-view reload under the new owner.
                crate::logger::log(format!(
                    "remote_ipc: coalesced control return into reacquire generation={} acquisition_sequence={}",
                    snapshot.generation, snapshot.acquisition_sequence
                ));
            } else {
                if let Some(lease) = self.remote_session_ui.local_ai_lease.take() {
                    self.release_local_ai_remote_barrier(lease.resume_video_upscale);
                }
                self.reload_after_remote_session_release();
            }
        }
        if let Some(snapshot) = snapshot.as_ref()
            && acquisition_changed
        {
            self.remote_session_ui.last_acquisition_sequence = snapshot.acquisition_sequence;
            let (media, slideshow, animations, continuous) =
                self.pause_local_progress_for_remote_session();
            crate::logger::log(format!(
                "remote_ipc: local playback paused on session acquire media={media} slideshow={slideshow} animations={animations} continuous_pending={continuous}"
            ));
            let mut resume_video_upscale = self.begin_local_ai_remote_barrier();
            if let Some(previous) = self.remote_session_ui.local_ai_lease.take() {
                resume_video_upscale |= previous.resume_video_upscale;
                crate::logger::log(format!(
                    "remote_ipc: local AI barrier carried across reacquire old_acquisition_sequence={} new_acquisition_sequence={}",
                    previous.acquisition_sequence, snapshot.acquisition_sequence
                ));
            }
            self.remote_session_ui.local_ai_lease = Some(RemoteLocalAiLease {
                acquisition_sequence: snapshot.acquisition_sequence,
                resume_video_upscale,
            });
            self.remote_session_ui
                .acquire_barrier_diagnostics
                .begin(snapshot.generation);
        }
        if remote_phase == super::session::RemoteControlPhase::AcquiringRemote
            && let (Some(handle), Some(snapshot)) = (handle.as_ref(), snapshot.as_ref())
        {
            let barrier = self.local_ai_remote_barrier_snapshot();
            if barrier.is_quiesced() {
                handle.finish_acquire(snapshot.generation);
            } else {
                let elapsed = snapshot
                    .active
                    .as_ref()
                    .map_or(std::time::Duration::ZERO, |active| active.elapsed);
                if self
                    .remote_session_ui
                    .acquire_barrier_diagnostics
                    .should_log(snapshot.generation, elapsed)
                {
                    crate::logger::log(format!(
                        "remote_ipc: acquire_barrier_wait generation={} elapsed_ms={} blockers={}",
                        snapshot.generation,
                        elapsed.as_millis(),
                        barrier.blocker_summary()
                    ));
                }
                if elapsed >= REMOTE_ACQUIRE_BARRIER_ABORT_AFTER {
                    crate::logger::log(format!(
                        "remote_ipc: acquire_barrier_timeout generation={} elapsed_ms={} blockers={}",
                        snapshot.generation,
                        elapsed.as_millis(),
                        barrier.blocker_summary()
                    ));
                    handle.abort_acquire_barrier(snapshot.generation);
                }
            }
        }
        if remote_phase == super::session::RemoteControlPhase::DrainingRemote {
            self.cancel_remote_video_stream_state(
                VideoStreamErrorCode::SessionMismatch,
                "リモートセッションを終了しています",
            );
        }
        // Reconcile the previous frame's stream before draining new requests. The remote session
        // owns and ticks its headless player without changing the local viewer presentation.
        self.poll_remote_video_streaming(ctx);
        // Observe owner transitions before draining its requests. In particular, an acquire and
        // the first video start can both arrive before the next UI frame; cancelling after the
        // drain would incorrectly reject that new owner's freshly opened stream.
        if let Some(handle) = handle.as_ref() {
            self.apply_pending_remote_ui_requests(handle, ctx);
        }
        self.poll_remote_fullscreen_restore();
        if remote_phase == super::session::RemoteControlPhase::DrainingRemote
            && let (Some(handle), Some(snapshot)) = (handle.as_ref(), snapshot.as_ref())
            && handle.complete_app_drain(snapshot.generation)
        {
            let released = handle.snapshot();
            self.remote_session_ui.last_control_return_sequence = released.control_return_sequence;
            if let Some(lease) = self.remote_session_ui.local_ai_lease.take() {
                self.release_local_ai_remote_barrier(lease.resume_video_upscale);
            }
            self.reload_after_remote_session_release();
        }
        if matches!(
            self.remote_session_ui.video_stream.as_ref(),
            Some(AppRemoteVideoStreamState::Opening(_) | AppRemoteVideoStreamState::Starting(_))
        ) {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        } else if blocks_local_control
            || self.remote_session_ui.pending_fullscreen_restore.is_some()
        {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    fn create_remote_video_streaming(
        &mut self,
        session_owner: &RemoteSessionIdentity,
        requested_path: &std::path::Path,
        quality: crate::video::stream::quality::QualityPreset,
        start_inputs: crate::video::RemoteStreamStartInputs,
        player: Box<crate::video::VideoPlayer>,
        start_budget_remaining: std::time::Duration,
    ) -> Result<AppRemoteVideoStreaming, String> {
        if !self.settings.remote_video_streaming_enabled {
            return Err("remote video streaming is disabled".to_owned());
        }
        let owner = self
            .remote_session_ui
            .handle
            .as_ref()
            .ok_or_else(|| "remote session service is unavailable".to_owned())?
            .streaming_owner(session_owner)
            .map_err(|response| response.message)?;
        let encoder = self.settings.remote_video_encoder.into();
        let segment_capacity = self.settings.remote_video_segment_window;
        if !crate::folder_tree::path_eq(player.path(), requested_path) {
            return Err("the requested video is not the remote session player".to_owned());
        }
        let chapters = player
            .info()
            .map(|info| info.chapters.clone())
            .unwrap_or_default();
        let chapter_starts = crate::video::decoder::boundary_starts_from_chapters(&chapters);
        let bookmark_starts = if matches!(
            self.settings.video_loop_mode,
            crate::settings::VideoLoopMode::Bookmark
        ) && matches!(
            self.video_continuous_mode,
            crate::video::VideoContinuousMode::Off
        ) {
            self.video_bookmark_db
                .as_ref()
                .map(|db| db.list_marker_entries(player.path()))
                .map(|bookmarks| crate::video_bookmarks::boundary_starts_from_bookmarks(&bookmarks))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let end_behavior = resolve_remote_video_end_behavior(
            self.video_continuous_mode,
            self.settings.video_loop_mode,
            chapter_starts,
            bookmark_starts,
        );
        let previous_speed = player.playback_speed();
        let speed_changed = (previous_speed - 1.0).abs() > 1.0e-6;
        if speed_changed {
            player.set_playback_speed(1.0);
        }
        let audio_processing = self
            .remote_clockless_audio_processing(start_inputs.normalize_gain, start_budget_remaining);
        let session = match RemoteVideoStreamingSession::start(
            owner,
            &player,
            start_inputs,
            encoder,
            quality,
            segment_capacity,
            self.settings.video_hw_decode,
            audio_processing,
        ) {
            Ok(session) => session,
            Err(error) => {
                if speed_changed {
                    player.set_playback_speed(previous_speed);
                }
                return Err(error);
            }
        };
        let playback = std::sync::Arc::new(VideoStreamPlaybackState::new(
            start_inputs.duration_secs,
            player.volume(),
            true,
        ));
        let jump_catalog = std::sync::Arc::new(super::video_jump::VideoJumpCatalogSource::new(
            requested_path.to_path_buf(),
            chapters,
        ));
        Ok(AppRemoteVideoStreaming {
            requested_path: requested_path.to_path_buf(),
            player,
            session,
            playback,
            end_behavior,
            jump_catalog,
        })
    }

    fn remote_clockless_audio_processing(
        &self,
        normalize_gain: f64,
        start_budget_remaining: std::time::Duration,
    ) -> crate::video::clockless_transcode::ClocklessAudioProcessing {
        #[cfg(not(windows))]
        {
            let _ = start_budget_remaining;
            return crate::video::clockless_transcode::ClocklessAudioProcessing::without_vst3(
                normalize_gain,
            );
        }

        #[cfg(windows)]
        {
            use crate::video::clockless_transcode::ClocklessAudioProcessing;

            if !self.settings.vst3_enabled || self.settings.vst3_plugins.is_empty() {
                return ClocklessAudioProcessing::without_vst3(normalize_gain);
            }

            // This creates one session-owned host. Generation configs clone its Arc, while the
            // worker-side OnceLock loads the chain only once and never blocks the UI thread.
            let sample_rate = crate::video::audio::default_output_sample_rate().unwrap_or(48_000);
            ClocklessAudioProcessing::with_remote_vst3(
                normalize_gain,
                self.settings.vst3_plugins.clone(),
                sample_rate,
                remote_vst_load_budget(start_budget_remaining),
            )
        }
    }

    fn cancel_remote_video_stream_state(
        &mut self,
        code: VideoStreamErrorCode,
        message: &'static str,
    ) {
        let Some(state) = self.remote_session_ui.video_stream.take() else {
            return;
        };
        match state {
            AppRemoteVideoStreamState::Opening(opening) => {
                self.fail_remote_video_opening(opening, code, message.to_owned());
            }
            AppRemoteVideoStreamState::Starting(starting) => {
                self.fail_remote_video_starting(starting, code, message.to_owned());
            }
            AppRemoteVideoStreamState::Streaming(streaming) => {
                streaming.player.set_playing(false);
                if let Some(handle) = self.remote_session_ui.handle.as_ref() {
                    handle.clear_video_stream(Some(streaming.session.id().0));
                }
            }
        }
    }

    fn fail_remote_video_opening(
        &mut self,
        opening: AppRemoteVideoOpening,
        code: VideoStreamErrorCode,
        message: String,
    ) {
        crate::logger::log(format!(
            "[remote-video] open failed: code={code:?} detail={message:?}"
        ));
        if let Some(player) = opening.player.as_ref() {
            player.set_playing(false);
        }
        opening
            .claimed
            .complete(VideoStreamUiOutcome::Error(VideoStreamError::new(
                code, message,
            )));
    }

    fn fail_remote_video_starting(
        &mut self,
        starting: AppRemoteVideoStarting,
        code: VideoStreamErrorCode,
        message: String,
    ) {
        starting
            .streaming
            .player
            .log_remote_start_failure(&format!("{code:?}"), &message);
        starting.streaming.player.set_playing(false);
        if let Some(handle) = self.remote_session_ui.handle.as_ref() {
            handle.clear_video_stream(Some(starting.streaming.session.id().0));
        }
        starting
            .claimed
            .complete(VideoStreamUiOutcome::Error(VideoStreamError::new(
                code, message,
            )));
    }

    fn take_remote_video_streaming(&mut self) -> Option<AppRemoteVideoStreaming> {
        match self.remote_session_ui.video_stream.take()? {
            AppRemoteVideoStreamState::Opening(opening) => {
                self.remote_session_ui.video_stream =
                    Some(AppRemoteVideoStreamState::Opening(opening));
                None
            }
            AppRemoteVideoStreamState::Starting(starting) => {
                self.remote_session_ui.video_stream =
                    Some(AppRemoteVideoStreamState::Starting(starting));
                None
            }
            AppRemoteVideoStreamState::Streaming(streaming) => Some(streaming),
        }
    }

    fn poll_remote_video_streaming(&mut self, ctx: &egui::Context) {
        let Some(state) = self.remote_session_ui.video_stream.take() else {
            return;
        };
        let mut streaming = match state {
            AppRemoteVideoStreamState::Opening(opening) => {
                self.poll_remote_video_opening(opening, ctx);
                return;
            }
            AppRemoteVideoStreamState::Starting(starting) => {
                self.poll_remote_video_starting(starting, ctx);
                return;
            }
            AppRemoteVideoStreamState::Streaming(streaming) => streaming,
        };
        streaming.player.tick(ctx);
        let snapshot = streaming.playback.snapshot();
        streaming.playback.update(
            streaming.player.duration(),
            streaming.player.volume(),
            snapshot.play_intent,
        );
        let outcome = streaming.session.reconcile();
        match outcome {
            StreamReconcile::Active => {
                self.remote_session_ui.video_stream =
                    Some(AppRemoteVideoStreamState::Streaming(streaming));
            }
            StreamReconcile::Stop(reason) => {
                streaming.player.set_playing(false);
                if let Some(handle) = self.remote_session_ui.handle.as_ref() {
                    handle.clear_video_stream(Some(streaming.session.id().0));
                }
                crate::logger::log(format!("remote-stream session stopped: {reason}"));
            }
        }
    }

    fn poll_remote_video_starting(
        &mut self,
        mut starting: AppRemoteVideoStarting,
        ctx: &egui::Context,
    ) {
        if starting.claimed.ownership_response().status != SessionStatus::Active {
            self.fail_remote_video_starting(
                starting,
                VideoStreamErrorCode::SessionMismatch,
                "リモートセッションの操作権が移動しました".to_owned(),
            );
            return;
        }
        // Starting owns the same headless player as Opening/Streaming. Its tick is the sole
        // consumer of decoder/audio engine events, including SeekCompleted, FirstFrameReady,
        // and BufferReady. Omitting it leaves the actor in Seeking and eventually back-pressures
        // the otherwise-running audio pump.
        starting.streaming.player.tick(ctx);
        self.poll_owned_remote_video_starting(starting);
    }

    fn poll_owned_remote_video_starting(&mut self, mut starting: AppRemoteVideoStarting) {
        if let Some(error) = starting.budget.expired_error(starting.waiting_stage) {
            self.fail_remote_video_starting(starting, error.code, error.message);
            return;
        }
        let outcome = self.remote_video_start_outcome(&mut starting);
        self.finish_stable_remote_video_start(starting, outcome);
    }

    fn remote_video_start_outcome(
        &self,
        starting: &mut AppRemoteVideoStarting,
    ) -> Result<(StreamReconcile, RemoteVideoStartReadiness), String> {
        let streaming = &mut starting.streaming;
        let player = &streaming.player;
        if !crate::folder_tree::path_eq(player.path(), &streaming.requested_path) {
            return Err("streaming media player was replaced".to_owned());
        }
        remote_video_start_outcome_for_player(&mut streaming.session, &streaming.playback, player)
    }

    fn finish_stable_remote_video_start(
        &mut self,
        starting: AppRemoteVideoStarting,
        outcome: Result<(StreamReconcile, RemoteVideoStartReadiness), String>,
    ) {
        match outcome {
            Ok((StreamReconcile::Active, RemoteVideoStartReadiness::Stable)) => {
                if let Some(error) = starting.budget.expired_error(starting.waiting_stage) {
                    self.fail_remote_video_starting(starting, error.code, error.message);
                    return;
                }
                let published = PublishedVideoStream {
                    session: starting.streaming.session.id(),
                    generation: starting.streaming.session.access(),
                    playback: std::sync::Arc::clone(&starting.streaming.playback),
                    buffer_target_secs: starting.streaming.session.buffer_target_secs(),
                    end_behavior: starting.streaming.end_behavior.clone(),
                    jump_catalog: std::sync::Arc::clone(&starting.streaming.jump_catalog),
                };
                if let Some(handle) = self.remote_session_ui.handle.as_ref() {
                    handle.publish_video_stream(published.clone());
                }
                starting
                    .claimed
                    .complete(VideoStreamUiOutcome::Started(published));
                self.remote_session_ui.video_stream =
                    Some(AppRemoteVideoStreamState::Streaming(starting.streaming));
            }
            Ok((StreamReconcile::Active, readiness)) => {
                self.continue_or_timeout_remote_video_starting(starting, readiness);
            }
            Ok((StreamReconcile::Stop(reason), _)) | Err(reason) => {
                self.fail_remote_video_starting(starting, VideoStreamErrorCode::Failed, reason)
            }
        }
    }

    fn continue_or_timeout_remote_video_starting(
        &mut self,
        mut starting: AppRemoteVideoStarting,
        readiness: RemoteVideoStartReadiness,
    ) {
        let waiting_stage = readiness.wait_stage();
        if let Some(error) = starting.budget.expired_error(waiting_stage) {
            self.fail_remote_video_starting(starting, error.code, error.message);
        } else {
            starting.waiting_stage = waiting_stage;
            self.remote_session_ui.video_stream =
                Some(AppRemoteVideoStreamState::Starting(starting));
        }
    }

    fn poll_remote_video_opening(
        &mut self,
        mut opening: AppRemoteVideoOpening,
        ctx: &egui::Context,
    ) {
        if opening.claimed.ownership_response().status != SessionStatus::Active {
            self.fail_remote_video_opening(
                opening,
                VideoStreamErrorCode::SessionMismatch,
                "リモートセッションの操作権が移動しました".to_owned(),
            );
            return;
        }
        if let Some(error) = opening.budget.expired_error(VideoStreamStartStage::Player) {
            self.fail_remote_video_opening(opening, error.code, error.message);
            return;
        }

        if opening.player.is_none() {
            opening.player = self.try_build_remote_video_player(&opening.requested_path);
        }
        if let Some(player) = opening.player.as_mut() {
            player.tick(ctx);
            if player.error().is_none()
                && player.prep_progress().phase() != crate::video::avio_progress::prep_phase::DONE
            {
                self.activity_gate.bump();
            }
        }
        let readiness = match opening.player.as_ref() {
            Some(player)
                if !crate::folder_tree::path_eq(player.path(), &opening.requested_path) =>
            {
                Err((
                    VideoStreamErrorCode::Failed,
                    "リモート配信用 player と要求されたメディアが一致しません".to_owned(),
                ))
            }
            Some(player) => {
                if let Some(error) = player.error() {
                    Err((
                        VideoStreamErrorCode::Failed,
                        format!("要求されたメディアを再生できません: {error}"),
                    ))
                } else if let Some(inputs) = player.remote_stream_start_inputs() {
                    if !inputs.has_audio {
                        Err((
                            VideoStreamErrorCode::Failed,
                            "remote streaming requires an audio stream".to_owned(),
                        ))
                    } else {
                        Ok(Some(inputs))
                    }
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        };

        match readiness {
            Ok(Some(start_inputs)) => {
                if let Some(error) = opening.budget.expired_error(VideoStreamStartStage::Player) {
                    self.fail_remote_video_opening(opening, error.code, error.message);
                    return;
                }
                let player = opening
                    .player
                    .take()
                    .expect("ready remote video opening must own a player");
                let result = self.create_remote_video_streaming(
                    &opening.owner,
                    &opening.requested_path,
                    opening.quality,
                    start_inputs,
                    player,
                    opening.budget.remaining(),
                );
                match result {
                    Ok(streaming) => {
                        self.remote_session_ui.video_stream = Some(
                            AppRemoteVideoStreamState::Starting(AppRemoteVideoStarting {
                                claimed: opening.claimed,
                                streaming,
                                budget: opening.budget,
                                waiting_stage: VideoStreamStartStage::Seek,
                            }),
                        );
                    }
                    Err(error) => opening.claimed.complete(video_stream_ui_failure(error)),
                }
            }
            Ok(None) => {
                if let Some(error) = opening.budget.expired_error(VideoStreamStartStage::Player) {
                    self.fail_remote_video_opening(opening, error.code, error.message);
                } else {
                    self.remote_session_ui.video_stream =
                        Some(AppRemoteVideoStreamState::Opening(opening));
                }
            }
            Err((code, message)) => {
                self.fail_remote_video_opening(opening, code, message);
            }
        }
    }

    #[allow(dead_code)] // Increment 6 IPC state response consumes this snapshot.
    pub(crate) fn remote_video_streaming_status(&self) -> Option<StreamGenerationStatus> {
        self.remote_session_ui
            .video_stream
            .as_ref()
            .and_then(|state| match state {
                AppRemoteVideoStreamState::Opening(_) => None,
                AppRemoteVideoStreamState::Starting(starting) => {
                    Some(starting.streaming.session.status())
                }
                AppRemoteVideoStreamState::Streaming(streaming) => Some(streaming.session.status()),
            })
    }

    #[cfg(test)]
    pub(crate) fn replace_remote_video_player_for_test(
        &mut self,
        player: crate::video::VideoPlayer,
    ) -> bool {
        let Some(AppRemoteVideoStreamState::Opening(opening)) =
            self.remote_session_ui.video_stream.as_mut()
        else {
            return false;
        };
        opening.player = Some(Box::new(player));
        true
    }

    #[cfg(test)]
    pub(crate) fn remote_video_opening_for_test(&self) -> bool {
        matches!(
            self.remote_session_ui.video_stream,
            Some(AppRemoteVideoStreamState::Opening(_))
        )
    }

    #[cfg(test)]
    pub(crate) fn remote_video_starting_for_test(&self) -> bool {
        matches!(
            self.remote_session_ui.video_stream,
            Some(AppRemoteVideoStreamState::Starting(_))
        )
    }

    #[cfg(test)]
    pub(crate) fn remote_video_player_intent_playing_for_test(&self) -> Option<bool> {
        self.remote_session_ui
            .video_stream
            .as_ref()
            .and_then(|state| match state {
                AppRemoteVideoStreamState::Opening(opening) => opening.player.as_deref(),
                AppRemoteVideoStreamState::Starting(starting) => {
                    Some(starting.streaming.player.as_ref())
                }
                AppRemoteVideoStreamState::Streaming(streaming) => Some(streaming.player.as_ref()),
            })
            .map(crate::video::VideoPlayer::intent_playing)
    }

    #[cfg(test)]
    pub(crate) fn remote_video_player_engine_state_for_test(&self) -> Option<&'static str> {
        self.remote_session_ui
            .video_stream
            .as_ref()
            .and_then(|state| match state {
                AppRemoteVideoStreamState::Opening(opening) => opening.player.as_deref(),
                AppRemoteVideoStreamState::Starting(starting) => {
                    Some(starting.streaming.player.as_ref())
                }
                AppRemoteVideoStreamState::Streaming(streaming) => Some(streaming.player.as_ref()),
            })
            .map(crate::video::VideoPlayer::engine_state_name)
    }

    #[cfg(test)]
    pub(crate) fn remote_video_player_clock_seeking_for_test(&self) -> Option<bool> {
        self.remote_session_ui
            .video_stream
            .as_ref()
            .and_then(|state| match state {
                AppRemoteVideoStreamState::Opening(opening) => opening.player.as_deref(),
                AppRemoteVideoStreamState::Starting(starting) => {
                    Some(starting.streaming.player.as_ref())
                }
                AppRemoteVideoStreamState::Streaming(streaming) => Some(streaming.player.as_ref()),
            })
            .map(crate::video::VideoPlayer::clock_is_seeking_for_test)
    }

    #[cfg(test)]
    pub(crate) fn apply_pending_remote_video_resume_for_test(&mut self) -> bool {
        let player = match self.remote_session_ui.video_stream.as_mut() {
            Some(AppRemoteVideoStreamState::Opening(opening)) => opening.player.as_deref_mut(),
            Some(AppRemoteVideoStreamState::Starting(starting)) => {
                Some(starting.streaming.player.as_mut())
            }
            Some(AppRemoteVideoStreamState::Streaming(streaming)) => {
                Some(streaming.player.as_mut())
            }
            None => None,
        };
        let Some(player) = player else {
            return false;
        };
        player.apply_pending_remote_resume_for_test();
        true
    }

    #[allow(dead_code)] // Increment 6 routes the existing media play/pause command here.
    pub(crate) fn set_remote_video_streaming_playing(&self, playing: bool) -> bool {
        self.remote_session_ui
            .video_stream
            .as_ref()
            .is_some_and(|state| match state {
                AppRemoteVideoStreamState::Opening(_) => false,
                AppRemoteVideoStreamState::Starting(_) => false,
                AppRemoteVideoStreamState::Streaming(streaming) => {
                    let accepted = streaming.session.set_playing(playing);
                    if accepted.is_ok() {
                        streaming.playback.set_play_intent(playing);
                    }
                    accepted.is_ok()
                }
            })
    }

    #[allow(dead_code)] // Increment 6 exposes quality selection and returns the new generation.
    pub(crate) fn change_remote_video_streaming_quality(
        &mut self,
        quality: crate::video::stream::quality::QualityPreset,
        position_secs: f64,
    ) -> Result<StreamingGeneration, String> {
        let streaming = self
            .remote_session_ui
            .video_stream
            .as_mut()
            .ok_or_else(|| "remote video streaming is not active".to_owned())?;
        let AppRemoteVideoStreamState::Streaming(streaming) = streaming else {
            return Err("remote video streaming is still opening".to_owned());
        };
        streaming.session.change_quality(quality, position_secs)
    }

    fn apply_pending_remote_ui_requests(&mut self, handle: &SessionHandle, ctx: &egui::Context) {
        for pending in handle.take_pending_ui_requests() {
            match pending {
                ClaimedRemoteUiRequest::Write(pending) => {
                    self.apply_pending_remote_write(pending, ctx)
                }
                ClaimedRemoteUiRequest::BookResumeRead(pending) => {
                    let latest = self.last_book_resume.as_ref().and_then(|(path, page)| {
                        (crate::path_key::normalize(path)
                            == crate::path_key::normalize(pending.path()))
                        .then_some(*page)
                    });
                    let page = latest.or_else(|| {
                        self.book_resume_db
                            .as_ref()
                            .and_then(|db| db.get(pending.path()))
                    });
                    pending.complete(page);
                }
                ClaimedRemoteUiRequest::VideoStream(pending) => {
                    self.apply_remote_video_stream_request(pending);
                }
            }
        }
    }

    fn apply_remote_video_stream_request(&mut self, pending: ClaimedVideoStreamUiRequest) {
        let request = pending.request().clone();
        if let VideoStreamUiRequest::Stop { session } = &request {
            let outcome = self.apply_remote_video_stop(*session);
            pending.complete(outcome);
            return;
        }
        if pending.ownership_response().status != SessionStatus::Active {
            pending.complete(VideoStreamUiOutcome::Error(VideoStreamError::new(
                VideoStreamErrorCode::SessionMismatch,
                "リモートセッションの操作権が移動しました",
            )));
            return;
        }
        let outcome = match request {
            VideoStreamUiRequest::Start {
                owner,
                path,
                quality,
                budget,
            } => {
                self.begin_remote_video_start(pending, owner, path, quality.into(), budget);
                return;
            }
            VideoStreamUiRequest::Control { session, action } => {
                self.apply_remote_video_control(session, action)
            }
            VideoStreamUiRequest::Seek {
                session,
                position_secs,
            } => self.apply_remote_video_seek(session, position_secs),
            VideoStreamUiRequest::Thumbnail {
                session,
                position_secs,
            } => self.apply_remote_video_thumbnail(session, position_secs),
            VideoStreamUiRequest::Stop { .. } => unreachable!("stop is handled before ownership"),
        };
        pending.complete(outcome);
    }

    fn begin_remote_video_start(
        &mut self,
        claimed: ClaimedVideoStreamUiRequest,
        owner: RemoteSessionIdentity,
        requested_path: std::path::PathBuf,
        quality: crate::video::stream::quality::QualityPreset,
        budget: VideoStreamStartBudget,
    ) {
        if let Some(error) = budget.expired_error(VideoStreamStartStage::Ui) {
            claimed.complete(VideoStreamUiOutcome::Error(error));
            return;
        }
        if !self.settings.remote_video_streaming_enabled {
            claimed.complete(video_stream_ui_failure(
                "remote video streaming is disabled".to_owned(),
            ));
            return;
        }
        let ownership = self
            .remote_session_ui
            .handle
            .as_ref()
            .ok_or_else(|| "remote session service is unavailable".to_owned())
            .and_then(|handle| {
                handle
                    .streaming_owner(&owner)
                    .map(|_| ())
                    .map_err(|response| response.message)
            });
        if let Err(error) = ownership {
            claimed.complete(video_stream_ui_failure(error));
            return;
        }

        // A new remote start replaces the previous remote-owned player. It does not route through
        // the local folder/viewer open path, so the existing remote-session dialog remains the
        // visible surface while the headless player decodes for the stream.
        self.cancel_remote_video_stream_state(
            VideoStreamErrorCode::Failed,
            "新しい動画ストリーミング開始要求に置き換えられました",
        );
        let player = self.try_build_remote_video_player(&requested_path);
        if let Some(player) = player.as_ref() {
            player.set_playing(false);
        }
        self.remote_session_ui.video_stream =
            Some(AppRemoteVideoStreamState::Opening(AppRemoteVideoOpening {
                claimed,
                owner,
                requested_path,
                quality,
                player,
                budget,
            }));
    }

    fn apply_remote_video_control(
        &mut self,
        session: u64,
        action: VideoStreamControlAction,
    ) -> VideoStreamUiOutcome {
        let Some(mut streaming) = self.take_remote_video_streaming() else {
            return video_stream_session_mismatch();
        };
        if streaming.session.id().0 != session {
            self.remote_session_ui.video_stream =
                Some(AppRemoteVideoStreamState::Streaming(streaming));
            return video_stream_session_mismatch();
        }
        let result = match action {
            VideoStreamControlAction::Play => streaming
                .session
                .set_playing(true)
                .map(|()| streaming.playback.set_play_intent(true))
                .map_err(remote_streaming_control_error),
            VideoStreamControlAction::Pause => streaming
                .session
                .set_playing(false)
                .map(|()| streaming.playback.set_play_intent(false))
                .map_err(remote_streaming_control_error),
            VideoStreamControlAction::Volume { volume }
                if volume.is_finite() && (0.0..=1.0).contains(&volume) =>
            {
                streaming.player.set_volume(volume);
                streaming.playback.set_volume(volume);
                Ok(())
            }
            VideoStreamControlAction::Volume { .. } => Err(VideoStreamError::new(
                VideoStreamErrorCode::Failed,
                "volume must be finite and between 0 and 1",
            )),
            VideoStreamControlAction::Quality {
                quality,
                position_secs,
            } if position_secs.is_finite() && position_secs >= 0.0 => streaming
                .session
                .change_quality(quality.into(), position_secs)
                .map(|_| ())
                .map_err(|error| VideoStreamError::new(VideoStreamErrorCode::Failed, error)),
            VideoStreamControlAction::Quality { .. } => Err(VideoStreamError::new(
                VideoStreamErrorCode::Failed,
                "quality position must be finite and non-negative",
            )),
        };
        let snapshot = streaming.playback.snapshot();
        streaming.playback.update(
            streaming.player.duration(),
            snapshot.volume,
            snapshot.play_intent,
        );
        if result.is_ok()
            && let Some(handle) = self.remote_session_ui.handle.as_ref()
        {
            handle.publish_video_stream(PublishedVideoStream {
                session: streaming.session.id(),
                generation: streaming.session.access(),
                playback: std::sync::Arc::clone(&streaming.playback),
                buffer_target_secs: streaming.session.buffer_target_secs(),
                end_behavior: streaming.end_behavior.clone(),
                jump_catalog: std::sync::Arc::clone(&streaming.jump_catalog),
            });
        }
        self.remote_session_ui.video_stream = Some(AppRemoteVideoStreamState::Streaming(streaming));
        match result {
            Ok(()) => VideoStreamUiOutcome::Controlled(SessionResponse {
                status: SessionStatus::Active,
                message: "remote session active".to_owned(),
                session_id: None,
            }),
            Err(error) => VideoStreamUiOutcome::Error(error),
        }
    }

    fn apply_remote_video_seek(
        &mut self,
        session: u64,
        position_secs: f64,
    ) -> VideoStreamUiOutcome {
        let Some(mut streaming) = self.take_remote_video_streaming() else {
            return video_stream_session_mismatch();
        };
        if streaming.session.id().0 != session {
            self.remote_session_ui.video_stream =
                Some(AppRemoteVideoStreamState::Streaming(streaming));
            return video_stream_session_mismatch();
        }
        let result = streaming.session.seek(position_secs);
        if result.is_ok()
            && let Some(handle) = self.remote_session_ui.handle.as_ref()
        {
            handle.publish_video_stream(PublishedVideoStream {
                session: streaming.session.id(),
                generation: streaming.session.access(),
                playback: std::sync::Arc::clone(&streaming.playback),
                buffer_target_secs: streaming.session.buffer_target_secs(),
                end_behavior: streaming.end_behavior.clone(),
                jump_catalog: std::sync::Arc::clone(&streaming.jump_catalog),
            });
        }
        self.remote_session_ui.video_stream = Some(AppRemoteVideoStreamState::Streaming(streaming));
        result
            .map(VideoStreamUiOutcome::Seeked)
            .unwrap_or_else(video_stream_ui_failure)
    }

    fn apply_remote_video_thumbnail(
        &mut self,
        session: u64,
        position_secs: Option<f64>,
    ) -> VideoStreamUiOutcome {
        let Some(streaming) = self.take_remote_video_streaming() else {
            return video_stream_session_mismatch();
        };
        if streaming.session.id().0 != session {
            self.remote_session_ui.video_stream =
                Some(AppRemoteVideoStreamState::Streaming(streaming));
            return video_stream_session_mismatch();
        }
        let outcome = if let Some(position_secs) = position_secs {
            match streaming
                .player
                .request_remote_seek_thumbnail(position_secs)
            {
                Some(target_secs) => streaming.player.nearest_seek_thumbnail(target_secs).map_or(
                    VideoStreamUiOutcome::ThumbnailPending,
                    VideoStreamUiOutcome::ThumbnailReady,
                ),
                None => video_stream_ui_failure("invalid remote thumbnail position".to_owned()),
            }
        } else {
            streaming.player.clear_remote_seek_thumbnail();
            VideoStreamUiOutcome::ThumbnailCleared
        };
        self.remote_session_ui.video_stream = Some(AppRemoteVideoStreamState::Streaming(streaming));
        outcome
    }

    fn apply_remote_video_stop(&mut self, session: u64) -> VideoStreamUiOutcome {
        let Some(state) = self.remote_session_ui.video_stream.take() else {
            return VideoStreamUiOutcome::Stopped;
        };
        match state {
            AppRemoteVideoStreamState::Opening(opening) => {
                self.remote_session_ui.video_stream =
                    Some(AppRemoteVideoStreamState::Opening(opening));
            }
            AppRemoteVideoStreamState::Starting(starting)
                if starting.streaming.session.id().0 == session =>
            {
                self.fail_remote_video_starting(
                    starting,
                    VideoStreamErrorCode::Failed,
                    "動画ストリーミング開始中に停止されました".to_owned(),
                );
            }
            AppRemoteVideoStreamState::Starting(starting) => {
                self.remote_session_ui.video_stream =
                    Some(AppRemoteVideoStreamState::Starting(starting));
            }
            AppRemoteVideoStreamState::Streaming(streaming)
                if streaming.session.id().0 == session =>
            {
                streaming.player.set_playing(false);
                if let Some(handle) = self.remote_session_ui.handle.as_ref() {
                    handle.clear_video_stream(Some(session));
                }
            }
            AppRemoteVideoStreamState::Streaming(streaming) => {
                self.remote_session_ui.video_stream =
                    Some(AppRemoteVideoStreamState::Streaming(streaming));
            }
        }
        VideoStreamUiOutcome::Stopped
    }

    fn apply_pending_remote_write(&mut self, pending: ClaimedRemoteWrite, ctx: &egui::Context) {
        let ownership = pending.ownership_response();
        if ownership.status != SessionStatus::Active {
            pending.complete(UiWriteOutcome::Session(ownership));
            return;
        }
        if matches!(
            pending.request(),
            RemoteWriteRequest::SetBookmark { .. }
                | RemoteWriteRequest::GetItemState { .. }
                | RemoteWriteRequest::SetBookBookmarkTitle { .. }
                | RemoteWriteRequest::RemoveBookBookmark { .. }
        ) {
            self.begin_remote_bookmark_write(pending);
            return;
        }
        let response = self.apply_remote_write(pending.request());
        pending.complete(UiWriteOutcome::Write(response));
        ctx.request_repaint();
    }

    fn apply_remote_write(&mut self, request: &RemoteWriteRequest) -> RemoteWriteResponse {
        match request {
            RemoteWriteRequest::SetSpread {
                address,
                spread_mode,
                reading_direction,
            } => self.persist_remote_spread(address, *spread_mode, *reading_direction),
            RemoteWriteRequest::RecordReadingProgress {
                address,
                context_address,
                page_index,
                page_number,
                page_count,
                record_resume,
                record_history,
            } => self.persist_remote_reading_progress(
                address,
                context_address,
                *page_index,
                *page_number,
                *page_count,
                *record_resume,
                *record_history,
            ),
            RemoteWriteRequest::SetRating { address, stars } => {
                self.persist_remote_rating(address, *stars)
            }
            RemoteWriteRequest::SetAdjustment {
                address,
                scope,
                values,
            } => self.persist_remote_adjustment(address, *scope, values),
            RemoteWriteRequest::GetAdjustmentState { address } => {
                self.remote_adjustment_state(address)
            }
            RemoteWriteRequest::SetViewTrim {
                address: _,
                context_address,
                state,
            } => self.persist_remote_view_trim(context_address, state),
            RemoteWriteRequest::GetViewTrimState {
                address: _,
                context_address,
            } => self.remote_view_trim_state(context_address),
            RemoteWriteRequest::SetSortOrder {
                scope: _,
                sort_order,
            } => self.persist_remote_sort_order(sort_order),
            RemoteWriteRequest::SetBookmark { .. }
            | RemoteWriteRequest::GetItemState { .. }
            | RemoteWriteRequest::ListBookBookmarks { .. }
            | RemoteWriteRequest::SetBookBookmarkTitle { .. }
            | RemoteWriteRequest::RemoveBookBookmark { .. } => write_error(
                RemoteWriteErrorCode::Internal,
                "ブックマーク要求の非同期経路が使われませんでした",
            ),
        }
    }

    fn begin_remote_bookmark_write(&mut self, pending: ClaimedRemoteWrite) {
        let request = pending.request().clone();
        let (address, context_address, page_index, action) = match request {
            RemoteWriteRequest::SetBookmark {
                address,
                context_address,
                page_index,
                bookmarked,
            } => (
                address,
                context_address,
                page_index,
                RemoteBookmarkRequestAction::SetPresence {
                    present: bookmarked,
                },
            ),
            RemoteWriteRequest::GetItemState {
                address,
                context_address,
                page_index,
                bookmark_supported,
            } => (
                address,
                context_address,
                page_index,
                RemoteBookmarkRequestAction::ReadState { bookmark_supported },
            ),
            RemoteWriteRequest::SetBookBookmarkTitle {
                address,
                context_address,
                page_index,
                id,
                title,
            } => (
                address,
                context_address,
                page_index,
                RemoteBookmarkRequestAction::SetTitle { id, title },
            ),
            RemoteWriteRequest::RemoveBookBookmark {
                address,
                context_address,
                page_index,
                id,
            } => (
                address,
                context_address,
                page_index,
                RemoteBookmarkRequestAction::Remove { id },
            ),
            _ => unreachable!("only bookmark-backed requests are deferred"),
        };
        let rating = if let RemoteBookmarkRequestAction::ReadState { bookmark_supported } = &action
        {
            let rating = match remote_rating_target(&self.settings, &address) {
                Ok(target) => {
                    let Some(db) = self.rating_db.as_ref() else {
                        pending.complete(UiWriteOutcome::Write(write_error(
                            RemoteWriteErrorCode::PersistenceFailed,
                            "レーティング DB を利用できません",
                        )));
                        return;
                    };
                    db.get(&target.key)
                }
                Err(error) => {
                    pending.complete(UiWriteOutcome::Write(RemoteWriteResponse::Error(error)));
                    return;
                }
            };
            if !bookmark_supported {
                pending.complete(UiWriteOutcome::Write(RemoteWriteResponse::Success(
                    RemoteWriteResult::item_state(RemoteItemState {
                        rating,
                        bookmark_supported: false,
                        bookmarked: false,
                    }),
                )));
                return;
            }
            Some(rating)
        } else {
            None
        };
        let draft =
            match remote_bookmark_draft(&self.settings, &address, &context_address, page_index) {
                Ok(draft) => draft,
                Err(error) => {
                    pending.complete(UiWriteOutcome::Write(RemoteWriteResponse::Error(error)));
                    return;
                }
            };
        let request_id = self.next_book_bookmark_request_id();
        let Some(service) = self.book_bookmark_service.as_ref() else {
            pending.complete(UiWriteOutcome::Write(write_error(
                RemoteWriteErrorCode::PersistenceFailed,
                "ブックマーク DB を利用できません",
            )));
            return;
        };
        let container_path = draft.container_path.clone();
        let kind = match action {
            RemoteBookmarkRequestAction::ReadState { .. } => {
                service.get_page_presence(request_id, draft);
                PendingRemoteBookmarkWriteKind::ReadState {
                    rating: rating.expect("read state computes rating"),
                }
            }
            RemoteBookmarkRequestAction::SetPresence { present } => {
                service.set_page_presence(request_id, draft, present);
                PendingRemoteBookmarkWriteKind::SetPresence
            }
            RemoteBookmarkRequestAction::SetTitle { id, title } => {
                service.set_title_in_container(request_id, id, title, container_path.clone());
                PendingRemoteBookmarkWriteKind::SetTitle
            }
            RemoteBookmarkRequestAction::Remove { id } => {
                service.remove_in_container(request_id, id, container_path.clone());
                PendingRemoteBookmarkWriteKind::Remove
            }
        };
        self.book_bookmark_pending_requests.insert(request_id);
        self.remote_session_ui.pending_bookmark_writes.insert(
            request_id,
            PendingRemoteBookmarkWrite {
                claimed: pending,
                kind,
                container_path,
            },
        );
    }

    fn persist_remote_spread(
        &mut self,
        address: &mimageviewer_ipc::RemoteAddress,
        spread_mode: RemoteSpreadMode,
        reading_direction: RemoteReadingDirection,
    ) -> RemoteWriteResponse {
        let key = match remote_spread_key(address) {
            Ok(key) => key,
            Err(error) => return RemoteWriteResponse::Error(error),
        };
        let mode = core_spread_mode(spread_mode);
        let direction = core_reading_direction(mode, reading_direction);
        let defaults = (
            self.settings.default_spread_mode,
            self.settings.default_reading_flow,
            self.settings.default_reading_direction,
        );
        let Some(db) = self.spread_db.as_mut() else {
            return write_error(
                RemoteWriteErrorCode::PersistenceFailed,
                "spread.db を開けなかったため保存できません",
            );
        };
        let started = std::time::Instant::now();
        match db.set_mode_and_direction(
            &key.exact,
            key.fallback.as_deref(),
            mode,
            direction,
            defaults,
        ) {
            Ok(()) => {
                crate::logger::log(format!(
                    "remote_ipc: UI write applied kind=set_spread duration_ms={:.1}",
                    started.elapsed().as_secs_f64() * 1000.0
                ));
                RemoteWriteResponse::Success(RemoteWriteResult::applied())
            }
            Err(error) => {
                crate::logger::log(format!(
                    "remote_ipc: UI write failed kind=set_spread duration_ms={:.1} error={error}",
                    started.elapsed().as_secs_f64() * 1000.0
                ));
                write_error(
                    RemoteWriteErrorCode::PersistenceFailed,
                    "spread.db への保存に失敗しました",
                )
            }
        }
    }

    fn persist_remote_reading_progress(
        &mut self,
        address: &mimageviewer_ipc::RemoteAddress,
        context_address: &mimageviewer_ipc::RemoteAddress,
        page_index: u32,
        page_number: u32,
        page_count: u32,
        record_resume: bool,
        record_history: bool,
    ) -> RemoteWriteResponse {
        let target = match remote_reading_target(address, context_address) {
            Ok(target) => target,
            Err(error) => return RemoteWriteResponse::Error(error),
        };
        if record_resume && (self.book_resume_db.is_none() || self.book_resume_writer.is_none()) {
            return write_error(
                RemoteWriteErrorCode::PersistenceFailed,
                "読書位置 DB を利用できません",
            );
        }
        let page_index = page_index as usize;
        if record_resume {
            if let Some(writer) = self.book_resume_writer.as_ref() {
                writer.record(&target.container_path, page_index);
            }
            self.last_book_resume = Some((target.container_path.clone(), page_index));
        }

        if self.settings.reading_history_enabled
            && record_history
            && self.reading_history_db.is_some()
            && self.reading_history_writer.is_some()
        {
            let key = crate::path_key::normalize_keep_drive(&target.container_path);
            let now = std::time::Instant::now();
            let throttled =
                self.last_reading_history_touch
                    .as_ref()
                    .is_some_and(|(last_key, at)| {
                        last_key == &key
                            && now.duration_since(*at)
                                < crate::reading_history_db::READING_HISTORY_TOUCH_THROTTLE
                    });
            if !throttled {
                let title = target
                    .container_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| target.container_path.to_string_lossy().into_owned());
                let entry = crate::reading_history_db::ReadingHistoryEntry::new(
                    target.container_path,
                    target.history_kind,
                    None,
                    title,
                    record_resume.then_some(i64::from(page_number)),
                    record_resume.then_some(i64::from(page_count)),
                );
                if let Some(writer) = self.reading_history_writer.as_ref() {
                    writer.record(entry, self.settings.reading_history_limit);
                }
                self.last_reading_history_touch = Some((key, now));
            }
        }
        RemoteWriteResponse::Success(RemoteWriteResult::applied())
    }

    fn persist_remote_rating(
        &mut self,
        address: &mimageviewer_ipc::RemoteAddress,
        stars: u8,
    ) -> RemoteWriteResponse {
        let target = match remote_rating_target(&self.settings, address) {
            Ok(target) => target,
            Err(error) => return RemoteWriteResponse::Error(error),
        };
        if let Err(error) = self.write_user_rating_shared(&target.key, stars, Some(&target.meta)) {
            crate::logger::log(format!("remote_ipc: rating write failed: {error}"));
            return write_error(
                RemoteWriteErrorCode::PersistenceFailed,
                "rating.db への保存に失敗しました",
            );
        }
        self.invalidate_rating_counts_cache();
        self.schedule_current_smart_folder_metadata_refresh(
            crate::app::smart_folder::SmartFolderMetadataDependency::Rating,
        );
        if self.settings.write_rating_to_xmp
            && let Some(path) = target.xmp_target
        {
            self.ensure_rating_write_handle();
            if let Some(handle) = self.rating_write_handle.as_ref() {
                handle.submit(crate::rating_write_worker::RatingWriteJob {
                    path,
                    rating: (stars != 0).then_some(stars),
                });
            }
        }
        RemoteWriteResponse::Success(RemoteWriteResult::applied())
    }

    fn remote_adjustment_state(
        &self,
        address: &mimageviewer_ipc::RemoteAddress,
    ) -> RemoteWriteResponse {
        let target = match remote_adjustment_target(&self.settings, address) {
            Ok(target) => target,
            Err(error) => return RemoteWriteResponse::Error(error),
        };
        if self.adjustment_db.is_none() {
            return write_error(
                RemoteWriteErrorCode::PersistenceFailed,
                "画像補正 DB を利用できません",
            );
        }
        RemoteWriteResponse::Success(RemoteWriteResult::adjustment_state(
            self.remote_adjustment_state_for_target(&target),
        ))
    }

    fn remote_adjustment_state_for_target(
        &self,
        target: &crate::app::PageAdjustmentTarget,
    ) -> RemoteAdjustmentState {
        let page = self.stored_page_params_for_target(target);
        let standard = self.adjustment_standard_params_for_target(target);
        let effective = page.clone().unwrap_or_else(|| standard.clone());
        let ai_mode = self.settings.ai_feature_mode;
        let denoise_label = effective
            .denoise_model
            .as_deref()
            .and_then(crate::ai::ModelKind::from_str)
            .map(|model| model.display_label())
            .unwrap_or(if effective.denoise_model.is_some() {
                "不明"
            } else {
                "なし"
            })
            .to_string();
        RemoteAdjustmentState {
            effective_values: super::remote_adjustment_values(&effective),
            standard_values: super::remote_adjustment_values(&standard),
            selected_scope: if target.compiled_book || page.is_some() {
                RemoteAdjustmentScope::Page
            } else {
                RemoteAdjustmentScope::Standard
            },
            has_page_override: page.is_some(),
            standard_label: self.adjustment_standard_label_for_target(target),
            standard_available: !target.compiled_book,
            colorize_preset_slots: std::array::from_fn(|index| {
                self.settings.colorize_preset_slots.slots[index]
                    .as_ref()
                    .map(super::remote_colorize_params)
            }),
            ai_model_catalog: remote_ai_model_catalog(ai_mode),
            post_filter: super::remote_post_filter_state(effective.post_filter),
            effective_ai_enabled: crate::ai::final_pipeline::effective_upscale_request(
                ai_mode, &effective,
            )
            .is_some()
                || crate::ai::final_pipeline::effective_denoise_request(ai_mode, &effective)
                    .is_some(),
            read_only: RemoteAdjustmentReadOnlyState {
                upscale_label: crate::adjustment::upscale_model_label(
                    effective.upscale_model.as_deref(),
                )
                .to_string(),
                denoise_label,
            },
        }
    }

    fn persist_remote_adjustment(
        &mut self,
        address: &mimageviewer_ipc::RemoteAddress,
        scope: RemoteAdjustmentScope,
        values: &mimageviewer_ipc::RemoteAdjustmentValues,
    ) -> RemoteWriteResponse {
        let target = match remote_adjustment_target(&self.settings, address) {
            Ok(target) => target,
            Err(error) => return RemoteWriteResponse::Error(error),
        };
        if self.adjustment_db.is_none() {
            return write_error(
                RemoteWriteErrorCode::PersistenceFailed,
                "画像補正 DB を利用できません",
            );
        }
        if scope == RemoteAdjustmentScope::Standard && target.compiled_book {
            return write_error(
                RemoteWriteErrorCode::Unsupported,
                "製本ページの標準は無補正固定です",
            );
        }
        let base = match scope {
            RemoteAdjustmentScope::Page => self.effective_params_for_target(&target),
            RemoteAdjustmentScope::Standard => self.adjustment_standard_params_for_target(&target),
        };
        let params = match super::apply_remote_adjustment_values(base, values) {
            Ok(params) => params,
            Err(message) => return write_error(RemoteWriteErrorCode::BadRequest, message),
        };
        let undo_target = target.clone();
        self.capture_adjust_full_for_target(
            undo_target,
            "リモート画像補正".to_string(),
            |app| match scope {
                RemoteAdjustmentScope::Page => {
                    app.restore_page_params_for_target(&target, Some(params));
                }
                RemoteAdjustmentScope::Standard => {
                    match app.adjustment_standard_scope_for_target(&target) {
                        crate::app::AdjustmentStandardScope::Favorite(id) => {
                            app.set_favorite_default(id, params)
                        }
                        crate::app::AdjustmentStandardScope::Global => {
                            app.copy_params_to_global(params)
                        }
                    }
                    if app.stored_page_params_for_target(&target).is_some() {
                        app.restore_page_params_for_target(&target, None);
                    }
                }
            },
        );
        RemoteWriteResponse::Success(RemoteWriteResult::adjustment_state(
            self.remote_adjustment_state_for_target(&target),
        ))
    }

    fn remote_view_trim_state(
        &self,
        context_address: &mimageviewer_ipc::RemoteAddress,
    ) -> RemoteWriteResponse {
        let key = match remote_spread_key(context_address) {
            Ok(key) => key,
            Err(error) => return RemoteWriteResponse::Error(error),
        };
        let Some(db) = self.view_trim_db.as_ref() else {
            return write_error(
                RemoteWriteErrorCode::PersistenceFailed,
                "view_trim.db を利用できません",
            );
        };
        let mut state = db
            .get_book_state(&key.exact)
            .or_else(|| {
                key.fallback
                    .as_deref()
                    .and_then(|fallback| db.get_book_state(fallback))
            })
            .unwrap_or_default();
        let legacy_margin_fit = matches!(
            self.settings.fullscreen_fit_mode,
            crate::settings::FullscreenFitMode::MarginFit
        ) || self.settings.margin_fit_enabled;
        state.apply_mode = crate::view_trim::effective_view_trim_base_apply_mode(
            state.apply_mode,
            legacy_margin_fit,
        );
        state.book_settings = state.book_settings.clamped();
        match serde_json::to_value(state) {
            Ok(state) => RemoteWriteResponse::Success(RemoteWriteResult::view_trim_state(state)),
            Err(error) => {
                crate::logger::log(format!(
                    "remote_ipc: view trim state serialization failed: {error}"
                ));
                write_error(
                    RemoteWriteErrorCode::Internal,
                    "表示トリム設定を返せませんでした",
                )
            }
        }
    }

    fn persist_remote_view_trim(
        &mut self,
        context_address: &mimageviewer_ipc::RemoteAddress,
        value: &serde_json::Value,
    ) -> RemoteWriteResponse {
        let mut state = match super::normalize_remote_view_trim_state(value) {
            Ok(state) => state,
            Err(message) => return write_error(RemoteWriteErrorCode::BadRequest, message),
        };
        let key = match remote_spread_key(context_address) {
            Ok(key) => key,
            Err(error) => return RemoteWriteResponse::Error(error),
        };
        let Some(db) = self.view_trim_db.as_ref() else {
            return write_error(
                RemoteWriteErrorCode::PersistenceFailed,
                "view_trim.db を利用できません",
            );
        };
        let previous_separate = db
            .get_book_state(&key.exact)
            .or_else(|| {
                key.fallback
                    .as_deref()
                    .and_then(|fallback| db.get_book_state(fallback))
            })
            .unwrap_or_default()
            .book_settings
            .spread_separate;
        if previous_separate != state.book_settings.spread_separate {
            let separate = state.book_settings.spread_separate;
            state.book_settings.spread_separate = previous_separate;
            state.book_settings = state.book_settings.with_spread_separate(separate);
        }
        if let Err(error) = db.set_book_state(&key.exact, state) {
            crate::logger::log(format!("remote_ipc: view trim write failed: {error}"));
            return write_error(
                RemoteWriteErrorCode::PersistenceFailed,
                "view_trim.db への保存に失敗しました",
            );
        }
        if matches!(
            self.settings.fullscreen_fit_mode,
            crate::settings::FullscreenFitMode::MarginFit
        ) || self.settings.margin_fit_enabled
        {
            self.settings.fullscreen_fit_mode = crate::settings::FullscreenFitMode::Page;
            self.settings.margin_fit_enabled = false;
            if !self.settings.save_checked() {
                return write_error(
                    RemoteWriteErrorCode::PersistenceFailed,
                    "旧余白カット設定の移行に失敗しました",
                );
            }
        }
        match serde_json::to_value(state) {
            Ok(state) => RemoteWriteResponse::Success(RemoteWriteResult::view_trim_state(state)),
            Err(error) => {
                crate::logger::log(format!(
                    "remote_ipc: persisted view trim state serialization failed: {error}"
                ));
                write_error(
                    RemoteWriteErrorCode::Internal,
                    "保存した表示トリム設定を返せませんでした",
                )
            }
        }
    }

    fn persist_remote_sort_order(&mut self, value: &str) -> RemoteWriteResponse {
        let sort_order = match super::parse_sort_order_wire(value) {
            Ok(sort_order) => sort_order,
            Err(message) => return write_error(RemoteWriteErrorCode::BadRequest, message),
        };
        let previous = self.settings.sort_order;
        self.settings.sort_order = sort_order;
        if !self.settings.save_checked() {
            self.settings.sort_order = previous;
            return write_error(
                RemoteWriteErrorCode::PersistenceFailed,
                "並べ替え設定を保存できませんでした",
            );
        }
        RemoteWriteResponse::Success(RemoteWriteResult::sort_state(
            super::remote_grid_sort_state(sort_order, None),
        ))
    }

    pub(crate) fn finish_remote_bookmark_event(
        &mut self,
        event: &crate::book_bookmarks::BookBookmarkEvent,
    ) -> bool {
        let request_id = match event {
            crate::book_bookmarks::BookBookmarkEvent::PagePresenceRead { request_id, .. }
            | crate::book_bookmarks::BookBookmarkEvent::PagePresenceSet { request_id, .. }
            | crate::book_bookmarks::BookBookmarkEvent::Removed { request_id, .. }
            | crate::book_bookmarks::BookBookmarkEvent::TitleUpdated { request_id, .. } => {
                *request_id
            }
            _ => return false,
        };
        let Some(pending) = self
            .remote_session_ui
            .pending_bookmark_writes
            .remove(&request_id)
        else {
            return matches!(
                event,
                crate::book_bookmarks::BookBookmarkEvent::PagePresenceRead { .. }
                    | crate::book_bookmarks::BookBookmarkEvent::PagePresenceSet { .. }
            );
        };
        self.book_bookmark_pending_requests.remove(&request_id);
        let (response, changed) = match (&pending.kind, event) {
            (
                PendingRemoteBookmarkWriteKind::ReadState { rating },
                crate::book_bookmarks::BookBookmarkEvent::PagePresenceRead {
                    result: Ok(bookmarked),
                    ..
                },
            ) => (
                RemoteWriteResponse::Success(RemoteWriteResult::item_state(RemoteItemState {
                    rating: *rating,
                    bookmark_supported: true,
                    bookmarked: *bookmarked,
                })),
                false,
            ),
            (
                PendingRemoteBookmarkWriteKind::SetPresence,
                crate::book_bookmarks::BookBookmarkEvent::PagePresenceSet { result: Ok(_), .. },
            )
            | (
                PendingRemoteBookmarkWriteKind::SetTitle,
                crate::book_bookmarks::BookBookmarkEvent::TitleUpdated { result: Ok(_), .. },
            )
            | (
                PendingRemoteBookmarkWriteKind::Remove,
                crate::book_bookmarks::BookBookmarkEvent::Removed { result: Ok(_), .. },
            ) => (
                RemoteWriteResponse::Success(RemoteWriteResult::applied()),
                true,
            ),
            (
                _,
                crate::book_bookmarks::BookBookmarkEvent::PagePresenceRead {
                    result: Err(error),
                    ..
                },
            )
            | (
                _,
                crate::book_bookmarks::BookBookmarkEvent::PagePresenceSet {
                    result: Err(error),
                    ..
                },
            )
            | (
                _,
                crate::book_bookmarks::BookBookmarkEvent::Removed {
                    result: Err(error), ..
                },
            )
            | (
                _,
                crate::book_bookmarks::BookBookmarkEvent::TitleUpdated {
                    result: Err(error), ..
                },
            ) => {
                crate::logger::log(format!(
                    "remote_ipc: bookmark service request failed request_id={request_id} error={error}"
                ));
                (
                    write_error(
                        RemoteWriteErrorCode::PersistenceFailed,
                        "ブックマーク DB の操作に失敗しました",
                    ),
                    false,
                )
            }
            _ => (
                write_error(
                    RemoteWriteErrorCode::Internal,
                    "ブックマーク応答の種別が一致しません",
                ),
                false,
            ),
        };
        if changed {
            self.invalidate_current_book_bookmarks_for_container(&pending.container_path);
            self.notify_bookmarks_changed();
        }
        pending.claimed.complete(UiWriteOutcome::Write(response));
        true
    }

    pub(crate) fn fail_remote_bookmark_writes(&mut self) {
        for (_, pending) in self.remote_session_ui.pending_bookmark_writes.drain() {
            pending.claimed.complete(UiWriteOutcome::Write(write_error(
                RemoteWriteErrorCode::PersistenceFailed,
                "ブックマーク service が停止しました",
            )));
        }
    }

    pub(crate) fn show_remote_session_dialog(&mut self, ctx: &egui::Context) {
        let session_snapshot = self
            .remote_session_ui
            .handle
            .as_ref()
            .map(SessionHandle::snapshot);
        let Some(session_snapshot) = session_snapshot else {
            return;
        };
        let phase = session_snapshot.phase;
        let Some(snapshot) = session_snapshot.active else {
            return;
        };
        let streaming_source = self
            .remote_session_ui
            .video_stream
            .as_ref()
            .map(|state| match state {
                AppRemoteVideoStreamState::Opening(opening) => &opening.requested_path,
                AppRemoteVideoStreamState::Starting(starting) => &starting.streaming.requested_path,
                AppRemoteVideoStreamState::Streaming(streaming) => &streaming.requested_path,
            })
            .map(|path| {
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            });
        let streaming_audio_status =
            self.remote_session_ui
                .video_stream
                .as_ref()
                .and_then(|state| match state {
                    AppRemoteVideoStreamState::Opening(_) => None,
                    AppRemoteVideoStreamState::Starting(starting) => {
                        Some(starting.streaming.session.access().audio_status())
                    }
                    AppRemoteVideoStreamState::Streaming(streaming) => {
                        Some(streaming.session.access().audio_status())
                    }
                });
        let mut disconnect = false;
        egui::Modal::new(egui::Id::new("remote_session_modal")).show(ctx, |ui| {
            ui.heading(match phase {
                super::session::RemoteControlPhase::AcquiringRemote => "リモート接続の準備中",
                super::session::RemoteControlPhase::DrainingRemote => {
                    "リモート接続を終了しています"
                }
                super::session::RemoteControlPhase::RemoteActive => "リモート接続中",
                super::session::RemoteControlPhase::Local => return,
            });
            ui.add_space(6.0);
            show_connection_summary(ui, &snapshot);
            ui.separator();
            if let Some(source) = streaming_source.as_deref() {
                ui.label(egui::RichText::new(format!("リモートへ配信中: {source}")).strong());
            }
            if let Some(status) = streaming_audio_status.as_ref() {
                if let Some(warning) = status.warning.as_deref() {
                    ui.colored_label(egui::Color32::from_rgb(255, 190, 90), warning);
                } else if status.active {
                    ui.label(format!(
                        "VST3: {} 個を配信音声へ適用中",
                        status.active_slots
                    ));
                }
            }
            ui.label(format!(
                "現在の処理: {}",
                snapshot.current_operation.as_deref().unwrap_or("待機中")
            ));
            ui.label(format!(
                "要求 {} 件 / 完了 {} 件 / 失敗 {} 件",
                snapshot.request_count, snapshot.completed_count, snapshot.failed_count
            ));
            ui.label(format!(
                "処理中 {} 件 / 待機 {} 件",
                snapshot.running_count, snapshot.queued_count
            ));
            ui.add_space(8.0);
            ui.label("切断するとローカル操作へ戻ります。リモートは次の操作時に再接続できます。");
            ui.add_space(8.0);
            if ui
                .add_enabled(
                    phase != super::session::RemoteControlPhase::DrainingRemote,
                    egui::Button::new("切断する").min_size(egui::vec2(160.0, 34.0)),
                )
                .clicked()
            {
                disconnect = true;
            }
        });
        if disconnect {
            if let Some(handle) = self.remote_session_ui.handle.as_ref() {
                handle.local_disconnect();
            }
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    pub(crate) fn show_remote_connection_dialog(&mut self, ctx: &egui::Context) {
        let dialog_open = self.remote_session_ui.connection_dialog.is_some();
        let session_logout_result = match &self.remote_session_ui.session_logout {
            RemoteSessionLogoutState::Running { receiver } => match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(
                    "すべての端末のログアウト結果を受け取れませんでした".to_owned(),
                )),
            },
            RemoteSessionLogoutState::Idle
            | RemoteSessionLogoutState::Confirming
            | RemoteSessionLogoutState::Finished(_) => None,
        };
        if let Some(result) = session_logout_result {
            // 失敗はダイアログが開いていたかに関わらず、完了した時点で 1 度だけ残す。
            // 署名鍵が回っていないという事実は、利用者がその瞬間を見ていなくても記録が要る。
            if let Err(error) = &result {
                crate::logger::log(format!("remote_ipc: logout-all failed: {error}"));
            }
            let disconnect_local = self.remote_session_ui.session_logout.finish(result);
            if disconnect_local && let Some(handle) = self.remote_session_ui.handle.as_ref() {
                handle.local_disconnect();
            }
            if !dialog_open {
                // 誰も見ていないうちに終わった場合は、閉じたときと同じ後始末をここで済ませる
                // (成功表示を溜め込まない)。失敗は「もう一度試す」と一緒に残す。
                self.remote_session_ui.session_logout.dialog_closed();
            }
        }
        if matches!(
            &self.remote_session_ui.session_logout,
            RemoteSessionLogoutState::Running { .. }
        ) {
            // The receiver is core-owned, so keep polling even while the dialog is closed.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }

        let Some(mut dialog) = self.remote_session_ui.connection_dialog.take() else {
            return;
        };
        let pin_update_result = match &dialog.pin_editor {
            RemotePinEditorState::Saving { receiver } => match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("PIN の設定結果を受け取れませんでした".to_owned()))
                }
            },
            RemotePinEditorState::Hidden | RemotePinEditorState::Editing { .. } => None,
        };
        if let Some(result) = pin_update_result {
            dialog.pin_editor = match result {
                Ok(()) => RemotePinEditorState::Hidden,
                Err(error) => RemotePinEditorState::Editing {
                    input: String::new(),
                    error: Some(error),
                    request_focus: true,
                },
            };
        }
        let tailscale_serve_result = match &dialog.tailscale_serve_setup {
            RemoteTailscaleServeSetupState::Running { receiver } => match receiver.try_recv() {
                Ok(result) => Some(result.map_err(|error| error.user_message())),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(
                    "tailscale serve の設定結果を受け取れませんでした".to_owned(),
                )),
            },
            RemoteTailscaleServeSetupState::Idle | RemoteTailscaleServeSetupState::Finished(_) => {
                None
            }
        };
        if let Some(result) = tailscale_serve_result {
            dialog.tailscale_serve_setup = RemoteTailscaleServeSetupState::Finished(result);
        }
        let service_control = self.remote_session_ui.remote_service_control.clone();
        let pin_configured = service_control
            .as_ref()
            .is_some_and(super::RemoteServiceControl::pin_configured);
        let service_enabled = self.settings.remote_service_enabled;
        // 案内するコマンドは、実際に子プロセスへ渡している待受ポートから組み立てる。
        // 既定値を UI 側で持つと、表示と実行で出所が 2 つになる。
        let serve_port = service_control
            .as_ref()
            .map_or(mimageviewer_ipc::DEFAULT_REMOTE_PORT, |control| {
                control.port()
            });
        if !pin_configured && matches!(&dialog.pin_editor, RemotePinEditorState::Hidden) {
            dialog.pin_editor = RemotePinEditorState::Editing {
                input: String::new(),
                error: None,
                request_focus: true,
            };
        }
        let snapshot = self
            .remote_session_ui
            .handle
            .as_ref()
            .map(SessionHandle::snapshot);
        let remote_web_connected = snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.remote_web_connected);
        let remote_client_active = snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.active.is_some());
        let info = snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.remote_web.clone());
        if info
            .as_ref()
            .is_some_and(|info| info.tailscale_serve == RemoteWebFeatureStatus::Configured)
        {
            dialog.tailscale_serve_setup = RemoteTailscaleServeSetupState::Idle;
        }
        let service_diagnostic = self
            .remote_session_ui
            .remote_service_status
            .as_ref()
            .map(super::RemoteServiceStatus::snapshot)
            .unwrap_or(super::service::RemoteServiceDiagnostic::Stopped);
        let accepting = remote_web_connected && info.is_some();
        let key_expiry_display = remote_key_expiry_display(
            info.as_ref()
                .and_then(|info| info.tailscale_key_expiry_unix_seconds),
            current_unix_seconds(),
        );
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let mut open = true;
        let mut requested_enabled = None;
        let mut close_requested = false;
        let mut begin_tailscale_serve_setup = false;
        let mut begin_session_logout = false;
        egui::Window::new("リモート接続")
            .id(egui::Id::new("remote_connection_dialog"))
            .collapsible(false)
            .resizable(false)
            .default_width(560.0)
            .open(&mut open)
            .show(ctx, |ui| {
                let mut begin_pin_save = false;
                ui.horizontal(|ui| {
                    ui.label("PIN:");
                    ui.strong(if pin_configured {
                        "設定済み"
                    } else {
                        "未設定"
                    });
                    if pin_configured
                        && matches!(&dialog.pin_editor, RemotePinEditorState::Hidden)
                        && ui.button("PIN を変更").clicked()
                    {
                        dialog.pin_editor = RemotePinEditorState::Editing {
                            input: String::new(),
                            error: None,
                            request_focus: true,
                        };
                    }
                });
                match &mut dialog.pin_editor {
                    RemotePinEditorState::Hidden => {}
                    RemotePinEditorState::Editing {
                        input,
                        error,
                        request_focus,
                    } => {
                        ui.horizontal(|ui| {
                            let response = crate::ime_focus::add_singleline_sensitive(
                                ui,
                                input,
                                None,
                                |edit| {
                                    edit.password(true)
                                        .desired_width(260.0)
                                        .hint_text("6文字以上（半角英数字・記号、空白なし）")
                                        .id(egui::Id::new("remote_connection_pin"))
                                },
                            );
                            if *request_focus {
                                response.request_focus();
                                *request_focus = false;
                            }
                            if ui.button("設定").clicked() {
                                begin_pin_save = true;
                            }
                        });
                        if let Some(error) = error {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        }
                    }
                    RemotePinEditorState::Saving { .. } => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("PIN を保存しています...");
                        });
                    }
                }
                if begin_pin_save {
                    let pin = match &mut dialog.pin_editor {
                        RemotePinEditorState::Editing { input, .. } => std::mem::take(input),
                        RemotePinEditorState::Hidden | RemotePinEditorState::Saving { .. } => {
                            String::new()
                        }
                    };
                    match mimageviewer_ipc::validate_pin(&pin) {
                        Err(error) => {
                            dialog.pin_editor = RemotePinEditorState::Editing {
                                input: pin,
                                error: Some(error),
                                request_focus: true,
                            };
                        }
                        Ok(()) => {
                            if let Some(control) = service_control.as_ref() {
                                match control.set_pin(pin) {
                                    Ok(receiver) => {
                                        dialog.pin_editor =
                                            RemotePinEditorState::Saving { receiver };
                                    }
                                    Err(error) => {
                                        dialog.pin_editor = RemotePinEditorState::Editing {
                                            input: String::new(),
                                            error: Some(error),
                                            request_focus: true,
                                        };
                                    }
                                }
                            } else {
                                dialog.pin_editor = RemotePinEditorState::Editing {
                                    input: pin,
                                    error: Some("PIN の設定を開始できませんでした".to_owned()),
                                    request_focus: true,
                                };
                            }
                        }
                    }
                }
                ui.small("PIN の設定・変更後、有効なリモート接続は自動的に再起動します。");
                ui.small("署名鍵も更新されるため、接続中の端末では PIN の再入力が必要になります。");
                if pin_configured {
                    ui.add_space(4.0);
                    match &self.remote_session_ui.session_logout {
                        RemoteSessionLogoutState::Idle => {
                            if ui.button("すべての端末をログアウト").clicked() {
                                self.remote_session_ui
                                    .session_logout
                                    .request_confirmation();
                            }
                        }
                        RemoteSessionLogoutState::Confirming => {}
                        RemoteSessionLogoutState::Running { .. } => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("すべての端末をログアウトしています...");
                            });
                        }
                        RemoteSessionLogoutState::Finished(Ok(())) => {
                            ui.label("すべての端末をログアウトしました。PIN は変わっていません。");
                        }
                        RemoteSessionLogoutState::Finished(Err(error)) => {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                            if ui.button("もう一度試す").clicked() {
                                self.remote_session_ui
                                    .session_logout
                                    .request_confirmation();
                            }
                        }
                    }
                }
                ui.add_space(6.0);
                if remote_enable_warning_visible(service_enabled) {
                    let error_color = ui.visuals().error_fg_color;
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.label(
                            egui::RichText::new(REMOTE_ENABLE_WARNING_PREFIX).color(error_color),
                        );
                        ui.label(
                            egui::RichText::new(REMOTE_ENABLE_WARNING_EMPHASIS)
                                .strong()
                                .color(error_color),
                        );
                        ui.label(
                            egui::RichText::new(REMOTE_ENABLE_WARNING_SUFFIX).color(error_color),
                        );
                    });
                    ui.hyperlink_to("詳しい説明を既定のブラウザで開く", REMOTE_MANUAL_URL);
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                remote_enable_action_allowed(pin_configured),
                                egui::Button::new("リモート接続を有効にする")
                                    .min_size(egui::vec2(220.0, 34.0)),
                            )
                            .clicked()
                        {
                            requested_enabled = Some(true);
                        }
                        if !pin_configured {
                            ui.small("PIN が未設定のため有効にできません。");
                        }
                    });
                } else if ui
                    .add(
                        egui::Button::new("リモート接続を無効にする")
                            .min_size(egui::vec2(220.0, 34.0)),
                    )
                    .clicked()
                {
                    requested_enabled = Some(false);
                }
                ui.separator();
                let state_label = remote_connection_state_label(accepting, &service_diagnostic);
                ui.horizontal(|ui| {
                    ui.label("状態:");
                    ui.strong(state_label);
                });
                ui.horizontal(|ui| {
                    ui.label("利用状況:");
                    ui.strong(remote_client_state_label(remote_client_active));
                });

                if !accepting {
                    match &service_diagnostic {
                        super::service::RemoteServiceDiagnostic::VersionMismatch => {
                            ui.label("本体とリモート接続機能の版が一致しません。");
                            ui.label("両方を同じビルドで更新してください。");
                        }
                        super::service::RemoteServiceDiagnostic::Error(error) => {
                            ui.label(error);
                        }
                        super::service::RemoteServiceDiagnostic::Starting => {
                            ui.small("通常は数秒で接続先が表示されます。");
                        }
                        super::service::RemoteServiceDiagnostic::Stopped => {}
                    }
                }

                if accepting && let Some(info) = info.as_ref() {
                    ui.separator();
                    ui.label(format!(
                        "HTTPS 証明書: {}",
                        remote_https_certificate_status_label(info.tailscale_https_certificate)
                    ));
                    if info.tailscale_https_certificate
                        == RemoteWebFeatureStatus::NotConfigured
                    {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            "tailnet で HTTPS 証明書が有効になっていないため、tailscale serve を設定できません。",
                        );
                        ui.label(
                            "Tailscale 管理コンソールの DNS ページで HTTPS 証明書を有効にしてください。",
                        );
                        ui.hyperlink_to(
                            "Tailscale の DNS 設定を開く",
                            REMOTE_TAILSCALE_DNS_URL,
                        );
                    }
                    show_remote_key_expiry(
                        ui,
                        info.tailscale_key_expiry_unix_seconds,
                        key_expiry_display,
                    );
                    ui.label(format!(
                        "tailscale serve: {}",
                        remote_feature_status_label(info.tailscale_serve)
                    ));
                    let elements = remote_tailscale_serve_elements(
                        info.tailscale_serve,
                        info.tailscale_https_certificate,
                        info.tailscale_serve_conflict.is_some(),
                        info.tailscale_serve_unsupported_path.is_some(),
                    );
                    if elements.show_unknown_message {
                        ui.label("Tailscale が見つからないか、状態を読み取れません。");
                    }
                    if elements.show_setup_button {
                        ui.label(format!(
                            "この PC の {serve_port} 番を、tailnet 内から HTTPS で開けるようにします。"
                        ));
                        ui.label("TLS は Tailscale が処理します。インターネットには公開されません。");
                        ui.label("実行するコマンド:");
                        ui.monospace(format!("tailscale serve --bg {serve_port}"));
                        if elements.show_unsupported_path_warning
                            && let Some(path) =
                                info.tailscale_serve_unsupported_path.as_deref()
                        {
                            ui.colored_label(
                                ui.visuals().warn_fg_color,
                                format!(
                                    "tailscale serve は {path} に設定されています。mImageViewer はパスを付けた公開に対応していないため、この URL では接続できません。"
                                ),
                            );
                        }
                        if elements.show_conflict_warning
                            && let Some(conflict) = info.tailscale_serve_conflict.as_deref()
                        {
                            let error_color = ui.visuals().error_fg_color;
                            ui.colored_label(
                                error_color,
                                format!(
                                    "現在 {} は {} に割り当てられています。設定すると置き換わります。",
                                    info.public_url, conflict
                                ),
                            );
                        }
                        let running = matches!(
                            &dialog.tailscale_serve_setup,
                            RemoteTailscaleServeSetupState::Running { .. }
                        );
                        let awaiting_refresh = matches!(
                            &dialog.tailscale_serve_setup,
                            RemoteTailscaleServeSetupState::Finished(Ok(()))
                        );
                        if ui
                            .add_enabled(
                                elements.setup_button_enabled && !running && !awaiting_refresh,
                                egui::Button::new("tailscale serve を設定する")
                                    .min_size(egui::vec2(220.0, 34.0)),
                            )
                            .clicked()
                        {
                            begin_tailscale_serve_setup = true;
                        }
                        match &dialog.tailscale_serve_setup {
                            RemoteTailscaleServeSetupState::Running { .. } => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label("tailscale serve を設定しています...");
                                });
                            }
                            RemoteTailscaleServeSetupState::Finished(Ok(())) => {
                                ui.label("設定しました。接続状態を再確認しています...");
                            }
                            RemoteTailscaleServeSetupState::Finished(Err(error)) => {
                                ui.colored_label(ui.visuals().error_fg_color, error);
                            }
                            RemoteTailscaleServeSetupState::Idle => {}
                        }
                    }
                    if elements.show_removal_note {
                        ui.small("解除は Tailscale 側で行ってください。");
                    }
                    if info.tailscale_serve == RemoteWebFeatureStatus::Configured {
                        ui.add_space(6.0);
                        paint_qr(ui, &info.public_url);
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.monospace(&info.public_url);
                            if ui.button("コピー").clicked() {
                                ui.ctx().copy_text(info.public_url.clone());
                            }
                        });
                        ui.small("QR コードには接続先だけが含まれ、PIN などの認証情報は含まれません。");
                    }
                }
                if info.is_none() {
                    match &dialog.tailscale_serve_setup {
                        RemoteTailscaleServeSetupState::Running { .. } => {
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("tailscale serve を設定しています...");
                            });
                        }
                        RemoteTailscaleServeSetupState::Finished(Ok(())) => {
                            ui.separator();
                            ui.label("tailscale serve を設定しました。接続状態を再確認しています...");
                        }
                        RemoteTailscaleServeSetupState::Finished(Err(error)) => {
                            ui.separator();
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        }
                        RemoteTailscaleServeSetupState::Idle => {}
                    }
                }

                ui.separator();
                if ui.button("閉じる").clicked() {
                    close_requested = true;
                }
            });

        let logout_confirmation_open = matches!(
            &self.remote_session_ui.session_logout,
            RemoteSessionLogoutState::Confirming
        );
        if logout_confirmation_open {
            let mut confirm = false;
            let mut cancel = escape_pressed;
            let response = egui::Modal::new(egui::Id::new("remote_logout_all_confirm")).show(
                ctx,
                |ui| {
                    ui.set_min_width(420.0);
                    ui.heading("すべての端末をログアウト");
                    ui.add_space(8.0);
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "この PC に接続中の端末を含め、すべての端末で PIN の再入力が必要になります。",
                    );
                    ui.label("PIN 自体は変わりません。");
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("ログアウトする").clicked() {
                            confirm = true;
                        }
                        if ui.button("キャンセル").clicked() {
                            cancel = true;
                        }
                    });
                },
            );
            if confirm {
                begin_session_logout = true;
            } else if cancel || response.should_close() {
                self.remote_session_ui.session_logout.cancel_confirmation();
            }
        }

        if begin_tailscale_serve_setup {
            dialog.tailscale_serve_setup = match service_control
                .as_ref()
                .ok_or_else(|| "tailscale serve の設定を開始できませんでした".to_owned())
                .and_then(super::RemoteServiceControl::configure_tailscale_serve)
            {
                Ok(receiver) => RemoteTailscaleServeSetupState::Running { receiver },
                Err(error) => RemoteTailscaleServeSetupState::Finished(Err(error)),
            };
            ctx.request_repaint();
        }

        if begin_session_logout {
            let result = service_control
                .as_ref()
                .ok_or_else(|| "すべての端末のログアウトを開始できませんでした".to_owned())
                .and_then(super::RemoteServiceControl::rotate_session_secret);
            self.remote_session_ui.session_logout.start(result);
            ctx.request_repaint();
        }

        if let Some(enabled) = requested_enabled {
            self.settings.remote_service_enabled = enabled;
            self.settings.save();
            if let Some(control) = service_control.as_ref() {
                control.set_enabled(enabled);
            } else if enabled
                && let Some(status) = self.remote_session_ui.remote_service_status.as_ref()
            {
                status.set_error("リモート接続を開始できませんでした");
            }
            ctx.request_repaint();
        }

        if close_requested || (escape_pressed && !logout_confirmation_open) || !open {
            self.remote_session_ui.session_logout.dialog_closed();
            self.remote_session_ui.connection_dialog = None;
        } else {
            let pin_saving = matches!(&dialog.pin_editor, RemotePinEditorState::Saving { .. });
            let tailscale_running = matches!(
                &dialog.tailscale_serve_setup,
                RemoteTailscaleServeSetupState::Running { .. }
            );
            self.remote_session_ui.connection_dialog = Some(dialog);
            ctx.request_repaint_after(if pin_saving || tailscale_running {
                std::time::Duration::from_millis(50)
            } else {
                std::time::Duration::from_secs(1)
            });
        }
    }

    fn reload_after_remote_session_release(&mut self) {
        let paused_animation_key = self
            .fullscreen_idx
            .filter(|index| self.fs_entry_is_animated(*index))
            .and_then(|index| self.items.get(index))
            .map(crate::grid_item::GridItem::perf_key);
        let fullscreen_key = self
            .fullscreen_idx
            .and_then(|index| self.items.get(index))
            .map(crate::grid_item::GridItem::perf_key);
        let view = if self.items_are_reading_history_view {
            ReloadedView::ReadingHistory
        } else if self.items_are_rating_view {
            ReloadedView::Rating
        } else if self.items_are_bookmark_view {
            ReloadedView::Bookmarks
        } else if self.items_are_smart_folder_view {
            self.current_smart_folder_id
                .map(ReloadedView::SmartFolder)
                .unwrap_or(ReloadedView::Other)
        } else {
            ReloadedView::Other
        };

        // 既存の各「再読み込み」入口は start_loading_items で viewer を閉じる。
        // 先に identity を保持し、同じ入口が完了した後だけ open_fullscreen へ戻す。
        if fullscreen_key.is_some() {
            self.close_fullscreen();
        }
        match view {
            ReloadedView::ReadingHistory => self.enter_reading_history(),
            ReloadedView::Rating => self.reload_current_rating_view_preserving_sort(),
            ReloadedView::Bookmarks => self.refresh_bookmark_browser(),
            ReloadedView::SmartFolder(id) => self.open_smart_folder(id, true),
            ReloadedView::Other => self.reload_current_folder_preserving_override(),
        }
        self.remote_session_ui.pending_fullscreen_restore =
            fullscreen_key.map(|item_key| PendingFullscreenRestore {
                item_key,
                view,
                wait_frames: 0,
            });
        self.remote_session_ui.paused_animation_restore_key = paused_animation_key;
        crate::logger::log("remote_ipc: local control restored; current view reload requested");
    }

    fn poll_remote_fullscreen_restore(&mut self) {
        let Some(mut pending) = self.remote_session_ui.pending_fullscreen_restore.take() else {
            return;
        };
        pending.wait_frames = pending.wait_frames.saturating_add(1);
        let ready = match pending.view {
            ReloadedView::ReadingHistory => true,
            ReloadedView::Rating => self.rating_view_pending.is_none(),
            ReloadedView::Bookmarks => self.bookmark_browser_pending.is_none(),
            ReloadedView::SmartFolder(id) => {
                self.current_smart_folder_id == Some(id)
                    && self.smart_folder_pending.is_none()
                    && self.smart_folder_prepare_pending.is_none()
                    && self.smart_folder_confirm_pending.is_none()
            }
            // 通常フォルダの同期/非同期差を吸収し、旧 items を同フレームに拾わない。
            ReloadedView::Other => pending.wait_frames >= 2,
        };
        if ready {
            if let Some(index) = self
                .items
                .iter()
                .position(|item| item.perf_key() == pending.item_key)
            {
                self.selected = Some(index);
                if matches!(
                    self.items.get(index),
                    Some(crate::grid_item::GridItem::Video(_))
                ) {
                    self.fs_video_open_autoplay_override = Some(false);
                }
                // Re-establish the viewer that existed before the remote session. This is an
                // automatic restoration, not a new item chosen by the local user.
                self.open_fullscreen(index, crate::app::HistoryTrigger::AutoAdvance);
                crate::logger::log(
                    "remote_ipc: fullscreen position restored after session release",
                );
                return;
            }
            // 非同期一覧の install が直後のフレームに来る場合だけ有界に待つ。
            if pending.wait_frames < 120 {
                self.remote_session_ui.pending_fullscreen_restore = Some(pending);
            }
        } else {
            self.remote_session_ui.pending_fullscreen_restore = Some(pending);
        }
    }
}

fn remote_spread_key(
    address: &mimageviewer_ipc::RemoteAddress,
) -> Result<crate::spread_db::SpreadContainerKey, RemoteWriteError> {
    let root = remote_logical_path(address)?;
    let segments = match &address.subresource {
        RemoteSubresource::File => Vec::new(),
        RemoteSubresource::ZipDirectory { prefix } => prefix
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect(),
        RemoteSubresource::ZipEntry { .. } | RemoteSubresource::PdfPage { .. } => {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::Unsupported,
                "ページ単体には見開き設定を保存できません",
            ));
        }
    };
    Ok(crate::spread_db::container_key_with_fallback(
        &root, &segments,
    ))
}

struct RemoteRatingTarget {
    key: String,
    meta: crate::rating_db::RatingMeta,
    xmp_target: Option<std::path::PathBuf>,
}

fn remote_ai_model_catalog(mode: crate::settings::AiFeatureMode) -> RemoteAiModelCatalog {
    let upscale = crate::adjustment::upscale_menu_items()
        .into_iter()
        .map(|(label, key)| RemoteAiModelOption {
            key: key.map(str::to_owned),
            label: label.to_owned(),
            selectable: match key {
                None => true,
                Some("auto") => !matches!(mode, crate::settings::AiFeatureMode::Disabled),
                Some(key) => crate::ai::ModelKind::from_str(key)
                    .is_some_and(|model| mode.allows_upscale_model(model)),
            },
        })
        .collect();
    let mut denoise = vec![RemoteAiModelOption {
        key: None,
        label: "なし".to_owned(),
        selectable: true,
    }];
    denoise.extend(crate::ai::ModelKind::denoise_models().iter().map(|model| {
        RemoteAiModelOption {
            key: Some(model.as_str().to_owned()),
            label: model.display_label().to_owned(),
            selectable: mode.allows_denoise(),
        }
    }));
    RemoteAiModelCatalog { upscale, denoise }
}

fn remote_adjustment_target(
    settings: &crate::settings::Settings,
    address: &mimageviewer_ipc::RemoteAddress,
) -> Result<crate::app::PageAdjustmentTarget, RemoteWriteError> {
    let logical = remote_logical_path(address)?;
    let page_key = crate::edit_source::page_key_for_remote(&logical, &address.subresource)
        .ok_or_else(|| {
            RemoteWriteError::new(
                RemoteWriteErrorCode::Unsupported,
                "この項目は画像補正の対象ではありません",
            )
        })?;
    let compiled_book = matches!(address.subresource, RemoteSubresource::File)
        && logical.parent().is_some_and(|parent| {
            crate::books::is_direct_book_folder(&settings.books_root_path(), parent)
        });
    let location_path = if compiled_book {
        None
    } else {
        match &address.subresource {
            RemoteSubresource::File => logical.parent().map(std::path::Path::to_path_buf),
            RemoteSubresource::ZipEntry { .. } | RemoteSubresource::PdfPage { .. } => {
                Some(logical.clone())
            }
            RemoteSubresource::ZipDirectory { .. } => None,
        }
    };
    let sidecar_coords = if compiled_book {
        None
    } else {
        let folder = logical
            .parent()
            .ok_or_else(|| {
                RemoteWriteError::new(
                    RemoteWriteErrorCode::BadRequest,
                    "補正 sidecar の親フォルダを解決できません",
                )
            })?
            .to_path_buf();
        let container_name = logical
            .file_name()
            .ok_or_else(|| {
                RemoteWriteError::new(
                    RemoteWriteErrorCode::BadRequest,
                    "補正 sidecar の項目名を解決できません",
                )
            })?
            .to_string_lossy()
            .to_lowercase();
        let rel_key = match &address.subresource {
            RemoteSubresource::File => {
                crate::sidecar::real_file_rel_key(&logical).ok_or_else(|| {
                    RemoteWriteError::new(
                        RemoteWriteErrorCode::BadRequest,
                        "補正 sidecar の項目名を解決できません",
                    )
                })?
            }
            RemoteSubresource::ZipEntry { entry_name } => {
                format!("{container_name}::{}", entry_name.to_lowercase())
            }
            RemoteSubresource::PdfPage { page_number } => {
                format!("{container_name}::page_{page_number}")
            }
            RemoteSubresource::ZipDirectory { .. } => {
                return Err(RemoteWriteError::new(
                    RemoteWriteErrorCode::Unsupported,
                    "ZIP 内フォルダは画像補正の対象ではありません",
                ));
            }
        };
        Some((folder, rel_key))
    };
    Ok(crate::app::PageAdjustmentTarget {
        page_key,
        location_path,
        sidecar_coords,
        compiled_book,
        idx_hint: None,
    })
}

fn remote_rating_target(
    settings: &crate::settings::Settings,
    address: &mimageviewer_ipc::RemoteAddress,
) -> Result<RemoteRatingTarget, RemoteWriteError> {
    use crate::rating_db::{RatingItemKind, RatingMeta};

    let root = remote_logical_path(address)?;
    let (key, meta, xmp_target) = match &address.subresource {
        RemoteSubresource::File => {
            let meta = RatingMeta::new(RatingItemKind::Image).with_source_path(&root);
            let compiled = root.parent().is_some_and(|parent| {
                crate::books::is_direct_book_folder(&settings.books_root_path(), parent)
            });
            let xmp_target =
                (!compiled && crate::xmp_writer::is_writable_format(&root)).then(|| root.clone());
            (
                crate::adjustment_db::normalize_path(&root),
                meta,
                xmp_target,
            )
        }
        RemoteSubresource::ZipEntry { entry_name } => {
            let mut meta = RatingMeta::new(RatingItemKind::ZipImage).with_source_path(&root);
            meta.entry_name = Some(entry_name.clone());
            (
                crate::adjustment_db::zip_entry_key(&root, entry_name),
                meta,
                None,
            )
        }
        RemoteSubresource::PdfPage { page_number } => {
            let mut meta = RatingMeta::new(RatingItemKind::PdfPage).with_source_path(&root);
            meta.page_num = Some(*page_number);
            (
                crate::adjustment_db::zip_entry_key(&root, &format!("page_{page_number}")),
                meta,
                None,
            )
        }
        RemoteSubresource::ZipDirectory { .. } => {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::Unsupported,
                "コンテナ自体にはページレーティングを付けられません",
            ));
        }
    };
    Ok(RemoteRatingTarget {
        key,
        meta,
        xmp_target,
    })
}

struct RemoteReadingTarget {
    container_path: std::path::PathBuf,
    history_kind: crate::reading_history_db::ReadingHistoryKind,
}

fn remote_reading_target(
    address: &mimageviewer_ipc::RemoteAddress,
    context_address: &mimageviewer_ipc::RemoteAddress,
) -> Result<RemoteReadingTarget, RemoteWriteError> {
    let container_path = remote_logical_path(context_address)?;
    let history_kind = match (&address.subresource, &context_address.subresource) {
        (RemoteSubresource::File, RemoteSubresource::File) => {
            crate::reading_history_db::ReadingHistoryKind::Folder
        }
        (
            RemoteSubresource::ZipEntry { .. },
            RemoteSubresource::File | RemoteSubresource::ZipDirectory { .. },
        ) => crate::reading_history_db::ReadingHistoryKind::Zip,
        (RemoteSubresource::PdfPage { .. }, RemoteSubresource::File) => {
            crate::reading_history_db::ReadingHistoryKind::Pdf
        }
        _ => {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::BadRequest,
                "ページと閲覧コンテキストが一致しません",
            ));
        }
    };
    Ok(RemoteReadingTarget {
        container_path,
        history_kind,
    })
}

fn remote_bookmark_draft(
    settings: &crate::settings::Settings,
    address: &mimageviewer_ipc::RemoteAddress,
    context_address: &mimageviewer_ipc::RemoteAddress,
    page_index: u32,
) -> Result<crate::book_bookmarks::NewBookBookmark, RemoteWriteError> {
    use crate::book_bookmarks::{BookContainerKind, NewBookBookmark, PageIdentity};

    let container_path = remote_logical_path(context_address)?;
    let (container_kind, page_identity) = match &address.subresource {
        RemoteSubresource::File => {
            let page_path = remote_logical_path(address)?;
            let relative = page_path
                .strip_prefix(&container_path)
                .ok()
                .filter(|value| !value.as_os_str().is_empty())
                .and_then(|value| value.to_str())
                .map(str::to_owned)
                .or_else(|| {
                    page_path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .map(str::to_owned)
                })
                .ok_or_else(|| {
                    RemoteWriteError::new(
                        RemoteWriteErrorCode::BadRequest,
                        "画像のページ identity を作れません",
                    )
                })?;
            let kind = if crate::books::is_direct_book_folder(
                &settings.books_root_path(),
                &container_path,
            ) {
                BookContainerKind::CompiledBook
            } else {
                BookContainerKind::ImageFolder
            };
            (kind, PageIdentity::RelativePath(relative))
        }
        RemoteSubresource::ZipEntry { entry_name } => (
            BookContainerKind::Zip,
            PageIdentity::ArchiveEntry(entry_name.clone()),
        ),
        RemoteSubresource::PdfPage { page_number } => {
            (BookContainerKind::Pdf, PageIdentity::PdfPage(*page_number))
        }
        RemoteSubresource::ZipDirectory { .. } => {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::Unsupported,
                "コンテナ自体はブックマークできません",
            ));
        }
    };
    Ok(NewBookBookmark {
        container_path,
        container_kind,
        page_identity,
        page_index_hint: page_index as usize,
    })
}

fn remote_logical_path(
    address: &mimageviewer_ipc::RemoteAddress,
) -> Result<std::path::PathBuf, RemoteWriteError> {
    address.validate_syntax().map_err(|error| {
        RemoteWriteError::new(
            RemoteWriteErrorCode::BadRequest,
            if error == mimageviewer_ipc::AddressError::NetworkPath {
                mimageviewer_ipc::REMOTE_NETWORK_PATH_MESSAGE
            } else {
                "コンテンツアドレスが不正です"
            },
        )
    })?;
    super::path_guard::resolve_existing(&address.path)
        .map(|resolved| resolved.logical)
        .map_err(|error| match error {
            super::path_guard::ResolveError::InvalidPath => RemoteWriteError::new(
                RemoteWriteErrorCode::BadRequest,
                "コンテンツアドレスが不正です",
            ),
            super::path_guard::ResolveError::NetworkPath => RemoteWriteError::new(
                RemoteWriteErrorCode::BadRequest,
                mimageviewer_ipc::REMOTE_NETWORK_PATH_MESSAGE,
            ),
            super::path_guard::ResolveError::Unavailable => {
                RemoteWriteError::new(RemoteWriteErrorCode::NotFound, "対象が見つかりません")
            }
        })
}

fn remote_enable_warning_visible(enabled: bool) -> bool {
    !enabled
}

fn core_spread_mode(mode: RemoteSpreadMode) -> crate::settings::SpreadMode {
    match mode {
        RemoteSpreadMode::Single => crate::settings::SpreadMode::Single,
        RemoteSpreadMode::Ltr => crate::settings::SpreadMode::Ltr,
        RemoteSpreadMode::LtrCover => crate::settings::SpreadMode::LtrCover,
        RemoteSpreadMode::Rtl => crate::settings::SpreadMode::Rtl,
        RemoteSpreadMode::RtlCover => crate::settings::SpreadMode::RtlCover,
    }
}

fn core_reading_direction(
    mode: crate::settings::SpreadMode,
    requested: RemoteReadingDirection,
) -> crate::settings::ReadingDirection {
    if mode.is_rtl() {
        crate::settings::ReadingDirection::Rtl
    } else if matches!(
        mode,
        crate::settings::SpreadMode::Ltr | crate::settings::SpreadMode::LtrCover
    ) {
        crate::settings::ReadingDirection::Ltr
    } else if requested.is_rtl() {
        crate::settings::ReadingDirection::Rtl
    } else {
        crate::settings::ReadingDirection::Ltr
    }
}

fn write_error(code: RemoteWriteErrorCode, message: &'static str) -> RemoteWriteResponse {
    RemoteWriteResponse::Error(RemoteWriteError::new(code, message))
}

fn remote_feature_status_label(status: RemoteWebFeatureStatus) -> &'static str {
    match status {
        RemoteWebFeatureStatus::Configured => "設定済み",
        RemoteWebFeatureStatus::NotConfigured => "未設定",
        RemoteWebFeatureStatus::Unknown => "確認できません",
    }
}

fn remote_https_certificate_status_label(status: RemoteWebFeatureStatus) -> &'static str {
    match status {
        RemoteWebFeatureStatus::Configured => "有効",
        RemoteWebFeatureStatus::NotConfigured => "無効",
        RemoteWebFeatureStatus::Unknown => "読み取れません",
    }
}

fn current_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(0)
}

fn show_remote_key_expiry(
    ui: &mut egui::Ui,
    expiry_unix_seconds: Option<i64>,
    display: RemoteKeyExpiryDisplay,
) {
    let Some(line) = remote_key_expiry_line(expiry_unix_seconds, display) else {
        return;
    };
    match display {
        RemoteKeyExpiryDisplay::Unavailable => {}
        RemoteKeyExpiryDisplay::Normal { .. } => {
            ui.label(line);
        }
        RemoteKeyExpiryDisplay::Warning { .. } => {
            ui.colored_label(ui.visuals().warn_fg_color, line);
            ui.label(
                "期限切れになる前に、Tailscale のデバイス設定でこの PC のキーを無期限にできます。",
            );
            ui.hyperlink_to(
                "Tailscale のデバイス一覧を開く",
                REMOTE_TAILSCALE_MACHINES_URL,
            );
        }
        RemoteKeyExpiryDisplay::Expired => {
            ui.colored_label(ui.visuals().warn_fg_color, line);
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "この PC は tailnet から外れているため、外出先から接続できません。",
            );
            ui.label(
                "Tailscale 管理コンソールのデバイス設定で、この PC のキーを無期限にできます。",
            );
            ui.hyperlink_to(
                "Tailscale のデバイス一覧を開く",
                REMOTE_TAILSCALE_MACHINES_URL,
            );
        }
    }
}

fn remote_key_expiry_line(
    expiry_unix_seconds: Option<i64>,
    display: RemoteKeyExpiryDisplay,
) -> Option<String> {
    let date = format_local_unix_seconds_date(expiry_unix_seconds?);
    match display {
        RemoteKeyExpiryDisplay::Unavailable => None,
        RemoteKeyExpiryDisplay::Normal { remaining_days }
        | RemoteKeyExpiryDisplay::Warning { remaining_days } => Some(format!(
            "接続キーの有効期限: {date} (あと {remaining_days} 日)"
        )),
        RemoteKeyExpiryDisplay::Expired => Some(format!("接続キーの有効期限: {date} (期限切れ)")),
    }
}

fn format_local_unix_seconds_date(unix_seconds: i64) -> String {
    format_local_unix_seconds_date_with(unix_seconds, format_local_unix_ms)
}

fn format_local_unix_seconds_date_with(
    unix_seconds: i64,
    format_local: impl FnOnce(u64) -> String,
) -> String {
    const DATE_FORMAT_FAILURE: &str = "取得できません";
    let Some(unix_ms) = u64::try_from(unix_seconds)
        .ok()
        .and_then(|seconds| seconds.checked_mul(1_000))
    else {
        return DATE_FORMAT_FAILURE.to_owned();
    };
    // 管理コンソールの地方時表示と突き合わせる値なので、UTC の civil date は再計算せず、
    // 既存の Win32 地方時変換が返す timestamp から日付部分だけを取り出す。
    let local = format_local(unix_ms);
    let Some(date) = local.get(..10) else {
        return DATE_FORMAT_FAILURE.to_owned();
    };
    let bytes = date.as_bytes();
    if bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| !matches!(index, 4 | 7) && !byte.is_ascii_digit())
    {
        return DATE_FORMAT_FAILURE.to_owned();
    }
    date.to_owned()
}

fn paint_qr(ui: &mut egui::Ui, url: &str) {
    let Ok(code) = QrCode::new(url.as_bytes()) else {
        ui.colored_label(egui::Color32::RED, "QR コードを生成できませんでした");
        return;
    };
    const QUIET_ZONE: usize = 4;
    const DISPLAY_PX: f32 = 240.0;
    let width = code.width();
    let modules = width + QUIET_ZONE * 2;
    let module_px = (DISPLAY_PX / modules as f32).floor().max(1.0);
    let side = module_px * modules as f32;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::WHITE);
    for (index, color) in code.to_colors().into_iter().enumerate() {
        if color != Color::Dark {
            continue;
        }
        let x = index % width + QUIET_ZONE;
        let y = index / width + QUIET_ZONE;
        let min = rect.min + egui::vec2(x as f32 * module_px, y as f32 * module_px);
        painter.rect_filled(
            egui::Rect::from_min_size(min, egui::vec2(module_px, module_px)),
            0.0,
            egui::Color32::BLACK,
        );
    }
}

fn show_connection_summary(ui: &mut egui::Ui, snapshot: &ActiveSessionSnapshot) {
    let connection = match snapshot.peer.connection_kind {
        SessionConnectionKind::Direct => "direct",
        SessionConnectionKind::Relay => "relay",
        SessionConnectionKind::Unknown => "取得できません",
    };
    ui.label(format!("接続種別: {connection}"));
    ui.label(format!(
        "対向端末: {}",
        snapshot
            .peer
            .device_name
            .as_deref()
            .unwrap_or("取得できません")
    ));
    ui.label(format!(
        "接続時刻: {} / 経過 {}",
        format_local_unix_ms(snapshot.connected_unix_ms),
        format_elapsed(snapshot.elapsed)
    ));
}

fn format_elapsed(elapsed: std::time::Duration) -> String {
    let total = elapsed.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REMOTE_ENABLE_WARNING_FIRST: &str = concat!(
        "リモート閲覧を有効にすると、",
        "すべてのドライブについて、mIV で表示できるファイル",
        "が、この PC の Tailscale アドレスへ接続でき、PIN を知っている人から見えるようになります。"
    );

    #[test]
    fn stream_control_errors_separate_session_loss_from_registration_invariants() {
        for error in [
            RemoteStreamingControlError::SessionMissing,
            RemoteStreamingControlError::SessionGenerationMismatch,
        ] {
            assert_eq!(
                remote_streaming_control_error(error).code,
                VideoStreamErrorCode::SessionMismatch
            );
        }
        for error in [
            RemoteStreamingControlError::RegistrationMissing,
            RemoteStreamingControlError::RegistrationMismatch,
        ] {
            assert_eq!(
                remote_streaming_control_error(error).code,
                VideoStreamErrorCode::Failed
            );
        }
    }

    #[test]
    fn acquire_barrier_diagnostics_logs_after_grace_and_then_throttles() {
        let mut diagnostics = RemoteAcquireBarrierDiagnostics::default();
        diagnostics.begin(7);

        assert!(!diagnostics.should_log(7, std::time::Duration::from_secs(2)));
        assert!(diagnostics.should_log(7, std::time::Duration::from_secs(3)));
        assert!(!diagnostics.should_log(7, std::time::Duration::from_secs(12)));
        assert!(diagnostics.should_log(7, std::time::Duration::from_secs(13)));
        assert!(
            !diagnostics.should_log(8, std::time::Duration::from_secs(2)),
            "a new acquisition gets its own grace period"
        );
    }

    #[test]
    fn only_a_new_acquire_coalesced_with_control_return_keeps_the_barrier_held() {
        assert!(coalesced_remote_reacquire(
            super::super::session::RemoteControlPhase::AcquiringRemote,
            true,
            true
        ));
        assert!(!coalesced_remote_reacquire(
            super::super::session::RemoteControlPhase::Local,
            true,
            true
        ));
        assert!(coalesced_remote_reacquire(
            super::super::session::RemoteControlPhase::DrainingRemote,
            true,
            true
        ));
        assert!(!coalesced_remote_reacquire(
            super::super::session::RemoteControlPhase::AcquiringRemote,
            true,
            false
        ));
    }

    #[test]
    fn remote_connection_requires_a_core_owned_pin_before_enable() {
        assert!(!remote_enable_action_allowed(false));
        assert!(remote_enable_action_allowed(true));
    }

    #[test]
    fn remote_connection_dialog_has_no_pending_enabled_state() {
        let RemoteConnectionDialogState {
            pin_editor,
            tailscale_serve_setup,
        } = RemoteConnectionDialogState::new(false);
        assert!(matches!(pin_editor, RemotePinEditorState::Editing { .. }));
        assert!(matches!(
            tailscale_serve_setup,
            RemoteTailscaleServeSetupState::Idle
        ));

        let RemoteConnectionDialogState {
            pin_editor,
            tailscale_serve_setup,
        } = RemoteConnectionDialogState::new(true);
        assert!(matches!(pin_editor, RemotePinEditorState::Hidden));
        assert!(matches!(
            tailscale_serve_setup,
            RemoteTailscaleServeSetupState::Idle
        ));
    }

    #[test]
    fn remote_session_logout_confirmation_has_explicit_confirm_and_cancel_paths() {
        let confirming = remote_session_logout_transition(
            RemoteSessionLogoutPhase::Idle,
            RemoteSessionLogoutEvent::RequestConfirmation,
        );
        assert_eq!(
            confirming.map(|transition| transition.phase),
            Some(RemoteSessionLogoutPhase::Confirming)
        );
        assert_eq!(
            remote_session_logout_transition(
                confirming.unwrap().phase,
                RemoteSessionLogoutEvent::Cancel,
            )
            .map(|transition| transition.phase),
            Some(RemoteSessionLogoutPhase::Idle)
        );

        let running = remote_session_logout_transition(
            confirming.unwrap().phase,
            RemoteSessionLogoutEvent::Confirm,
        );
        assert_eq!(
            running.map(|transition| transition.phase),
            Some(RemoteSessionLogoutPhase::Running)
        );
        assert_eq!(
            remote_session_logout_transition(
                running.unwrap().phase,
                RemoteSessionLogoutEvent::FinishSuccess,
            )
            .map(|transition| transition.phase),
            Some(RemoteSessionLogoutPhase::FinishedSuccess)
        );
        assert_eq!(
            remote_session_logout_transition(
                RemoteSessionLogoutPhase::Idle,
                RemoteSessionLogoutEvent::Confirm,
            ),
            None
        );
    }

    #[test]
    fn remote_session_logout_completion_survives_dialog_close_and_disconnects_once() {
        let mut phase = RemoteSessionLogoutPhase::Running;
        let mut disconnect_count = 0;

        let closed =
            remote_session_logout_transition(phase, RemoteSessionLogoutEvent::DialogClosed)
                .unwrap();
        phase = closed.phase;
        disconnect_count += usize::from(closed.disconnect_local);
        assert_eq!(phase, RemoteSessionLogoutPhase::Running);

        let finished =
            remote_session_logout_transition(phase, RemoteSessionLogoutEvent::FinishSuccess)
                .unwrap();
        phase = finished.phase;
        disconnect_count += usize::from(finished.disconnect_local);

        let hidden_finished =
            remote_session_logout_transition(phase, RemoteSessionLogoutEvent::DialogClosed)
                .unwrap();
        phase = hidden_finished.phase;
        disconnect_count += usize::from(hidden_finished.disconnect_local);

        assert_eq!(phase, RemoteSessionLogoutPhase::Idle);
        assert_eq!(disconnect_count, 1);
        assert!(
            remote_session_logout_transition(phase, RemoteSessionLogoutEvent::FinishSuccess,)
                .is_none()
        );
    }

    #[test]
    fn remote_session_logout_open_success_does_not_drain_twice() {
        let finished = remote_session_logout_transition(
            RemoteSessionLogoutPhase::Running,
            RemoteSessionLogoutEvent::FinishSuccess,
        )
        .unwrap();
        assert!(finished.disconnect_local);
        let closed = remote_session_logout_transition(
            finished.phase,
            RemoteSessionLogoutEvent::DialogClosed,
        )
        .unwrap();
        assert_eq!(closed.phase, RemoteSessionLogoutPhase::Idle);
        assert!(!closed.disconnect_local);
        assert!(
            remote_session_logout_transition(
                finished.phase,
                RemoteSessionLogoutEvent::FinishSuccess,
            )
            .is_none()
        );
    }

    #[test]
    fn remote_session_logout_dialog_close_table_and_failure_are_explicit() {
        // 閉じたときの行き先。失敗だけが残り、成功は溜め込まない。
        let cases = [
            (
                RemoteSessionLogoutPhase::Confirming,
                RemoteSessionLogoutPhase::Idle,
            ),
            (
                RemoteSessionLogoutPhase::Running,
                RemoteSessionLogoutPhase::Running,
            ),
            (
                RemoteSessionLogoutPhase::FinishedSuccess,
                RemoteSessionLogoutPhase::Idle,
            ),
            (
                RemoteSessionLogoutPhase::FinishedError,
                RemoteSessionLogoutPhase::FinishedError,
            ),
        ];
        for (phase, expected_phase) in cases {
            let transition =
                remote_session_logout_transition(phase, RemoteSessionLogoutEvent::DialogClosed)
                    .unwrap();
            assert_eq!(transition.phase, expected_phase);
            assert!(!transition.disconnect_local);
        }

        let failed = remote_session_logout_transition(
            RemoteSessionLogoutPhase::Confirming,
            RemoteSessionLogoutEvent::StartFailed,
        )
        .unwrap();
        assert_eq!(failed.phase, RemoteSessionLogoutPhase::FinishedError);
        assert!(!failed.disconnect_local);
    }

    #[test]
    fn remote_session_logout_start_failure_stays_distinct_from_pin_state() {
        let dialog = RemoteConnectionDialogState::new(true);
        let mut logout = RemoteSessionLogoutState::default();
        logout.request_confirmation();
        logout.start(Err("session secret rotation failed".to_owned()));

        assert!(matches!(dialog.pin_editor, RemotePinEditorState::Hidden));
        assert!(matches!(
            logout,
            RemoteSessionLogoutState::Finished(Err(ref error))
                if error == "session secret rotation failed"
        ));
    }

    #[test]
    fn tailscale_serve_dialog_elements_follow_status_and_conflict() {
        assert_eq!(
            remote_tailscale_serve_elements(
                RemoteWebFeatureStatus::Configured,
                RemoteWebFeatureStatus::Configured,
                false,
                false,
            ),
            RemoteTailscaleServeElements {
                show_setup_button: false,
                setup_button_enabled: false,
                show_conflict_warning: false,
                show_unsupported_path_warning: false,
                show_unknown_message: false,
                show_removal_note: true,
            }
        );
        assert_eq!(
            remote_tailscale_serve_elements(
                RemoteWebFeatureStatus::NotConfigured,
                RemoteWebFeatureStatus::Configured,
                false,
                false,
            ),
            RemoteTailscaleServeElements {
                show_setup_button: true,
                setup_button_enabled: true,
                show_conflict_warning: false,
                show_unsupported_path_warning: false,
                show_unknown_message: false,
                show_removal_note: false,
            }
        );
        assert!(
            remote_tailscale_serve_elements(
                RemoteWebFeatureStatus::NotConfigured,
                RemoteWebFeatureStatus::Configured,
                true,
                false,
            )
            .show_conflict_warning
        );
        assert_eq!(
            remote_tailscale_serve_elements(
                RemoteWebFeatureStatus::Unknown,
                RemoteWebFeatureStatus::Unknown,
                true,
                true,
            ),
            RemoteTailscaleServeElements {
                show_setup_button: false,
                setup_button_enabled: false,
                show_conflict_warning: false,
                show_unsupported_path_warning: false,
                show_unknown_message: true,
                show_removal_note: false,
            }
        );
    }

    #[test]
    fn unsupported_tailscale_serve_path_warns_without_disabling_root_setup() {
        let elements = remote_tailscale_serve_elements(
            RemoteWebFeatureStatus::NotConfigured,
            RemoteWebFeatureStatus::Configured,
            false,
            true,
        );
        assert!(elements.show_setup_button);
        assert!(elements.setup_button_enabled);
        assert!(elements.show_unsupported_path_warning);
        assert!(!elements.show_conflict_warning);
    }

    #[test]
    fn https_certificate_state_controls_only_the_serve_setup_button() {
        let invalid = remote_tailscale_serve_elements(
            RemoteWebFeatureStatus::NotConfigured,
            RemoteWebFeatureStatus::NotConfigured,
            false,
            false,
        );
        assert!(invalid.show_setup_button);
        assert!(!invalid.setup_button_enabled);

        let unknown = remote_tailscale_serve_elements(
            RemoteWebFeatureStatus::NotConfigured,
            RemoteWebFeatureStatus::Unknown,
            false,
            false,
        );
        assert!(unknown.show_setup_button);
        assert!(unknown.setup_button_enabled);
        assert!(remote_enable_action_allowed(true));
    }

    #[test]
    fn key_expiry_display_covers_warning_boundaries_without_inventing_unlimited_state() {
        const DAY: i64 = 86_400;
        let now = 1_700_000_000;
        assert_eq!(
            remote_key_expiry_display(None, now),
            RemoteKeyExpiryDisplay::Unavailable
        );
        assert_eq!(
            remote_key_expiry_line(None, RemoteKeyExpiryDisplay::Unavailable),
            None,
            "期限情報が無いダイアログには期限の行を出さない"
        );
        assert_eq!(
            remote_key_expiry_display(Some(now + 179 * DAY), now),
            RemoteKeyExpiryDisplay::Normal {
                remaining_days: 179
            }
        );
        assert_eq!(
            remote_key_expiry_display(Some(now + 30 * DAY), now),
            RemoteKeyExpiryDisplay::Warning { remaining_days: 30 }
        );
        assert_eq!(
            remote_key_expiry_display(Some(now + DAY), now),
            RemoteKeyExpiryDisplay::Warning { remaining_days: 1 }
        );
        assert_eq!(
            remote_key_expiry_display(Some(now), now),
            RemoteKeyExpiryDisplay::Expired
        );
        assert_eq!(
            remote_key_expiry_display(Some(now - 1), now),
            RemoteKeyExpiryDisplay::Expired
        );
    }

    #[test]
    fn tailnet_prerequisite_links_and_expiry_date_are_stable() {
        assert_eq!(
            REMOTE_TAILSCALE_DNS_URL,
            "https://console.tailscale.com/admin/dns"
        );
        assert_eq!(
            REMOTE_TAILSCALE_MACHINES_URL,
            "https://console.tailscale.com/admin/machines"
        );
        assert_eq!(
            format_local_unix_seconds_date_with(1_770_508_800, |unix_ms| {
                assert_eq!(unix_ms, 1_770_508_800_000);
                "2026-02-09 00:00:00".to_owned()
            }),
            "2026-02-09",
            "管理コンソールと同じ地方時の日付部分を表示する"
        );
        assert_eq!(
            format_local_unix_seconds_date_with(-1, |_| unreachable!()),
            "取得できません"
        );
        assert_eq!(
            remote_key_expiry_line(Some(-1), RemoteKeyExpiryDisplay::Expired).as_deref(),
            Some("接続キーの有効期限: 取得できません (期限切れ)"),
            "地方時変換に失敗しても期限切れ分類は変えない"
        );
        assert_eq!(
            format_local_unix_seconds_date_with(i64::MAX, |_| unreachable!()),
            "取得できません"
        );
        assert_eq!(
            format_local_unix_seconds_date_with(1_770_508_800, |_| {
                "取得できません".to_owned()
            }),
            "取得できません"
        );
    }

    #[test]
    fn remote_access_warning_is_shown_while_disabled_before_the_one_click_enable() {
        assert!(remote_enable_warning_visible(false));
        assert!(!remote_enable_warning_visible(true));
        assert_eq!(
            REMOTE_ENABLE_WARNING_FIRST,
            "リモート閲覧を有効にすると、すべてのドライブについて、mIV で表示できるファイルが、この PC の Tailscale アドレスへ接続でき、PIN を知っている人から見えるようになります。"
        );
        assert_eq!(
            [
                REMOTE_ENABLE_WARNING_PREFIX,
                REMOTE_ENABLE_WARNING_EMPHASIS,
                REMOTE_ENABLE_WARNING_SUFFIX,
            ]
            .concat(),
            REMOTE_ENABLE_WARNING_FIRST
        );
        assert_eq!(
            REMOTE_MANUAL_URL,
            "https://mikage.to/mimageviewer/manual/tut-remote.html"
        );
    }

    #[test]
    fn remote_connection_labels_use_health_and_active_session_separately() {
        assert_eq!(
            remote_connection_state_label(
                true,
                &crate::remote_ipc::service::RemoteServiceDiagnostic::Starting
            ),
            "受け付けています"
        );
        assert_eq!(
            remote_connection_state_label(
                false,
                &crate::remote_ipc::service::RemoteServiceDiagnostic::Stopped
            ),
            "停止しています"
        );
        assert_eq!(remote_client_state_label(false), "なし");
        assert_eq!(remote_client_state_label(true), "操作中");
    }

    #[test]
    fn remote_ai_catalog_exposes_core_labels_and_server_side_mode_permissions() {
        let light = remote_ai_model_catalog(crate::settings::AiFeatureMode::Light);
        assert!(light.upscale.iter().any(|entry| {
            entry.key.as_deref() == Some("realcugan_4x")
                && entry.label == crate::ai::ModelKind::UpscaleRealCugan4x.display_label()
                && entry.selectable
        }));
        assert!(light.upscale.iter().any(|entry| {
            entry.key.as_deref() == Some("realesrgan_x4plus") && !entry.selectable
        }));
        assert!(light.denoise.iter().skip(1).all(|entry| !entry.selectable));

        let high = remote_ai_model_catalog(crate::settings::AiFeatureMode::HighQuality);
        assert!(high.upscale.iter().all(|entry| entry.selectable));
        assert!(high.denoise.iter().all(|entry| entry.selectable));
    }

    #[test]
    fn remote_vst_load_budget_preserves_the_encoder_playlist_reserve() {
        assert_eq!(
            remote_vst_load_budget(std::time::Duration::from_secs(15)),
            std::time::Duration::from_secs(10)
        );
        assert_eq!(
            remote_vst_load_budget(std::time::Duration::from_secs(8)),
            std::time::Duration::from_secs(5)
        );
        assert!(remote_vst_load_budget(std::time::Duration::from_secs(2)).is_zero());
    }

    #[test]
    fn remote_video_end_behavior_matches_local_continuous_and_loop_precedence() {
        use crate::settings::VideoLoopMode;
        use crate::video::VideoContinuousMode;

        assert_eq!(
            resolve_remote_video_end_behavior(
                VideoContinuousMode::Off,
                VideoLoopMode::Off,
                Vec::new(),
                Vec::new(),
            ),
            VideoStreamEndBehavior::Stop,
            "continuous OFF and loop OFF stop at EOF"
        );
        assert_eq!(
            resolve_remote_video_end_behavior(
                VideoContinuousMode::Continuous,
                VideoLoopMode::Full,
                Vec::new(),
                Vec::new(),
            ),
            VideoStreamEndBehavior::Next { wrap: false },
            "continuous playback suppresses the local loop setting"
        );
        assert_eq!(
            resolve_remote_video_end_behavior(
                VideoContinuousMode::ContinuousLoop,
                VideoLoopMode::Off,
                Vec::new(),
                Vec::new(),
            ),
            VideoStreamEndBehavior::Next { wrap: true },
            "continuous-loop wraps the video list"
        );
        assert_eq!(
            resolve_remote_video_end_behavior(
                VideoContinuousMode::Off,
                VideoLoopMode::Full,
                Vec::new(),
                Vec::new(),
            ),
            VideoStreamEndBehavior::Loop {
                boundary_starts_secs: vec![0.0],
            },
            "whole-video loop returns to zero"
        );
    }

    #[test]
    fn remote_video_end_behavior_keeps_local_section_loop_boundaries_and_fallback() {
        use crate::settings::VideoLoopMode;
        use crate::video::VideoContinuousMode;

        assert_eq!(
            resolve_remote_video_end_behavior(
                VideoContinuousMode::Off,
                VideoLoopMode::Chapter,
                vec![12.0, 42.5],
                Vec::new(),
            ),
            VideoStreamEndBehavior::Loop {
                boundary_starts_secs: vec![12.0, 42.5],
            }
        );
        assert_eq!(
            resolve_remote_video_end_behavior(
                VideoContinuousMode::Off,
                VideoLoopMode::Bookmark,
                Vec::new(),
                Vec::new(),
            ),
            VideoStreamEndBehavior::Loop {
                boundary_starts_secs: vec![0.0],
            },
            "a missing bookmark set degrades to the same full loop as local playback"
        );
    }

    #[test]
    fn remote_spread_key_matches_worker_canonical_path() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        std::fs::create_dir_all(root.join("album")).unwrap();
        std::fs::write(root.join("album/book.zip"), b"zip").unwrap();
        for path in [root.clone(), root.join("album/book.zip")] {
            let address =
                mimageviewer_ipc::RemoteAddress::file(path.to_string_lossy().into_owned());
            let worker = crate::remote_ipc::path_guard::resolve_existing(&address.path).unwrap();
            let ui_key = remote_spread_key(&address).unwrap();
            assert_eq!(ui_key.exact, worker.logical, "path={path:?}");
        }
    }

    #[test]
    fn remote_write_keys_canonicalize_aliases_and_reject_missing_paths() {
        let temp = tempfile::tempdir().unwrap();
        let album = temp.path().join("album");
        std::fs::create_dir_all(&album).unwrap();
        let page = album.join("page.jpg");
        std::fs::write(&page, b"image").unwrap();
        let alias = album.join("..").join("album").join("page.jpg");
        let canonical_address =
            mimageviewer_ipc::RemoteAddress::file(page.to_string_lossy().into_owned());
        let alias_address =
            mimageviewer_ipc::RemoteAddress::file(alias.to_string_lossy().into_owned());
        let settings = crate::settings::Settings::default();

        assert_ne!(canonical_address.path, alias_address.path);
        assert_eq!(
            remote_logical_path(&canonical_address).unwrap(),
            remote_logical_path(&alias_address).unwrap()
        );
        assert_eq!(
            remote_rating_target(&settings, &canonical_address)
                .unwrap()
                .key,
            remote_rating_target(&settings, &alias_address).unwrap().key
        );

        let canonical_folder =
            mimageviewer_ipc::RemoteAddress::file(album.to_string_lossy().into_owned());
        let alias_folder = mimageviewer_ipc::RemoteAddress::file(
            album
                .join("..")
                .join("album")
                .to_string_lossy()
                .into_owned(),
        );
        assert_eq!(
            remote_spread_key(&canonical_folder).unwrap(),
            remote_spread_key(&alias_folder).unwrap()
        );

        let missing = mimageviewer_ipc::RemoteAddress::file(
            temp.path()
                .join("missing.jpg")
                .to_string_lossy()
                .into_owned(),
        );
        match remote_rating_target(&settings, &missing) {
            Err(error) => assert_eq!(error.code, RemoteWriteErrorCode::NotFound),
            Ok(_) => panic!("missing rating target must be rejected"),
        }
        assert_eq!(
            remote_spread_key(&missing).unwrap_err().code,
            RemoteWriteErrorCode::NotFound
        );
    }

    #[test]
    fn remote_page_keys_match_local_rating_history_and_bookmark_rules() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let settings = crate::settings::Settings::default();

        std::fs::create_dir_all(root.join("album")).unwrap();
        std::fs::create_dir_all(root.join("books")).unwrap();
        std::fs::write(root.join("album/page.jpg"), b"image").unwrap();
        std::fs::write(root.join("books/book.cbz"), b"zip").unwrap();

        let folder = mimageviewer_ipc::RemoteAddress::file(
            root.join("album").to_string_lossy().into_owned(),
        );
        let image = mimageviewer_ipc::RemoteAddress::file(
            root.join("album/page.jpg").to_string_lossy().into_owned(),
        );
        let image_target = remote_rating_target(&settings, &image).unwrap();
        assert_eq!(
            image_target.key,
            crate::adjustment_db::normalize_path(&root.join("album/page.jpg"))
        );
        let reading = remote_reading_target(&image, &folder).unwrap();
        assert_eq!(reading.container_path, root.join("album"));
        assert_eq!(
            reading.history_kind,
            crate::reading_history_db::ReadingHistoryKind::Folder
        );
        let bookmark = remote_bookmark_draft(&settings, &image, &folder, 4).unwrap();
        assert_eq!(
            bookmark.page_identity,
            crate::book_bookmarks::PageIdentity::RelativePath("page.jpg".to_owned())
        );
        assert_eq!(bookmark.page_index_hint, 4);

        let zip_page = mimageviewer_ipc::RemoteAddress {
            path: root.join("books/book.cbz").to_string_lossy().into_owned(),
            subresource: RemoteSubresource::ZipEntry {
                entry_name: "chapter/001.png".to_owned(),
            },
        };
        let zip_target = remote_rating_target(&settings, &zip_page).unwrap();
        assert_eq!(
            zip_target.key,
            crate::adjustment_db::zip_entry_key(&root.join("books/book.cbz"), "chapter/001.png",)
        );
    }
}

#[cfg(windows)]
fn format_local_unix_ms(unix_ms: u64) -> String {
    const WINDOWS_TICKS_PER_MILLISECOND: u64 = 10_000;
    const UNIX_TO_WINDOWS_MILLISECONDS: u64 = 11_644_473_600_000;
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
    use windows::Win32::System::Time::FileTimeToSystemTime;

    let Some(ticks) = unix_ms
        .checked_add(UNIX_TO_WINDOWS_MILLISECONDS)
        .and_then(|value| value.checked_mul(WINDOWS_TICKS_PER_MILLISECOND))
    else {
        return "取得できません".to_owned();
    };
    let filetime = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut local = FILETIME::default();
    let mut system = SYSTEMTIME::default();
    if unsafe { FileTimeToLocalFileTime(&filetime, &mut local) }.is_err()
        || unsafe { FileTimeToSystemTime(&local, &mut system) }.is_err()
    {
        return "取得できません".to_owned();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        system.wYear, system.wMonth, system.wDay, system.wHour, system.wMinute, system.wSecond
    )
}

#[cfg(not(windows))]
fn format_local_unix_ms(unix_ms: u64) -> String {
    format!("unix-ms {unix_ms}")
}
