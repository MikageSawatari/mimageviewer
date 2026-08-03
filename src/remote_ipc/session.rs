use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mimageviewer_ipc::{
    RemoteWebConnectionInfo, RemoteWriteError, RemoteWriteErrorCode, RemoteWriteRequest,
    RemoteWriteResponse, SessionAcquireRequest, SessionPeerInfo, SessionPingRequest,
    SessionResponse, SessionStatus, VideoStreamControlAction, VideoStreamError,
    VideoStreamErrorCode, VideoStreamQuality,
};

use crate::video::stream::session::{
    StreamingGeneration, StreamingGenerationAccess, StreamingSessionId,
};

use super::video_stream::{VideoStreamStartBudget, VideoStreamStartStage};
pub(crate) const LIVENESS_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub(crate) const UI_REQUEST_ACCEPT_TIMEOUT: Duration = Duration::from_secs(2);
const UI_REQUEST_QUEUE_CAPACITY: usize = 16;
const UI_REQUEST_PENDING: u8 = 0;
const UI_REQUEST_CLAIMED: u8 = 1;
const UI_REQUEST_CANCELLED: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReleaseReason {
    Local,
    LivenessTimeout,
    IdleTimeout,
    Superseded,
}

impl ReleaseReason {
    fn status(self) -> SessionStatus {
        match self {
            Self::Local => SessionStatus::LocalInUse,
            Self::LivenessTimeout | Self::IdleTimeout => SessionStatus::Expired,
            Self::Superseded => SessionStatus::Superseded,
        }
    }
}

#[derive(Clone, Debug)]
struct OperationState {
    description: String,
    started: bool,
}

#[derive(Clone, Debug)]
struct ActiveSession {
    client_id: String,
    peer: SessionPeerInfo,
    connected_unix_ms: u64,
    pub(crate) connected_at: Duration,
    last_ping: Duration,
    last_activity: Duration,
    media_playing: bool,
    streaming: Option<StreamingRegistration>,
    request_count: u64,
    completed_count: u64,
    failed_count: u64,
    operations: BTreeMap<u64, OperationState>,
}

#[derive(Clone, Debug)]
struct StreamingRegistration {
    id: u64,
    playing: bool,
    cancel: Arc<AtomicBool>,
}

impl ActiveSession {
    fn cancel_streaming(&mut self) {
        if let Some(streaming) = self.streaming.take() {
            streaming.cancel.store(true, Ordering::Release);
        }
    }
}

#[derive(Debug)]
struct SessionStateMachine {
    active: Option<ActiveSession>,
    last_owner: Option<String>,
    last_release_reason: Option<ReleaseReason>,
    generation: u64,
    next_operation: u64,
    next_streaming_registration: u64,
    acquisition_sequence: u64,
    control_return_sequence: u64,
}

impl Default for SessionStateMachine {
    fn default() -> Self {
        Self {
            active: None,
            last_owner: None,
            last_release_reason: None,
            generation: 0,
            next_operation: 1,
            next_streaming_registration: 1,
            acquisition_sequence: 0,
            control_return_sequence: 0,
        }
    }
}

impl SessionStateMachine {
    fn acquire(
        &mut self,
        now: Duration,
        connected_unix_ms: u64,
        request: SessionAcquireRequest,
    ) -> SessionResponse {
        if let Some(active) = self.active.as_mut()
            && active.client_id == request.client_id
        {
            active.last_ping = now;
            active.last_activity = now;
            active.peer = request.peer;
            return SessionResponse::active();
        }

        if let Some(mut previous) = self.active.take() {
            previous.cancel_streaming();
            self.last_owner = Some(previous.client_id);
            self.last_release_reason = Some(ReleaseReason::Superseded);
        }
        self.generation = self.generation.wrapping_add(1);
        self.acquisition_sequence = self.acquisition_sequence.wrapping_add(1);
        self.last_owner = Some(request.client_id.clone());
        self.last_release_reason = None;
        self.active = Some(ActiveSession {
            client_id: request.client_id,
            peer: request.peer,
            connected_unix_ms,
            connected_at: now,
            last_ping: now,
            last_activity: now,
            media_playing: false,
            streaming: None,
            request_count: 0,
            completed_count: 0,
            failed_count: 0,
            operations: BTreeMap::new(),
        });
        SessionResponse::active()
    }

    fn ping(&mut self, now: Duration, request: &SessionPingRequest) -> SessionResponse {
        self.expire(now);
        let Some(active) = self.active.as_mut() else {
            return self.inactive_response(&request.client_id);
        };
        if active.client_id != request.client_id {
            return status_response(
                SessionStatus::Superseded,
                "別の端末で使用中です。操作すると操作権を取得します。",
            );
        }
        active.last_ping = now;
        active.media_playing = request.media_playing;
        if request.user_active {
            active.last_activity = now;
        }
        SessionResponse::active()
    }

    fn begin_operation(
        &mut self,
        now: Duration,
        client_id: &str,
        description: String,
    ) -> Result<(u64, u64), SessionResponse> {
        self.expire(now);
        let Some(active) = self.active.as_mut() else {
            return Err(self.inactive_response(client_id));
        };
        if active.client_id != client_id {
            return Err(status_response(
                SessionStatus::Superseded,
                "別の端末で使用中です。操作すると操作権を取得します。",
            ));
        }
        active.last_ping = now;
        active.last_activity = now;
        active.request_count = active.request_count.saturating_add(1);
        let token = self.next_operation;
        self.next_operation = self.next_operation.wrapping_add(1).max(1);
        active.operations.insert(
            token,
            OperationState {
                description,
                started: false,
            },
        );
        Ok((self.generation, token))
    }

    fn streaming_owner(&mut self, now: Duration, client_id: &str) -> Result<u64, SessionResponse> {
        self.expire(now);
        let Some(active) = self.active.as_mut() else {
            return Err(self.inactive_response(client_id));
        };
        if active.client_id != client_id {
            return Err(status_response(
                SessionStatus::Superseded,
                "別の端末で使用中です。操作すると操作権を取得します。",
            ));
        }
        active.last_ping = now;
        active.last_activity = now;
        Ok(self.generation)
    }

    fn register_streaming(
        &mut self,
        now: Duration,
        generation: u64,
        client_id: &str,
        cancel: Arc<AtomicBool>,
    ) -> Result<u64, SessionResponse> {
        self.expire(now);
        let Some(active) = self.active.as_mut() else {
            return Err(self.inactive_response(client_id));
        };
        if self.generation != generation || active.client_id != client_id {
            return Err(status_response(
                SessionStatus::Superseded,
                "別のリモート接続が操作権を取得しました。",
            ));
        }
        active.cancel_streaming();
        let id = self.next_streaming_registration;
        self.next_streaming_registration = self.next_streaming_registration.wrapping_add(1).max(1);
        active.streaming = Some(StreamingRegistration {
            id,
            playing: true,
            cancel,
        });
        active.last_ping = now;
        active.last_activity = now;
        Ok(id)
    }

    fn set_streaming_playing(
        &mut self,
        generation: u64,
        registration_id: u64,
        playing: bool,
    ) -> bool {
        let Some(streaming) = self
            .active
            .as_mut()
            .filter(|_| self.generation == generation)
            .and_then(|active| active.streaming.as_mut())
            .filter(|streaming| streaming.id == registration_id)
        else {
            return false;
        };
        streaming.playing = playing;
        true
    }

    fn note_segment_fetch(&mut self, now: Duration, generation: u64, registration_id: u64) -> bool {
        let Some(active) = self
            .active
            .as_mut()
            .filter(|_| self.generation == generation)
        else {
            return false;
        };
        if !active
            .streaming
            .as_ref()
            .is_some_and(|streaming| streaming.id == registration_id)
        {
            return false;
        }
        active.last_ping = now;
        true
    }

    fn unregister_streaming(&mut self, generation: u64, registration_id: u64) {
        let Some(active) = self
            .active
            .as_mut()
            .filter(|_| self.generation == generation)
        else {
            return;
        };
        if active
            .streaming
            .as_ref()
            .is_some_and(|streaming| streaming.id == registration_id)
        {
            active.streaming = None;
        }
    }

    fn start_operation(&mut self, generation: u64, token: u64) {
        if generation == self.generation
            && let Some(operation) = self
                .active
                .as_mut()
                .and_then(|active| active.operations.get_mut(&token))
        {
            operation.started = true;
        }
    }

    fn finish_operation(&mut self, generation: u64, token: u64, success: bool) {
        if generation != self.generation {
            return;
        }
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active.operations.remove(&token).is_none() {
            return;
        }
        if success {
            active.completed_count = active.completed_count.saturating_add(1);
        } else {
            active.failed_count = active.failed_count.saturating_add(1);
        }
    }

    fn local_disconnect(&mut self) {
        self.release(ReleaseReason::Local);
    }

    fn expire(&mut self, now: Duration) -> Option<ReleaseReason> {
        let reason = self.active.as_ref().and_then(|active| {
            if now.saturating_sub(active.last_ping) >= LIVENESS_TIMEOUT {
                Some(ReleaseReason::LivenessTimeout)
            } else if !active.media_playing
                && !active
                    .streaming
                    .as_ref()
                    .is_some_and(|streaming| streaming.playing)
                && now.saturating_sub(active.last_activity) >= IDLE_TIMEOUT
            {
                Some(ReleaseReason::IdleTimeout)
            } else {
                None
            }
        });
        if let Some(reason) = reason {
            self.release(reason);
        }
        reason
    }

    fn release(&mut self, reason: ReleaseReason) {
        let Some(mut active) = self.active.take() else {
            return;
        };
        active.cancel_streaming();
        self.last_owner = Some(active.client_id);
        self.last_release_reason = Some(reason);
        self.generation = self.generation.wrapping_add(1);
        if reason != ReleaseReason::Superseded {
            self.control_return_sequence = self.control_return_sequence.wrapping_add(1);
        }
    }

    fn inactive_response(&self, client_id: &str) -> SessionResponse {
        if self.last_owner.as_deref() == Some(client_id)
            && let Some(reason) = self.last_release_reason
        {
            return status_response(reason.status(), release_message(reason));
        }
        status_response(
            SessionStatus::NotAcquired,
            "リモートセッションを取得してください。",
        )
    }

    fn snapshot(&self) -> SessionSnapshot {
        let active = self.active.as_ref().map(|active| ActiveSessionSnapshot {
            peer: active.peer.clone(),
            connected_unix_ms: active.connected_unix_ms,
            elapsed: active.connected_at,
            request_count: active.request_count,
            completed_count: active.completed_count,
            failed_count: active.failed_count,
            queued_count: active
                .operations
                .values()
                .filter(|operation| !operation.started)
                .count(),
            running_count: active
                .operations
                .values()
                .filter(|operation| operation.started)
                .count(),
            current_operation: active
                .operations
                .iter()
                .rev()
                .find(|(_, operation)| operation.started)
                .or_else(|| active.operations.iter().rev().next())
                .map(|(_, operation)| operation.description.clone()),
            streaming: active.streaming.is_some(),
            streaming_playing: active
                .streaming
                .as_ref()
                .is_some_and(|streaming| streaming.playing),
        });
        SessionSnapshot {
            active,
            acquisition_sequence: self.acquisition_sequence,
            control_return_sequence: self.control_return_sequence,
            remote_web_connected: false,
            remote_web: None,
        }
    }
}

fn status_response(status: SessionStatus, message: impl Into<String>) -> SessionResponse {
    SessionResponse {
        status,
        message: message.into(),
    }
}

fn release_message(reason: ReleaseReason) -> &'static str {
    match reason {
        ReleaseReason::Local => "ローカルで使用中です。操作すると再接続します。",
        ReleaseReason::LivenessTimeout => "接続の生存確認が途絶えました。再接続してください。",
        ReleaseReason::IdleTimeout => "放置時間を超えたため切断されました。再接続してください。",
        ReleaseReason::Superseded => "別の端末で使用中です。操作すると操作権を取得します。",
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveSessionSnapshot {
    pub(crate) peer: SessionPeerInfo,
    pub(crate) connected_unix_ms: u64,
    pub(crate) elapsed: Duration,
    pub(crate) request_count: u64,
    pub(crate) completed_count: u64,
    pub(crate) failed_count: u64,
    pub(crate) queued_count: usize,
    pub(crate) running_count: usize,
    pub(crate) current_operation: Option<String>,
    #[allow(dead_code)] // Increment 6 VideoStreamState exposes these remote-owner facts.
    pub(crate) streaming: bool,
    #[allow(dead_code)] // Increment 6 VideoStreamState exposes these remote-owner facts.
    pub(crate) streaming_playing: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionSnapshot {
    pub(crate) active: Option<ActiveSessionSnapshot>,
    pub(crate) acquisition_sequence: u64,
    pub(crate) control_return_sequence: u64,
    pub(crate) remote_web_connected: bool,
    pub(crate) remote_web: Option<RemoteWebConnectionInfo>,
}

pub(crate) enum UiWriteOutcome {
    Write(RemoteWriteResponse),
    Session(SessionResponse),
}

pub(crate) struct VideoStreamPlaybackState {
    duration_secs: AtomicU64,
    volume: AtomicU64,
    play_intent: AtomicBool,
}

impl VideoStreamPlaybackState {
    pub(crate) fn new(duration_secs: f64, volume: f64, play_intent: bool) -> Self {
        Self {
            duration_secs: AtomicU64::new(duration_secs.to_bits()),
            volume: AtomicU64::new(volume.to_bits()),
            play_intent: AtomicBool::new(play_intent),
        }
    }

    pub(crate) fn update(&self, duration_secs: f64, volume: f64, play_intent: bool) {
        self.duration_secs
            .store(duration_secs.to_bits(), Ordering::Release);
        self.volume.store(volume.to_bits(), Ordering::Release);
        self.play_intent.store(play_intent, Ordering::Release);
    }

    pub(crate) fn set_play_intent(&self, play_intent: bool) {
        self.play_intent.store(play_intent, Ordering::Release);
    }

    pub(crate) fn set_volume(&self, volume: f64) {
        self.volume.store(volume.to_bits(), Ordering::Release);
    }

    pub(crate) fn snapshot(&self) -> VideoStreamPlaybackSnapshot {
        VideoStreamPlaybackSnapshot {
            duration_secs: f64::from_bits(self.duration_secs.load(Ordering::Acquire)),
            volume: f64::from_bits(self.volume.load(Ordering::Acquire)),
            play_intent: self.play_intent.load(Ordering::Acquire),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VideoStreamPlaybackSnapshot {
    pub(crate) duration_secs: f64,
    pub(crate) volume: f64,
    pub(crate) play_intent: bool,
}

#[derive(Clone)]
pub(crate) struct PublishedVideoStream {
    pub(crate) session: StreamingSessionId,
    pub(crate) generation: StreamingGenerationAccess,
    pub(crate) playback: Arc<VideoStreamPlaybackState>,
    pub(crate) buffer_target_secs: f64,
    pub(crate) end_behavior: mimageviewer_ipc::VideoStreamEndBehavior,
}

impl PublishedVideoStream {
    pub(crate) fn generation_id(&self) -> StreamingGeneration {
        self.generation.generation()
    }
}

#[derive(Clone)]
pub(crate) enum VideoStreamUiRequest {
    Start {
        client_id: String,
        path: PathBuf,
        quality: VideoStreamQuality,
        budget: VideoStreamStartBudget,
    },
    Control {
        session: u64,
        action: VideoStreamControlAction,
    },
    Seek {
        session: u64,
        position_secs: f64,
    },
    Thumbnail {
        session: u64,
        position_secs: Option<f64>,
    },
    Stop {
        session: u64,
    },
}

impl VideoStreamUiRequest {
    #[cfg(test)]
    pub(crate) fn start_for_test(
        client_id: String,
        path: PathBuf,
        quality: VideoStreamQuality,
    ) -> Self {
        Self::Start {
            client_id,
            path,
            quality,
            budget: VideoStreamStartBudget::from_enqueued_at(Instant::now()),
        }
    }
}

pub(crate) enum VideoStreamUiOutcome {
    Started(PublishedVideoStream),
    Controlled(SessionResponse),
    Seeked(StreamingGeneration),
    ThumbnailPending,
    ThumbnailReady(crate::video::thumbnail::Thumbnail),
    ThumbnailCleared,
    Stopped,
    Error(VideoStreamError),
}

struct UiRequestDispatch {
    state: AtomicU8,
}

struct QueuedRemoteWrite {
    request: RemoteWriteRequest,
    operation: SessionOperation,
    reply: mpsc::SyncSender<UiWriteOutcome>,
    dispatch: Arc<UiRequestDispatch>,
}

struct QueuedBookResumeRead {
    path: std::path::PathBuf,
    reply: mpsc::SyncSender<Option<usize>>,
    dispatch: Arc<UiRequestDispatch>,
}

struct QueuedVideoStreamUiRequest {
    request: VideoStreamUiRequest,
    operation: SessionOperation,
    reply: mpsc::SyncSender<VideoStreamUiOutcome>,
    dispatch: Arc<UiRequestDispatch>,
}

enum QueuedRemoteUiRequest {
    Write(QueuedRemoteWrite),
    BookResumeRead(QueuedBookResumeRead),
    VideoStream(QueuedVideoStreamUiRequest),
}

pub(crate) struct ClaimedRemoteWrite {
    request: RemoteWriteRequest,
    operation: SessionOperation,
    reply: mpsc::SyncSender<UiWriteOutcome>,
}

pub(crate) struct ClaimedBookResumeRead {
    path: std::path::PathBuf,
    reply: mpsc::SyncSender<Option<usize>>,
}

pub(crate) struct ClaimedVideoStreamUiRequest {
    request: VideoStreamUiRequest,
    operation: SessionOperation,
    reply: mpsc::SyncSender<VideoStreamUiOutcome>,
}

pub(crate) enum ClaimedRemoteUiRequest {
    Write(ClaimedRemoteWrite),
    BookResumeRead(ClaimedBookResumeRead),
    VideoStream(ClaimedVideoStreamUiRequest),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UiReadError {
    Busy,
    Timeout,
    Stopped,
}

#[derive(Clone)]
pub(crate) struct SessionHandle {
    inner: Arc<Mutex<SessionStateMachine>>,
    origin: Instant,
    repaint: Arc<Mutex<Option<egui::Context>>>,
    remote_web_connections: Arc<Mutex<BTreeMap<u64, Option<RemoteWebConnectionInfo>>>>,
    ui_request_tx: mpsc::SyncSender<QueuedRemoteUiRequest>,
    ui_request_rx: Arc<Mutex<mpsc::Receiver<QueuedRemoteUiRequest>>>,
    published_video_stream: Arc<Mutex<Option<PublishedVideoStream>>>,
}

impl UiRequestDispatch {
    fn pending() -> Self {
        Self {
            state: AtomicU8::new(UI_REQUEST_PENDING),
        }
    }

    fn try_claim(&self) -> bool {
        self.state
            .compare_exchange(
                UI_REQUEST_PENDING,
                UI_REQUEST_CLAIMED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn cancel_pending(&self) -> bool {
        self.state
            .compare_exchange(
                UI_REQUEST_PENDING,
                UI_REQUEST_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

impl ClaimedRemoteWrite {
    pub(crate) fn request(&self) -> &RemoteWriteRequest {
        &self.request
    }

    pub(crate) fn ownership_response(&self) -> SessionResponse {
        self.operation.ownership_response()
    }

    pub(crate) fn complete(self, outcome: UiWriteOutcome) {
        let success = matches!(
            outcome,
            UiWriteOutcome::Write(RemoteWriteResponse::Success(_))
        );
        self.operation.finish(success);
        let _ = self.reply.send(outcome);
    }
}

impl ClaimedBookResumeRead {
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub(crate) fn complete(self, page: Option<usize>) {
        let _ = self.reply.send(page);
    }
}

impl ClaimedVideoStreamUiRequest {
    pub(crate) fn request(&self) -> &VideoStreamUiRequest {
        &self.request
    }

    pub(crate) fn ownership_response(&self) -> SessionResponse {
        self.operation.ownership_response()
    }

    pub(crate) fn complete(self, outcome: VideoStreamUiOutcome) {
        let success = !matches!(outcome, VideoStreamUiOutcome::Error(_));
        self.operation.finish(success);
        let _ = self.reply.send(outcome);
    }
}

impl SessionHandle {
    pub(crate) fn new() -> Self {
        let (ui_request_tx, ui_request_rx) = mpsc::sync_channel(UI_REQUEST_QUEUE_CAPACITY);
        Self {
            inner: Arc::new(Mutex::new(SessionStateMachine::default())),
            origin: Instant::now(),
            repaint: Arc::new(Mutex::new(None)),
            remote_web_connections: Arc::new(Mutex::new(BTreeMap::new())),
            ui_request_tx,
            ui_request_rx: Arc::new(Mutex::new(ui_request_rx)),
            published_video_stream: Arc::new(Mutex::new(None)),
        }
    }

    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    pub(crate) fn acquire(&self, request: SessionAcquireRequest) -> SessionResponse {
        let peer_kind = request.peer.connection_kind;
        let peer_name = request
            .peer
            .device_name
            .clone()
            .map(|name| {
                name.chars()
                    .filter(|character| !character.is_control())
                    .take(128)
                    .collect()
            })
            .unwrap_or_else(|| "unknown".to_owned());
        let connected_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        let (response, owner_changed) = {
            let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            let generation = state.generation;
            let response = state.acquire(self.now(), connected_unix_ms, request);
            (response, state.generation != generation)
        };
        if owner_changed {
            self.clear_video_stream(None);
        }
        crate::logger::log(format!(
            "remote_ipc: session_acquired connection_kind={peer_kind:?} peer={peer_name}"
        ));
        self.notify_ui();
        response
    }

    pub(crate) fn ping(&self, request: &SessionPingRequest) -> SessionResponse {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .ping(self.now(), request)
    }

    pub(crate) fn begin_operation(
        &self,
        client_id: &str,
        description: String,
    ) -> Result<SessionOperation, SessionResponse> {
        let (generation, token) = self
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .begin_operation(self.now(), client_id, description)?;
        Ok(SessionOperation {
            handle: self.clone(),
            generation,
            token,
            client_id: client_id.to_owned(),
            finished: false,
        })
    }

    /// 長寿命の streaming session を現在の remote owner に結び付ける token を返す。
    /// IPC の start 要求は通常の client_id 検証後、この token だけを UI へ渡す。
    pub(crate) fn streaming_owner(
        &self,
        client_id: &str,
    ) -> Result<RemoteSessionOwner, SessionResponse> {
        let generation = self
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .streaming_owner(self.now(), client_id)?;
        Ok(RemoteSessionOwner {
            handle: self.clone(),
            generation,
            client_id: client_id.to_owned(),
        })
    }

    pub(crate) fn snapshot(&self) -> SessionSnapshot {
        let mut snapshot = self
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .snapshot();
        if let Some(active) = snapshot.active.as_mut() {
            active.elapsed = self.now().saturating_sub(active.elapsed);
        }
        let remote_web_connections = self
            .remote_web_connections
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        snapshot.remote_web_connected = !remote_web_connections.is_empty();
        snapshot.remote_web = remote_web_connections
            .iter()
            .rev()
            .find_map(|(_, info)| info.clone());
        snapshot
    }

    pub(crate) fn remote_web_connected(&self, connection_id: u64) {
        self.remote_web_connections
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(connection_id, None);
        self.notify_ui();
    }

    pub(crate) fn announce_remote_web(
        &self,
        connection_id: u64,
        info: RemoteWebConnectionInfo,
    ) -> bool {
        if !validate_remote_web_connection_info(&info) {
            return false;
        }
        let mut connections = self
            .remote_web_connections
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(connection) = connections.get_mut(&connection_id) else {
            return false;
        };
        *connection = Some(info);
        drop(connections);
        self.notify_ui();
        true
    }

    pub(crate) fn remote_web_disconnected(&self, connection_id: u64) {
        let removed = self
            .remote_web_connections
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&connection_id)
            .is_some();
        if removed {
            self.notify_ui();
        }
    }

    pub(crate) fn local_disconnect(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .local_disconnect();
        self.clear_video_stream(None);
        crate::logger::log("remote_ipc: session_released reason=local".to_owned());
        self.notify_ui();
    }

    pub(crate) fn submit_write(
        &self,
        request: RemoteWriteRequest,
        operation: SessionOperation,
    ) -> UiWriteOutcome {
        self.submit_write_with_timeout(request, operation, UI_REQUEST_ACCEPT_TIMEOUT)
    }

    fn submit_write_with_timeout(
        &self,
        request: RemoteWriteRequest,
        operation: SessionOperation,
        timeout: Duration,
    ) -> UiWriteOutcome {
        let dispatch = Arc::new(UiRequestDispatch::pending());
        let (reply, receiver) = mpsc::sync_channel(1);
        let queued = QueuedRemoteUiRequest::Write(QueuedRemoteWrite {
            request,
            operation,
            reply,
            dispatch: Arc::clone(&dispatch),
        });
        match self.ui_request_tx.try_send(queued) {
            Ok(()) => self.notify_ui(),
            Err(mpsc::TrySendError::Full(_)) => return write_busy_outcome(),
            Err(mpsc::TrySendError::Disconnected(_)) => return write_stopped_outcome(),
        }
        match receiver.recv_timeout(timeout) {
            Ok(outcome) => outcome,
            Err(mpsc::RecvTimeoutError::Disconnected) => write_stopped_outcome(),
            Err(mpsc::RecvTimeoutError::Timeout) if dispatch.cancel_pending() => {
                write_timeout_outcome()
            }
            // UI が期限内に claim 済みなら、同じ App-owned DB 操作の確定結果を返す。
            Err(mpsc::RecvTimeoutError::Timeout) => {
                receiver.recv().unwrap_or_else(|_| write_stopped_outcome())
            }
        }
    }

    /// App 所有の `book_resume_db` へ 1 件だけ問い合わせる。container worker は
    /// DB 接続を開かず、この UI request と現在の外側 session operation を使う。
    pub(crate) fn read_book_resume(
        &self,
        path: &std::path::Path,
    ) -> Result<Option<usize>, UiReadError> {
        self.read_book_resume_with_timeout(path, UI_REQUEST_ACCEPT_TIMEOUT)
    }

    fn read_book_resume_with_timeout(
        &self,
        path: &std::path::Path,
        timeout: Duration,
    ) -> Result<Option<usize>, UiReadError> {
        let dispatch = Arc::new(UiRequestDispatch::pending());
        let (reply, receiver) = mpsc::sync_channel(1);
        let queued = QueuedRemoteUiRequest::BookResumeRead(QueuedBookResumeRead {
            path: path.to_path_buf(),
            reply,
            dispatch: Arc::clone(&dispatch),
        });
        match self.ui_request_tx.try_send(queued) {
            Ok(()) => self.notify_ui(),
            Err(mpsc::TrySendError::Full(_)) => return Err(UiReadError::Busy),
            Err(mpsc::TrySendError::Disconnected(_)) => return Err(UiReadError::Stopped),
        }
        match receiver.recv_timeout(timeout) {
            Ok(page) => Ok(page),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(UiReadError::Stopped),
            Err(mpsc::RecvTimeoutError::Timeout) if dispatch.cancel_pending() => {
                Err(UiReadError::Timeout)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                receiver.recv().map_err(|_| UiReadError::Stopped)
            }
        }
    }

    pub(crate) fn submit_video_stream(
        &self,
        request: VideoStreamUiRequest,
        operation: SessionOperation,
    ) -> VideoStreamUiOutcome {
        let start_budget = match &request {
            VideoStreamUiRequest::Start { budget, .. } => Some(*budget),
            _ => None,
        };
        let accept_timeout = start_budget
            .map(VideoStreamStartBudget::remaining)
            .unwrap_or(UI_REQUEST_ACCEPT_TIMEOUT);
        let dispatch = Arc::new(UiRequestDispatch::pending());
        let (reply, receiver) = mpsc::sync_channel(1);
        let queued = QueuedRemoteUiRequest::VideoStream(QueuedVideoStreamUiRequest {
            request,
            operation,
            reply,
            dispatch: Arc::clone(&dispatch),
        });
        match self.ui_request_tx.try_send(queued) {
            Ok(()) => self.notify_ui(),
            Err(mpsc::TrySendError::Full(_)) => {
                return video_stream_ui_error(
                    VideoStreamErrorCode::Busy,
                    "本体 UI の動画操作 queue が混み合っています",
                );
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return video_stream_ui_error(
                    VideoStreamErrorCode::Internal,
                    "本体 UI の動画操作受付が停止しています",
                );
            }
        }
        match receiver.recv_timeout(accept_timeout) {
            Ok(outcome) => outcome,
            Err(mpsc::RecvTimeoutError::Disconnected) => video_stream_ui_error(
                VideoStreamErrorCode::Internal,
                "本体 UI の動画操作受付が停止しています",
            ),
            Err(mpsc::RecvTimeoutError::Timeout) if dispatch.cancel_pending() => match start_budget
            {
                Some(budget) => {
                    VideoStreamUiOutcome::Error(budget.timeout_error(VideoStreamStartStage::Ui))
                }
                None => video_stream_ui_error(
                    VideoStreamErrorCode::UiTimeout,
                    "本体 UI が 2 秒以内に動画操作要求を受理しませんでした",
                ),
            },
            Err(mpsc::RecvTimeoutError::Timeout) => receiver.recv().unwrap_or_else(|_| {
                video_stream_ui_error(
                    VideoStreamErrorCode::Internal,
                    "本体 UI の動画操作応答が停止しています",
                )
            }),
        }
    }

    pub(crate) fn publish_video_stream(&self, stream: PublishedVideoStream) {
        *self
            .published_video_stream
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(stream);
    }

    pub(crate) fn clear_video_stream(&self, session: Option<u64>) {
        let mut published = self
            .published_video_stream
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if session.is_none_or(|expected| {
            published
                .as_ref()
                .is_some_and(|stream| stream.session.0 == expected)
        }) {
            *published = None;
        }
    }

    pub(crate) fn video_stream(
        &self,
        session: u64,
    ) -> Result<PublishedVideoStream, VideoStreamError> {
        self.published_video_stream
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .filter(|stream| stream.session.0 == session)
            .cloned()
            .ok_or_else(|| {
                VideoStreamError::new(
                    VideoStreamErrorCode::SessionMismatch,
                    "動画ストリーミングセッションが一致しません",
                )
            })
    }

    pub(crate) fn take_pending_ui_requests(&self) -> Vec<ClaimedRemoteUiRequest> {
        let receiver = self
            .ui_request_rx
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut claimed = Vec::new();
        while let Ok(queued) = receiver.try_recv() {
            match queued {
                QueuedRemoteUiRequest::Write(queued) if queued.dispatch.try_claim() => {
                    queued.operation.started();
                    claimed.push(ClaimedRemoteUiRequest::Write(ClaimedRemoteWrite {
                        request: queued.request,
                        operation: queued.operation,
                        reply: queued.reply,
                    }));
                }
                QueuedRemoteUiRequest::BookResumeRead(queued) if queued.dispatch.try_claim() => {
                    claimed.push(ClaimedRemoteUiRequest::BookResumeRead(
                        ClaimedBookResumeRead {
                            path: queued.path,
                            reply: queued.reply,
                        },
                    ));
                }
                QueuedRemoteUiRequest::VideoStream(queued) if queued.dispatch.try_claim() => {
                    queued.operation.started();
                    claimed.push(ClaimedRemoteUiRequest::VideoStream(
                        ClaimedVideoStreamUiRequest {
                            request: queued.request,
                            operation: queued.operation,
                            reply: queued.reply,
                        },
                    ));
                }
                QueuedRemoteUiRequest::Write(_)
                | QueuedRemoteUiRequest::BookResumeRead(_)
                | QueuedRemoteUiRequest::VideoStream(_) => {}
            }
        }
        claimed
    }

    #[cfg(test)]
    fn take_pending_writes(&self) -> Vec<ClaimedRemoteWrite> {
        self.take_pending_ui_requests()
            .into_iter()
            .map(|request| match request {
                ClaimedRemoteUiRequest::Write(write) => write,
                ClaimedRemoteUiRequest::BookResumeRead(_) => {
                    panic!("book resume read reached a write-only test")
                }
                ClaimedRemoteUiRequest::VideoStream(_) => {
                    panic!("video stream request reached a write-only test")
                }
            })
            .collect()
    }

    pub(crate) fn install_repaint_context(&self, ctx: &egui::Context) {
        *self
            .repaint
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(ctx.clone());
    }

    fn notify_ui(&self) {
        if let Some(ctx) = self
            .repaint
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
        {
            ctx.request_repaint();
        }
    }

    fn expire(&self) -> Option<ReleaseReason> {
        let reason = self
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .expire(self.now());
        if reason.is_some() {
            self.clear_video_stream(None);
        }
        reason
    }
}

#[derive(Clone)]
pub(crate) struct RemoteSessionOwner {
    handle: SessionHandle,
    generation: u64,
    client_id: String,
}

impl RemoteSessionOwner {
    pub(crate) fn is_current(&self) -> bool {
        let state = self
            .handle
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.generation == self.generation
            && state
                .active
                .as_ref()
                .is_some_and(|active| active.client_id == self.client_id)
    }

    pub(crate) fn register_streaming(
        &self,
    ) -> Result<RemoteStreamingRegistration, SessionResponse> {
        let cancel = Arc::new(AtomicBool::new(false));
        let registration_id = self
            .handle
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .register_streaming(
                self.handle.now(),
                self.generation,
                &self.client_id,
                Arc::clone(&cancel),
            )?;
        Ok(RemoteStreamingRegistration {
            activity: RemoteStreamingActivity {
                handle: self.handle.clone(),
                generation: self.generation,
                registration_id,
            },
            cancel,
        })
    }
}

pub(crate) struct RemoteStreamingRegistration {
    activity: RemoteStreamingActivity,
    cancel: Arc<AtomicBool>,
}

impl RemoteStreamingRegistration {
    pub(crate) fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    pub(crate) fn activity(&self) -> RemoteStreamingActivity {
        self.activity.clone()
    }
}

impl Drop for RemoteStreamingRegistration {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.activity
            .handle
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .unregister_streaming(self.activity.generation, self.activity.registration_id);
    }
}

#[derive(Clone)]
pub(crate) struct RemoteStreamingActivity {
    handle: SessionHandle,
    generation: u64,
    registration_id: u64,
}

impl RemoteStreamingActivity {
    pub(crate) fn set_playing(&self, playing: bool) -> bool {
        self.handle
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .set_streaming_playing(self.generation, self.registration_id, playing)
    }

    #[allow(dead_code)] // Increment 6's dedicated segment lane reports successful fetch attempts.
    pub(crate) fn note_segment_fetch(&self) -> bool {
        self.handle
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .note_segment_fetch(self.handle.now(), self.generation, self.registration_id)
    }
}

fn validate_remote_web_connection_info(info: &RemoteWebConnectionInfo) -> bool {
    let url = info.public_url.trim();
    let lower = url.to_ascii_lowercase();
    let remainder = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    let Some(remainder) = remainder else {
        return false;
    };
    let authority = remainder.split('/').next().unwrap_or_default();
    url.len() <= 2048
        && !authority.is_empty()
        && !authority.contains('@')
        && !url.contains(['?', '#'])
        && !url.chars().any(char::is_control)
}

fn write_error_outcome(code: RemoteWriteErrorCode, message: &'static str) -> UiWriteOutcome {
    UiWriteOutcome::Write(RemoteWriteResponse::Error(RemoteWriteError::new(
        code, message,
    )))
}

fn write_busy_outcome() -> UiWriteOutcome {
    write_error_outcome(
        RemoteWriteErrorCode::Busy,
        "本体 UI への書き込み queue が混み合っています",
    )
}

fn write_timeout_outcome() -> UiWriteOutcome {
    write_error_outcome(
        RemoteWriteErrorCode::UiTimeout,
        "本体 UI が 2 秒以内に書き込み要求を受理しませんでした",
    )
}

fn write_stopped_outcome() -> UiWriteOutcome {
    write_error_outcome(
        RemoteWriteErrorCode::Internal,
        "本体 UI の書き込み受付が停止しています",
    )
}

fn video_stream_ui_error(
    code: VideoStreamErrorCode,
    message: &'static str,
) -> VideoStreamUiOutcome {
    VideoStreamUiOutcome::Error(VideoStreamError::new(code, message))
}

pub(crate) struct SessionOperation {
    handle: SessionHandle,
    generation: u64,
    token: u64,
    client_id: String,
    finished: bool,
}

impl SessionOperation {
    pub(crate) fn started(&self) {
        self.handle
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .start_operation(self.generation, self.token);
    }

    pub(crate) fn finish(mut self, success: bool) {
        self.handle
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .finish_operation(self.generation, self.token, success);
        self.finished = true;
    }

    pub(crate) fn ownership_response(&self) -> SessionResponse {
        let state = self
            .handle
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match state.active.as_ref() {
            Some(active)
                if state.generation == self.generation && active.client_id == self.client_id =>
            {
                SessionResponse::active()
            }
            Some(_) => status_response(
                SessionStatus::Superseded,
                "別のリモート接続が操作権を取得しました。",
            ),
            None => state.inactive_response(&self.client_id),
        }
    }
}

impl Drop for SessionOperation {
    fn drop(&mut self) {
        if !self.finished {
            self.handle
                .inner
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .finish_operation(self.generation, self.token, false);
        }
    }
}

pub(super) struct SessionRuntime {
    handle: SessionHandle,
    stop: Arc<AtomicBool>,
    watcher: Option<std::thread::JoinHandle<()>>,
}

impl SessionRuntime {
    pub(super) fn start() -> Result<Self, String> {
        let handle = SessionHandle::new();
        let stop = Arc::new(AtomicBool::new(false));
        let watcher_handle = handle.clone();
        let watcher_stop = Arc::clone(&stop);
        let watcher = std::thread::Builder::new()
            .name("remote-session-watchdog".to_owned())
            .spawn(move || session_watchdog(watcher_handle, watcher_stop))
            .map_err(|error| format!("remote session watchdog を開始できません: {error}"))?;
        Ok(Self {
            handle,
            stop,
            watcher: Some(watcher),
        })
    }

    pub(super) fn handle(&self) -> SessionHandle {
        self.handle.clone()
    }
}

impl Drop for SessionRuntime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
    }
}

fn session_watchdog(handle: SessionHandle, stop: Arc<AtomicBool>) {
    let mut sleep_prevented = false;
    while !stop.load(Ordering::Acquire) {
        if let Some(reason) = handle.expire() {
            crate::logger::log(format!(
                "remote_ipc: session_released reason={}",
                match reason {
                    ReleaseReason::LivenessTimeout => "liveness_timeout",
                    ReleaseReason::IdleTimeout => "idle_timeout",
                    ReleaseReason::Local => "local",
                    ReleaseReason::Superseded => "superseded",
                }
            ));
            handle.notify_ui();
        }
        let active = handle.snapshot().active.is_some();
        if active != sleep_prevented {
            set_sleep_prevention(active);
            sleep_prevented = active;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if sleep_prevented {
        set_sleep_prevention(false);
    }
}

#[cfg(windows)]
fn set_sleep_prevention(active: bool) {
    use windows::Win32::System::Power::{
        ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };
    let flags = if active {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    let ok = unsafe { SetThreadExecutionState(flags) }.0 != 0;
    crate::logger::log(format!(
        "remote_ipc: sleep_prevention active={active} result={}",
        if ok { "ok" } else { "error" }
    ));
}

#[cfg(not(windows))]
fn set_sleep_prevention(_active: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_request() -> RemoteWriteRequest {
        RemoteWriteRequest::SetSpread {
            address: mimageviewer_ipc::RemoteAddress::file(
                "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2",
                "books/book.pdf",
            ),
            spread_mode: mimageviewer_ipc::RemoteSpreadMode::RtlCover,
            reading_direction: mimageviewer_ipc::RemoteReadingDirection::Rtl,
        }
    }

    fn write_requests() -> Vec<RemoteWriteRequest> {
        let favorite = "30d6c167-7148-4f3e-9a5a-21c5fd31ecb2";
        let container = mimageviewer_ipc::RemoteAddress::file(favorite, "books/book.pdf");
        let page = mimageviewer_ipc::RemoteAddress {
            favorite_id: favorite.to_owned(),
            relative_path: "books/book.pdf".to_owned(),
            subresource: mimageviewer_ipc::RemoteSubresource::PdfPage { page_number: 3 },
        };
        vec![
            write_request(),
            RemoteWriteRequest::RecordReadingProgress {
                address: page.clone(),
                context_address: container.clone(),
                page_index: 3,
                page_number: 4,
                page_count: 10,
                record_resume: true,
                record_history: true,
            },
            RemoteWriteRequest::SetRating {
                address: page.clone(),
                stars: 4,
            },
            RemoteWriteRequest::SetBookmark {
                address: page.clone(),
                context_address: container.clone(),
                page_index: 3,
                bookmarked: true,
            },
            RemoteWriteRequest::GetItemState {
                address: page,
                context_address: container,
                page_index: 3,
                bookmark_supported: true,
            },
        ]
    }

    fn peer() -> SessionPeerInfo {
        SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: Some("phone".to_owned()),
        }
    }

    fn acquire(state: &mut SessionStateMachine, now: Duration) {
        assert_eq!(
            state
                .acquire(
                    now,
                    1,
                    SessionAcquireRequest {
                        client_id: "client".to_owned(),
                        peer: peer(),
                    },
                )
                .status,
            SessionStatus::Active
        );
    }

    #[test]
    fn liveness_timeout_releases_session() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);
        assert_eq!(
            state.expire(LIVENESS_TIMEOUT),
            Some(ReleaseReason::LivenessTimeout)
        );
        assert!(state.active.is_none());
    }

    #[test]
    fn idle_timeout_releases_only_when_not_playing() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);
        let almost_liveness = LIVENESS_TIMEOUT - Duration::from_secs(1);
        let mut now = Duration::ZERO;
        while now < IDLE_TIMEOUT {
            now += almost_liveness;
            let response = state.ping(
                now,
                &SessionPingRequest {
                    client_id: "client".to_owned(),
                    user_active: false,
                    media_playing: true,
                },
            );
            assert_eq!(response.status, SessionStatus::Active);
        }
        assert_eq!(state.expire(now), None);
        state.ping(
            now,
            &SessionPingRequest {
                client_id: "client".to_owned(),
                user_active: false,
                media_playing: false,
            },
        );
        assert_eq!(state.expire(now), Some(ReleaseReason::IdleTimeout));
    }

    fn register_test_stream(
        state: &mut SessionStateMachine,
        now: Duration,
    ) -> (u64, u64, Arc<AtomicBool>) {
        let generation = state.streaming_owner(now, "client").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let registration = state
            .register_streaming(now, generation, "client", Arc::clone(&cancel))
            .unwrap();
        (generation, registration, cancel)
    }

    #[test]
    fn ownership_loss_cancels_registered_streaming() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);
        let (_, _, cancel) = register_test_stream(&mut state, Duration::ZERO);

        state.acquire(
            Duration::from_secs(1),
            2,
            SessionAcquireRequest {
                client_id: "other-client".to_owned(),
                peer: peer(),
            },
        );

        assert!(cancel.load(Ordering::Acquire));
    }

    #[test]
    fn local_control_return_cancels_registered_streaming() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);
        let (_, _, cancel) = register_test_stream(&mut state, Duration::ZERO);

        state.local_disconnect();

        assert!(cancel.load(Ordering::Acquire));
        assert!(state.active.is_none());
        assert_eq!(state.last_release_reason, Some(ReleaseReason::Local));
    }

    #[test]
    fn liveness_timeout_cancels_registered_streaming() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);
        let (_, _, cancel) = register_test_stream(&mut state, Duration::ZERO);

        assert_eq!(
            state.expire(LIVENESS_TIMEOUT),
            Some(ReleaseReason::LivenessTimeout)
        );
        assert!(cancel.load(Ordering::Acquire));
    }

    #[test]
    fn paused_streaming_idle_timeout_cancels_even_when_segment_fetches_keep_liveness() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);
        let (generation, registration, cancel) = register_test_stream(&mut state, Duration::ZERO);
        assert!(state.set_streaming_playing(generation, registration, false));

        let mut now = Duration::from_secs(30);
        while now < IDLE_TIMEOUT {
            assert!(state.note_segment_fetch(now, generation, registration));
            now += Duration::from_secs(30);
        }
        assert_eq!(state.expire(IDLE_TIMEOUT), Some(ReleaseReason::IdleTimeout));
        assert!(cancel.load(Ordering::Acquire));
    }

    #[test]
    fn playing_streaming_suppresses_idle_and_segment_fetch_counts_for_liveness() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);
        let (generation, registration, cancel) = register_test_stream(&mut state, Duration::ZERO);

        let mut now = Duration::from_secs(30);
        while now <= IDLE_TIMEOUT + Duration::from_secs(30) {
            assert!(state.note_segment_fetch(now, generation, registration));
            assert_eq!(state.expire(now), None);
            now += Duration::from_secs(30);
        }
        assert!(!cancel.load(Ordering::Acquire));
    }

    #[test]
    fn api_request_counts_as_activity() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);
        let now = IDLE_TIMEOUT - Duration::from_secs(1);
        for seconds in (30..now.as_secs()).step_by(30) {
            assert_eq!(
                state
                    .ping(
                        Duration::from_secs(seconds),
                        &SessionPingRequest {
                            client_id: "client".to_owned(),
                            user_active: false,
                            media_playing: false,
                        },
                    )
                    .status,
                SessionStatus::Active
            );
        }
        let (generation, token) = state
            .begin_operation(now, "client", "一覧を取得中".to_owned())
            .unwrap();
        state.finish_operation(generation, token, true);
        assert_eq!(state.expire(now + Duration::from_secs(2)), None);
    }

    #[test]
    fn remote_can_reacquire_after_local_disconnect() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);
        state.local_disconnect();
        assert_eq!(
            state
                .begin_operation(Duration::from_secs(1), "client", "test".to_owned())
                .unwrap_err()
                .status,
            SessionStatus::LocalInUse
        );
        acquire(&mut state, Duration::from_secs(2));
        assert!(
            state
                .begin_operation(Duration::from_secs(3), "client", "test".to_owned())
                .is_ok()
        );
    }

    #[test]
    fn write_without_session_is_rejected_before_ui_queue() {
        for request in write_requests() {
            let handle = SessionHandle::new();
            let response = match handle
                .begin_operation("client", format!("{} を適用中", request.kind_name()))
            {
                Ok(_) => panic!(
                    "{} unexpectedly acquired a missing session",
                    request.kind_name()
                ),
                Err(response) => response,
            };
            assert_eq!(
                response.status,
                SessionStatus::NotAcquired,
                "{}",
                request.kind_name()
            );
            assert!(handle.take_pending_writes().is_empty());
        }
    }

    #[test]
    fn every_write_kind_reaches_ui_and_returns_its_application_result() {
        for request in write_requests() {
            let kind = request.kind_name();
            let handle = SessionHandle::new();
            handle.acquire(SessionAcquireRequest {
                client_id: "client".to_owned(),
                peer: peer(),
            });
            let worker_handle = handle.clone();
            let worker = std::thread::spawn(move || {
                let operation = worker_handle
                    .begin_operation("client", format!("{kind} を適用中"))
                    .unwrap();
                worker_handle.submit_write(request, operation)
            });
            let deadline = Instant::now() + Duration::from_secs(1);
            let claimed = loop {
                if let Some(claimed) = handle.take_pending_writes().pop() {
                    break claimed;
                }
                assert!(Instant::now() < deadline, "{kind}");
                std::thread::yield_now();
            };
            assert_eq!(claimed.request().kind_name(), kind);
            assert_eq!(claimed.ownership_response().status, SessionStatus::Active);
            claimed.complete(UiWriteOutcome::Write(RemoteWriteResponse::Success(
                mimageviewer_ipc::RemoteWriteResult::applied(),
            )));
            assert!(matches!(
                worker.join().unwrap(),
                UiWriteOutcome::Write(RemoteWriteResponse::Success(_))
            ));
        }
    }

    #[test]
    fn book_resume_read_reaches_the_same_bounded_ui_queue() {
        let handle = SessionHandle::new();
        handle.acquire(SessionAcquireRequest {
            client_id: "client".to_owned(),
            peer: peer(),
        });
        let worker_handle = handle.clone();
        let worker = std::thread::spawn(move || {
            worker_handle.read_book_resume(std::path::Path::new("C:/books/book.zip"))
        });

        let claimed = loop {
            if let Some(request) = handle.take_pending_ui_requests().pop() {
                break request;
            }
            std::thread::yield_now();
        };
        let ClaimedRemoteUiRequest::BookResumeRead(read) = claimed else {
            panic!("book resume read was routed as a write")
        };
        assert_eq!(read.path(), std::path::Path::new("C:/books/book.zip"));
        read.complete(Some(7));
        assert_eq!(worker.join().unwrap(), Ok(Some(7)));
    }

    #[test]
    fn every_write_kind_is_rejected_if_ownership_is_lost_before_ui_apply() {
        for request in write_requests() {
            let kind = request.kind_name();
            let handle = SessionHandle::new();
            handle.acquire(SessionAcquireRequest {
                client_id: "client".to_owned(),
                peer: peer(),
            });
            let worker_handle = handle.clone();
            let worker = std::thread::spawn(move || {
                let operation = worker_handle
                    .begin_operation("client", format!("{kind} を適用中"))
                    .unwrap();
                worker_handle.submit_write(request, operation)
            });
            let deadline = Instant::now() + Duration::from_secs(1);
            let claimed = loop {
                if let Some(claimed) = handle.take_pending_writes().pop() {
                    break claimed;
                }
                assert!(Instant::now() < deadline, "{kind}");
                std::thread::yield_now();
            };
            handle.local_disconnect();
            let ownership = claimed.ownership_response();
            assert_eq!(ownership.status, SessionStatus::LocalInUse, "{kind}");
            claimed.complete(UiWriteOutcome::Session(ownership));
            assert!(matches!(
                worker.join().unwrap(),
                UiWriteOutcome::Session(SessionResponse {
                    status: SessionStatus::LocalInUse,
                    ..
                })
            ));
        }
    }

    #[test]
    fn ui_claimed_write_returns_its_application_result() {
        let handle = SessionHandle::new();
        handle.acquire(SessionAcquireRequest {
            client_id: "client".to_owned(),
            peer: peer(),
        });
        let worker_handle = handle.clone();
        let worker = std::thread::spawn(move || {
            let operation = worker_handle
                .begin_operation("client", "見開き設定を書き込み中".to_owned())
                .unwrap();
            worker_handle.submit_write(write_request(), operation)
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        let claimed = loop {
            if let Some(claimed) = handle.take_pending_writes().pop() {
                break claimed;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert_eq!(claimed.ownership_response().status, SessionStatus::Active);
        claimed.complete(UiWriteOutcome::Write(RemoteWriteResponse::Success(
            mimageviewer_ipc::RemoteWriteResult::applied(),
        )));
        assert!(matches!(
            worker.join().unwrap(),
            UiWriteOutcome::Write(RemoteWriteResponse::Success(_))
        ));
    }

    #[test]
    fn ui_write_timeout_cancels_before_late_drain() {
        let handle = SessionHandle::new();
        handle.acquire(SessionAcquireRequest {
            client_id: "client".to_owned(),
            peer: peer(),
        });
        let operation = handle
            .begin_operation("client", "見開き設定を書き込み中".to_owned())
            .unwrap();
        let outcome =
            handle.submit_write_with_timeout(write_request(), operation, Duration::from_millis(10));
        assert!(matches!(
            outcome,
            UiWriteOutcome::Write(RemoteWriteResponse::Error(RemoteWriteError {
                code: RemoteWriteErrorCode::UiTimeout,
                ..
            }))
        ));
        assert!(handle.take_pending_writes().is_empty());
    }

    #[test]
    fn in_flight_operation_observes_local_disconnect() {
        let handle = SessionHandle::new();
        handle.acquire(SessionAcquireRequest {
            client_id: "client".to_owned(),
            peer: peer(),
        });
        let operation = handle.begin_operation("client", "test".to_owned()).unwrap();
        handle.local_disconnect();
        assert_eq!(
            operation.ownership_response().status,
            SessionStatus::LocalInUse
        );
    }

    #[test]
    fn later_remote_client_supersedes_the_previous_owner() {
        let handle = SessionHandle::new();
        handle.acquire(SessionAcquireRequest {
            client_id: "client-a".to_owned(),
            peer: peer(),
        });
        let old_operation = handle
            .begin_operation("client-a", "old".to_owned())
            .unwrap();
        handle.acquire(SessionAcquireRequest {
            client_id: "client-b".to_owned(),
            peer: peer(),
        });
        assert_eq!(
            old_operation.ownership_response().status,
            SessionStatus::Superseded
        );
        assert!(handle.begin_operation("client-b", "new".to_owned()).is_ok());
    }

    #[test]
    fn acquisition_sequence_changes_only_when_the_owner_changes() {
        let handle = SessionHandle::new();
        let request = |client_id: &str| SessionAcquireRequest {
            client_id: client_id.to_owned(),
            peer: peer(),
        };
        handle.acquire(request("client-a"));
        let first = handle.snapshot().acquisition_sequence;
        handle.acquire(request("client-a"));
        assert_eq!(handle.snapshot().acquisition_sequence, first);
        handle.acquire(request("client-b"));
        assert_eq!(handle.snapshot().acquisition_sequence, first + 1);
        assert_eq!(
            handle
                .ping(&SessionPingRequest {
                    client_id: "client-a".to_owned(),
                    user_active: true,
                    media_playing: false,
                })
                .status,
            SessionStatus::Superseded
        );
    }

    #[test]
    fn remote_web_connection_url_rejects_credentials_and_query_values() {
        let handle = SessionHandle::new();
        let info = |url: &str| RemoteWebConnectionInfo {
            public_url: url.to_owned(),
            tailscale_serve: mimageviewer_ipc::RemoteWebFeatureStatus::Configured,
            pin_configured: true,
        };
        handle.remote_web_connected(1);
        assert!(handle.snapshot().remote_web_connected);
        assert!(handle.snapshot().remote_web.is_none());
        assert!(handle.announce_remote_web(1, info("https://viewer.example/")));
        handle.remote_web_connected(2);
        handle.remote_web_connected(3);
        assert!(!handle.announce_remote_web(2, info("https://viewer.example/?t=secret")));
        assert!(!handle.announce_remote_web(3, info("https://user:secret@viewer.example/")));
        assert_eq!(
            handle.snapshot().remote_web.unwrap().public_url,
            "https://viewer.example/"
        );
        handle.remote_web_disconnected(1);
        assert!(handle.snapshot().remote_web.is_none());
        assert!(handle.snapshot().remote_web_connected);
        handle.remote_web_disconnected(2);
        handle.remote_web_disconnected(3);
        assert!(!handle.snapshot().remote_web_connected);
    }
}
