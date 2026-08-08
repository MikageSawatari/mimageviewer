use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mimageviewer_ipc::{
    RemoteSessionIdentity, RemoteWebConnectionInfo, RemoteWriteError, RemoteWriteErrorCode,
    RemoteWriteRequest, RemoteWriteResponse, SessionAcquireRequest, SessionPeerInfo,
    SessionPingRequest, SessionResponse, SessionStatus, VideoStreamControlAction, VideoStreamError,
    VideoStreamErrorCode, VideoStreamQuality,
};
use uuid::Uuid;

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
    AcquireBarrierTimeout,
    LivenessTimeout,
    IdleTimeout,
    BackgroundExpired,
    Superseded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteClientPresence {
    Foreground,
    Detached { since: Duration },
}

/// PC と remote の排他的な操作権 lifecycle。
///
/// `active: Option<_>` や複数の bool から状態を推測せず、この phase だけを正本にする。
/// App を生成しない unit test で遷移規則を固定できるよう、時刻・I/O・worker を持たない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RemoteControlPhase {
    #[default]
    Local,
    AcquiringRemote,
    RemoteActive,
    DrainingRemote,
}

impl RemoteControlPhase {
    pub(crate) fn blocks_local_control(self) -> bool {
        !matches!(self, Self::Local)
    }

    pub(crate) fn accepts_remote_work(self) -> bool {
        matches!(self, Self::RemoteActive)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RemoteControlLifecycle {
    phase: RemoteControlPhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteControlTransition {
    BeginAcquire,
    FinishAcquire,
    BeginDrain,
    FinishDrain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemoteControlTransitionError {
    phase: RemoteControlPhase,
    transition: RemoteControlTransition,
}

impl RemoteControlLifecycle {
    fn transition(
        &mut self,
        transition: RemoteControlTransition,
    ) -> Result<(), RemoteControlTransitionError> {
        let next = match (self.phase, transition) {
            (RemoteControlPhase::Local, RemoteControlTransition::BeginAcquire) => {
                RemoteControlPhase::AcquiringRemote
            }
            (RemoteControlPhase::AcquiringRemote, RemoteControlTransition::FinishAcquire) => {
                RemoteControlPhase::RemoteActive
            }
            (
                RemoteControlPhase::AcquiringRemote | RemoteControlPhase::RemoteActive,
                RemoteControlTransition::BeginDrain,
            ) => RemoteControlPhase::DrainingRemote,
            (RemoteControlPhase::DrainingRemote, RemoteControlTransition::FinishDrain) => {
                RemoteControlPhase::Local
            }
            (phase, transition) => {
                return Err(RemoteControlTransitionError { phase, transition });
            }
        };
        self.phase = next;
        Ok(())
    }
}

impl ReleaseReason {
    fn status(self) -> SessionStatus {
        match self {
            Self::Local | Self::AcquireBarrierTimeout => SessionStatus::LocalInUse,
            Self::LivenessTimeout | Self::IdleTimeout | Self::BackgroundExpired => {
                SessionStatus::Expired
            }
            Self::Superseded => SessionStatus::Superseded,
        }
    }
}

#[derive(Clone, Debug)]
struct OperationState {
    description: String,
    started: bool,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
struct ActiveSession {
    client_id: String,
    session_id: String,
    peer: SessionPeerInfo,
    connected_unix_ms: u64,
    pub(crate) connected_at: Duration,
    last_ping: Duration,
    last_activity: Duration,
    presence: RemoteClientPresence,
    media_playing: bool,
    streaming: Option<StreamingRegistration>,
    streaming_workers: BTreeMap<u64, Arc<AtomicBool>>,
    request_count: u64,
    completed_count: u64,
    failed_count: u64,
    operations: BTreeMap<u64, OperationState>,
}

#[derive(Clone, Debug)]
struct StreamingRegistration {
    id: u64,
    playing: bool,
}

impl ActiveSession {
    fn cancel_streaming(&mut self) {
        for cancel in self.streaming_workers.values() {
            cancel.store(true, Ordering::Release);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteStreamingControlError {
    SessionMissing,
    SessionGenerationMismatch,
    RegistrationMissing,
    RegistrationMismatch,
}

impl RemoteStreamingControlError {
    pub(crate) fn is_session_mismatch(self) -> bool {
        matches!(self, Self::SessionMissing | Self::SessionGenerationMismatch)
    }
}

#[derive(Debug)]
struct SessionStateMachine {
    lifecycle: RemoteControlLifecycle,
    active: Option<ActiveSession>,
    last_owner: Option<String>,
    last_release_reason: Option<ReleaseReason>,
    generation: u64,
    next_operation: u64,
    next_streaming_registration: u64,
    acquisition_sequence: u64,
    control_return_sequence: u64,
    drain_reason: Option<ReleaseReason>,
    app_drain_complete: bool,
}

impl Default for SessionStateMachine {
    fn default() -> Self {
        Self {
            lifecycle: RemoteControlLifecycle::default(),
            active: None,
            last_owner: None,
            last_release_reason: None,
            generation: 0,
            next_operation: 1,
            next_streaming_registration: 1,
            acquisition_sequence: 0,
            control_return_sequence: 0,
            drain_reason: None,
            app_drain_complete: false,
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
        match self.lifecycle.phase {
            // The previous owner's resources still belong to that drain. A new acquire is a
            // waiter, not a second BeginDrain transition, and must not replace its reason.
            RemoteControlPhase::DrainingRemote => return session_closing_response(),
            RemoteControlPhase::AcquiringRemote | RemoteControlPhase::RemoteActive => {
                self.begin_drain(ReleaseReason::Superseded);
                return session_closing_response();
            }
            RemoteControlPhase::Local => {}
        }
        if self.active.is_some() {
            crate::logger::log(
                "remote_ipc: lifecycle invariant violation: local phase has active session payload"
                    .to_owned(),
            );
            return session_closing_response();
        }
        if self
            .lifecycle
            .transition(RemoteControlTransition::BeginAcquire)
            .is_err()
        {
            return session_closing_response();
        }
        self.generation = self.generation.wrapping_add(1);
        self.acquisition_sequence = self.acquisition_sequence.wrapping_add(1);
        self.last_owner = Some(request.client_id.clone());
        self.last_release_reason = None;
        self.drain_reason = None;
        self.app_drain_complete = false;
        self.active = Some(ActiveSession {
            client_id: request.client_id,
            session_id: new_session_id(),
            peer: request.peer,
            connected_unix_ms,
            connected_at: now,
            last_ping: now,
            last_activity: now,
            presence: RemoteClientPresence::Foreground,
            media_playing: false,
            streaming: None,
            streaming_workers: BTreeMap::new(),
            request_count: 0,
            completed_count: 0,
            failed_count: 0,
            operations: BTreeMap::new(),
        });
        SessionResponse::active(
            self.active
                .as_ref()
                .expect("active session was just installed")
                .session_id
                .clone(),
        )
    }

    fn finish_acquire(&mut self, generation: u64) -> bool {
        generation == self.generation
            && self.active.is_some()
            && self
                .lifecycle
                .transition(RemoteControlTransition::FinishAcquire)
                .is_ok()
    }

    fn abort_acquire_barrier(&mut self, generation: u64) -> bool {
        if generation != self.generation
            || self.lifecycle.phase != RemoteControlPhase::AcquiringRemote
        {
            return false;
        }
        self.begin_drain(ReleaseReason::AcquireBarrierTimeout)
    }

    fn ping(&mut self, now: Duration, request: &SessionPingRequest) -> SessionResponse {
        self.expire(now);
        let Some(active) = self.active.as_ref() else {
            return self.inactive_response(&request.owner.client_id);
        };
        if active.client_id != request.owner.client_id
            || active.session_id != request.owner.session_id
        {
            return status_response(
                SessionStatus::Superseded,
                "この接続の操作権は無効です。再接続してください。",
            );
        }
        if self.lifecycle.phase == RemoteControlPhase::DrainingRemote {
            return self.draining_owner_response();
        }
        let active = self
            .active
            .as_mut()
            .expect("active session was just checked");
        active.last_ping = now;
        active.presence = RemoteClientPresence::Foreground;
        active.media_playing = request.media_playing;
        if request.user_active {
            active.last_activity = now;
        }
        SessionResponse::active(active.session_id.clone())
    }

    fn note_ai_client_seen(&mut self, now: Duration, owner: &RemoteSessionIdentity) -> bool {
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        if active.client_id != owner.client_id
            || active.session_id != owner.session_id
            || self.lifecycle.phase == RemoteControlPhase::DrainingRemote
        {
            return false;
        }
        active.last_ping = now;
        active.last_activity = now;
        active.presence = RemoteClientPresence::Foreground;
        true
    }

    fn begin_operation(
        &mut self,
        now: Duration,
        owner: &RemoteSessionIdentity,
        description: String,
    ) -> Result<(u64, u64, Arc<AtomicBool>), SessionResponse> {
        self.expire(now);
        let Some(active) = self.active.as_ref() else {
            return Err(self.inactive_response(&owner.client_id));
        };
        if active.client_id != owner.client_id || active.session_id != owner.session_id {
            return Err(status_response(
                SessionStatus::Superseded,
                "この接続の操作権は無効です。再接続してください。",
            ));
        }
        if self.lifecycle.phase == RemoteControlPhase::DrainingRemote {
            return Err(self.draining_owner_response());
        }
        let active = self
            .active
            .as_mut()
            .expect("active session was just checked");
        active.last_ping = now;
        active.last_activity = now;
        active.presence = RemoteClientPresence::Foreground;
        active.request_count = active.request_count.saturating_add(1);
        let token = self.next_operation;
        self.next_operation = self.next_operation.wrapping_add(1).max(1);
        let cancel = Arc::new(AtomicBool::new(false));
        active.operations.insert(
            token,
            OperationState {
                description,
                started: false,
                cancel: Arc::clone(&cancel),
            },
        );
        Ok((self.generation, token, cancel))
    }

    fn streaming_owner(
        &mut self,
        now: Duration,
        owner: &RemoteSessionIdentity,
    ) -> Result<u64, SessionResponse> {
        self.expire(now);
        let Some(active) = self.active.as_ref() else {
            return Err(self.inactive_response(&owner.client_id));
        };
        if active.client_id != owner.client_id || active.session_id != owner.session_id {
            return Err(status_response(
                SessionStatus::Superseded,
                "この接続の操作権は無効です。再接続してください。",
            ));
        }
        if !self.lifecycle.phase.accepts_remote_work() {
            return Err(self.draining_owner_response());
        }
        let active = self
            .active
            .as_mut()
            .expect("active session was just checked");
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
        if !self.lifecycle.phase.accepts_remote_work() {
            return Err(self.draining_owner_response());
        }
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
        active.streaming = Some(StreamingRegistration { id, playing: true });
        active.streaming_workers.insert(id, cancel);
        active.last_ping = now;
        active.last_activity = now;
        Ok(id)
    }

    fn set_streaming_playing(
        &mut self,
        generation: u64,
        registration_id: u64,
        playing: bool,
    ) -> Result<(), RemoteStreamingControlError> {
        let Some(active) = self.active.as_mut() else {
            return Err(RemoteStreamingControlError::SessionMissing);
        };
        if self.generation != generation {
            return Err(RemoteStreamingControlError::SessionGenerationMismatch);
        }
        let Some(streaming) = active.streaming.as_mut() else {
            return Err(RemoteStreamingControlError::RegistrationMissing);
        };
        if streaming.id != registration_id {
            return Err(RemoteStreamingControlError::RegistrationMismatch);
        }
        streaming.playing = playing;
        Ok(())
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

    fn unregister_streaming(&mut self, generation: u64, registration_id: u64) -> bool {
        let Some(active) = self
            .active
            .as_mut()
            .filter(|_| self.generation == generation)
        else {
            return false;
        };
        if active
            .streaming
            .as_ref()
            .is_some_and(|streaming| streaming.id == registration_id)
        {
            active.streaming = None;
            return self.try_finish_drain();
        }
        false
    }

    fn finish_streaming_worker(&mut self, generation: u64, registration_id: u64) -> bool {
        let Some(active) = self
            .active
            .as_mut()
            .filter(|_| self.generation == generation)
        else {
            return false;
        };
        active.streaming_workers.remove(&registration_id);
        self.try_finish_drain()
    }

    fn start_operation(&mut self, generation: u64, token: u64) -> bool {
        if self.lifecycle.phase.accepts_remote_work()
            && generation == self.generation
            && let Some(operation) = self
                .active
                .as_mut()
                .and_then(|active| active.operations.get_mut(&token))
        {
            operation.started = true;
            return true;
        }
        false
    }

    fn finish_operation(&mut self, generation: u64, token: u64, success: bool) -> bool {
        if generation != self.generation {
            return false;
        }
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        if active.operations.remove(&token).is_none() {
            return false;
        }
        if success {
            active.completed_count = active.completed_count.saturating_add(1);
        } else {
            active.failed_count = active.failed_count.saturating_add(1);
        }
        self.try_finish_drain()
    }

    #[cfg(test)]
    fn local_disconnect(&mut self) {
        self.begin_drain(ReleaseReason::Local);
    }

    fn expire(&mut self, now: Duration) -> Option<ReleaseReason> {
        self.expire_with_ai(now, false)
    }

    fn expire_with_ai(&mut self, now: Duration, has_nonterminal_ai: bool) -> Option<ReleaseReason> {
        let reason = self.active.as_ref().and_then(|active| {
            if now.saturating_sub(active.last_ping) >= LIVENESS_TIMEOUT {
                if has_nonterminal_ai {
                    if now.saturating_sub(active.last_activity) >= IDLE_TIMEOUT {
                        Some(ReleaseReason::BackgroundExpired)
                    } else {
                        None
                    }
                } else {
                    Some(ReleaseReason::LivenessTimeout)
                }
            } else if !active.media_playing
                && !active
                    .streaming
                    .as_ref()
                    .is_some_and(|streaming| streaming.playing)
                && now.saturating_sub(active.last_activity) >= IDLE_TIMEOUT
            {
                Some(if has_nonterminal_ai {
                    ReleaseReason::BackgroundExpired
                } else {
                    ReleaseReason::IdleTimeout
                })
            } else {
                None
            }
        });
        if reason.is_none()
            && has_nonterminal_ai
            && let Some(active) = self.active.as_mut()
            && now.saturating_sub(active.last_ping) >= LIVENESS_TIMEOUT
            && matches!(active.presence, RemoteClientPresence::Foreground)
        {
            active.presence = RemoteClientPresence::Detached { since: now };
        }
        if let Some(reason) = reason {
            self.begin_drain(reason);
        }
        reason
    }

    fn begin_drain(&mut self, reason: ReleaseReason) -> bool {
        if self
            .lifecycle
            .transition(RemoteControlTransition::BeginDrain)
            .is_err()
        {
            return false;
        }
        self.drain_reason = Some(reason);
        self.app_drain_complete = false;
        if let Some(active) = self.active.as_mut() {
            active.cancel_streaming();
            for operation in active.operations.values() {
                operation.cancel.store(true, Ordering::Release);
            }
        }
        true
    }

    fn complete_app_drain(&mut self, generation: u64) -> bool {
        if generation != self.generation
            || self.lifecycle.phase != RemoteControlPhase::DrainingRemote
        {
            return false;
        }
        self.app_drain_complete = true;
        self.try_finish_drain()
    }

    fn try_finish_drain(&mut self) -> bool {
        if self.lifecycle.phase != RemoteControlPhase::DrainingRemote
            || !self.app_drain_complete
            || self.active.as_ref().is_some_and(|active| {
                !active.operations.is_empty() || !active.streaming_workers.is_empty()
            })
        {
            return false;
        }
        let reason = self.drain_reason.unwrap_or(ReleaseReason::Local);
        match self.release(reason) {
            Ok(()) => true,
            Err(error) => {
                crate::logger::log(format!(
                    "remote_ipc: lifecycle invariant violation: transition={:?} phase={:?}",
                    error.transition, error.phase
                ));
                false
            }
        }
    }

    fn release(&mut self, reason: ReleaseReason) -> Result<(), RemoteControlTransitionError> {
        // Final release is owned by the typed phase transition. `active` is session payload,
        // not a sentinel for whether control may return to the PC.
        self.lifecycle
            .transition(RemoteControlTransition::FinishDrain)?;

        if let Some(mut active) = self.active.take() {
            active.cancel_streaming();
            self.last_owner = Some(active.client_id);
        } else {
            crate::logger::log(
                "remote_ipc: lifecycle invariant violation: final drain has no active session payload"
                    .to_owned(),
            );
        }
        self.last_release_reason = Some(reason);
        self.drain_reason = None;
        self.app_drain_complete = false;
        self.generation = self.generation.wrapping_add(1);
        self.control_return_sequence = self.control_return_sequence.wrapping_add(1);
        Ok(())
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

    fn draining_owner_response(&self) -> SessionResponse {
        let reason = self.drain_reason.unwrap_or(ReleaseReason::Local);
        status_response(reason.status(), release_message(reason))
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
            phase: self.lifecycle.phase,
            generation: self.generation,
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
        session_id: None,
    }
}

fn new_session_id() -> String {
    Uuid::new_v4().simple().to_string()
}

fn session_closing_response() -> SessionResponse {
    status_response(
        SessionStatus::LocalInUse,
        "リモートセッションを安全に終了しています。完了後に再接続してください。",
    )
}

fn release_message(reason: ReleaseReason) -> &'static str {
    match reason {
        ReleaseReason::Local => "本体で切断されました。再接続してください。",
        ReleaseReason::AcquireBarrierTimeout => {
            "本体の処理を安全に停止できなかったため接続を中止しました。処理の終了後に再接続してください。"
        }
        ReleaseReason::LivenessTimeout => "接続の生存確認が途絶えました。再接続してください。",
        ReleaseReason::IdleTimeout => "放置時間を超えたため切断されました。再接続してください。",
        ReleaseReason::BackgroundExpired => {
            "バックグラウンド保持時間を超えたため切断されました。再接続してください。"
        }
        ReleaseReason::Superseded => "別の端末で使用中です。再接続してください。",
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
    pub(crate) phase: RemoteControlPhase,
    pub(crate) generation: u64,
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
    pub(crate) jump_catalog: Arc<super::video_jump::VideoJumpCatalogSource>,
}

impl PublishedVideoStream {
    pub(crate) fn generation_id(&self) -> StreamingGeneration {
        self.generation.generation()
    }
}

#[derive(Clone)]
pub(crate) enum VideoStreamUiRequest {
    Start {
        owner: RemoteSessionIdentity,
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
        owner: RemoteSessionIdentity,
        path: PathBuf,
        quality: VideoStreamQuality,
    ) -> Self {
        Self::Start {
            owner,
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

/// App が所有し、remote worker へ許可した AI 資源だけを公開する bridge。
#[derive(Clone)]
pub(crate) struct RemoteAiExecutionBridge {
    runtime: Arc<(Mutex<RemoteAiRuntimeState>, Condvar)>,
    manager: Arc<crate::ai::model_manager::ModelManager>,
    background_mode: Arc<AtomicU8>,
}

enum RemoteAiRuntimeState {
    Empty,
    Initializing,
    Ready(Arc<crate::ai::runtime::AiRuntime>),
}

pub(crate) enum RemoteLocalRuntimeClaim {
    Initialize,
    WaitingForRemote,
    Ready(Arc<crate::ai::runtime::AiRuntime>),
}

pub(crate) struct RemoteAiResources {
    pub(crate) runtime: Arc<crate::ai::runtime::AiRuntime>,
    pub(crate) manager: Arc<crate::ai::model_manager::ModelManager>,
    pub(crate) background_mode: u8,
}

impl RemoteAiExecutionBridge {
    pub(crate) fn new(
        runtime: Option<Arc<crate::ai::runtime::AiRuntime>>,
        manager: Arc<crate::ai::model_manager::ModelManager>,
        background_mode: u8,
    ) -> Self {
        let state = runtime.map_or(RemoteAiRuntimeState::Empty, RemoteAiRuntimeState::Ready);
        Self {
            runtime: Arc::new((Mutex::new(state), Condvar::new())),
            manager,
            background_mode: Arc::new(AtomicU8::new(background_mode.min(2))),
        }
    }

    pub(crate) fn set_background_mode(&self, mode: u8) {
        self.background_mode.store(mode.min(2), Ordering::Release);
    }

    pub(crate) fn ready_runtime(&self) -> Option<Arc<crate::ai::runtime::AiRuntime>> {
        let (state, _) = &*self.runtime;
        match &*state.lock().unwrap_or_else(|error| error.into_inner()) {
            RemoteAiRuntimeState::Ready(runtime) => Some(Arc::clone(runtime)),
            RemoteAiRuntimeState::Empty | RemoteAiRuntimeState::Initializing => None,
        }
    }

    /// Runtime constructor を実行してよい owner を一つに限定する。
    ///
    /// remote worker が初期化中なら App は UI thread で待たず、後続 frame の
    /// ready_runtime poll に委ねる。逆に App が claim 済みなら remote worker は
    /// Condvar で待つため、起動直後の acquire と初回 App update が競合しても
    /// Runtime を二重生成しない。
    pub(crate) fn claim_local_runtime_init(&self) -> RemoteLocalRuntimeClaim {
        let (state, wake) = &*self.runtime;
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        match &*state {
            RemoteAiRuntimeState::Ready(existing) => {
                RemoteLocalRuntimeClaim::Ready(Arc::clone(existing))
            }
            RemoteAiRuntimeState::Initializing => RemoteLocalRuntimeClaim::WaitingForRemote,
            RemoteAiRuntimeState::Empty => {
                *state = RemoteAiRuntimeState::Initializing;
                // No waiter exists yet, but keep all state changes paired with the same
                // Condvar owner.
                wake.notify_all();
                RemoteLocalRuntimeClaim::Initialize
            }
        }
    }

    pub(crate) fn complete_claimed_runtime_init(
        &self,
        created: Result<crate::ai::runtime::AiRuntime, crate::ai::AiError>,
    ) -> Result<Arc<crate::ai::runtime::AiRuntime>, crate::ai::AiError> {
        let (runtime_state, wake) = &*self.runtime;
        let mut state = runtime_state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match created {
            Ok(runtime) => {
                let runtime = Arc::new(runtime);
                debug_assert!(matches!(*state, RemoteAiRuntimeState::Initializing));
                *state = RemoteAiRuntimeState::Ready(Arc::clone(&runtime));
                wake.notify_all();
                Ok(runtime)
            }
            Err(error) => {
                if matches!(*state, RemoteAiRuntimeState::Initializing) {
                    *state = RemoteAiRuntimeState::Empty;
                }
                wake.notify_all();
                Err(error)
            }
        }
    }

    /// shared runtime が未生成なら、この呼び出し元 remote worker 上で生成する。
    pub(crate) fn resources_for_remote(&self) -> Option<RemoteAiResources> {
        self.resources_for_remote_inner()
    }
}

impl RemoteAiExecutionBridge {
    fn resources_for_remote_inner(&self) -> Option<RemoteAiResources> {
        let (runtime_state, wake) = &*self.runtime;
        let mut state = runtime_state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            match &*state {
                RemoteAiRuntimeState::Ready(runtime) => {
                    return Some(RemoteAiResources {
                        runtime: Arc::clone(runtime),
                        manager: Arc::clone(&self.manager),
                        background_mode: self.background_mode.load(Ordering::Acquire),
                    });
                }
                RemoteAiRuntimeState::Initializing => {
                    state = wake.wait(state).unwrap_or_else(|error| error.into_inner());
                }
                RemoteAiRuntimeState::Empty => break,
            }
        }
        *state = RemoteAiRuntimeState::Initializing;
        drop(state);
        let created =
            crate::ai::runtime::AiRuntime::new_with_backend(crate::ai::AiBackend::DirectMl);
        match self.complete_claimed_runtime_init(created) {
            Ok(runtime) => Some(RemoteAiResources {
                runtime,
                manager: Arc::clone(&self.manager),
                background_mode: self.background_mode.load(Ordering::Acquire),
            }),
            Err(error) => {
                crate::logger::log(format!(
                    "remote_ipc: shared AI runtime init failed; using diffusion fallback: {error}"
                ));
                None
            }
        }
    }
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
    phase_wake: Arc<(Mutex<u64>, Condvar)>,
    ai_bridge: Arc<Mutex<Option<RemoteAiExecutionBridge>>>,
    ai_jobs: Arc<Mutex<Weak<super::ai_job::RemoteAiJobRegistry>>>,
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
        let outcome = match outcome {
            VideoStreamUiOutcome::Controlled(_) => {
                VideoStreamUiOutcome::Controlled(self.operation.ownership_response())
            }
            outcome => outcome,
        };
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
            phase_wake: Arc::new((Mutex::new(0), Condvar::new())),
            ai_bridge: Arc::new(Mutex::new(None)),
            ai_jobs: Arc::new(Mutex::new(Weak::new())),
        }
    }

    pub(crate) fn install_ai_jobs(&self, jobs: &Arc<super::ai_job::RemoteAiJobRegistry>) {
        *self
            .ai_jobs
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Arc::downgrade(jobs);
    }

    pub(crate) fn ai_job_registry(&self) -> Option<Arc<super::ai_job::RemoteAiJobRegistry>> {
        self.ai_jobs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .upgrade()
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
        let (response, phase_changed, generation_changed, drain_reason) = {
            let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            let phase = state.lifecycle.phase;
            let generation = state.generation;
            let response = state.acquire(self.now(), connected_unix_ms, request);
            let next_phase = state.lifecycle.phase;
            (
                response,
                next_phase != phase,
                state.generation != generation,
                (next_phase != phase && next_phase == RemoteControlPhase::DrainingRemote)
                    .then_some(state.drain_reason)
                    .flatten(),
            )
        };
        if phase_changed || generation_changed {
            self.clear_video_stream(None);
            self.notify_phase_changed();
        }
        if let Some(reason) = drain_reason {
            self.notify_ai_drain(reason);
        }
        if response.status == SessionStatus::Active {
            crate::logger::log(format!(
                "remote_ipc: session_acquired connection_kind={peer_kind:?} peer={peer_name}"
            ));
        } else if phase_changed {
            crate::logger::log(format!(
                "remote_ipc: session_acquire_deferred status={:?} connection_kind={peer_kind:?} peer={peer_name}",
                response.status
            ));
        }
        self.notify_ui();
        response
    }

    pub(crate) fn ping(&self, request: &SessionPingRequest) -> SessionResponse {
        let has_nonterminal_ai = self
            .ai_job_registry()
            .is_some_and(|jobs| jobs.has_nonterminal_jobs());
        let (response, phase_changed) = {
            let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            if has_nonterminal_ai {
                state.note_ai_client_seen(self.now(), &request.owner);
            }
            let phase = state.lifecycle.phase;
            let response = state.ping(self.now(), request);
            (response, state.lifecycle.phase != phase)
        };
        if phase_changed {
            self.clear_video_stream(None);
            self.notify_phase_changed();
            self.notify_ui();
        }
        response
    }

    pub(crate) fn begin_operation(
        &self,
        owner: &RemoteSessionIdentity,
        description: String,
    ) -> Result<SessionOperation, SessionResponse> {
        let has_nonterminal_ai = self
            .ai_job_registry()
            .is_some_and(|jobs| jobs.has_nonterminal_jobs());
        let (result, phase_changed) = {
            let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            if has_nonterminal_ai {
                state.note_ai_client_seen(self.now(), owner);
            }
            let phase = state.lifecycle.phase;
            let result = state.begin_operation(self.now(), owner, description);
            (result, state.lifecycle.phase != phase)
        };
        if phase_changed {
            self.clear_video_stream(None);
            self.notify_phase_changed();
            self.notify_ui();
        }
        let (generation, token, cancel) = result?;
        Ok(SessionOperation {
            handle: self.clone(),
            generation,
            token,
            owner: owner.clone(),
            cancel,
            finished: false,
        })
    }

    pub(crate) fn finish_acquire(&self, generation: u64) -> bool {
        let changed = self
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .finish_acquire(generation);
        if changed {
            self.notify_phase_changed();
            self.notify_ui();
        }
        changed
    }

    pub(crate) fn abort_acquire_barrier(&self, generation: u64) -> bool {
        let changed = self
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .abort_acquire_barrier(generation);
        if changed {
            self.notify_ai_drain(ReleaseReason::AcquireBarrierTimeout);
            self.clear_video_stream(None);
            self.notify_phase_changed();
            crate::logger::log(
                "remote_ipc: session_drain_started reason=acquire_barrier_timeout".to_owned(),
            );
            self.notify_ui();
        }
        changed
    }

    pub(crate) fn complete_app_drain(&self, generation: u64) -> bool {
        let released = self
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .complete_app_drain(generation);
        if released {
            self.clear_video_stream(None);
            self.notify_phase_changed();
            self.notify_ui();
        }
        released
    }

    /// 長寿命の streaming session を現在の remote owner に結び付ける token を返す。
    /// IPC の start 要求は通常の client_id 検証後、この token だけを UI へ渡す。
    pub(crate) fn streaming_owner(
        &self,
        owner: &RemoteSessionIdentity,
    ) -> Result<RemoteSessionOwner, SessionResponse> {
        let generation = self
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .streaming_owner(self.now(), owner)?;
        Ok(RemoteSessionOwner {
            handle: self.clone(),
            generation,
            client_id: owner.client_id.clone(),
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

    #[cfg(test)]
    pub(crate) fn owner_for_test(&self, client_id: &str) -> RemoteSessionIdentity {
        let state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let active = state
            .active
            .as_ref()
            .filter(|active| active.client_id == client_id)
            .expect("test client owns the active remote session");
        RemoteSessionIdentity {
            client_id: active.client_id.clone(),
            session_id: active.session_id.clone(),
        }
    }

    pub(crate) fn install_ai_bridge(&self, bridge: RemoteAiExecutionBridge) {
        *self
            .ai_bridge
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(bridge);
    }

    pub(crate) fn ai_bridge(&self) -> Option<RemoteAiExecutionBridge> {
        self.ai_bridge
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub(crate) fn remote_ai_resources(&self) -> Option<RemoteAiResources> {
        self.ai_bridge()?.resources_for_remote()
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
        let phase_changed = self
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .begin_drain(ReleaseReason::Local);
        if phase_changed {
            self.notify_ai_drain(ReleaseReason::Local);
            self.clear_video_stream(None); // drain starts before release
            self.notify_phase_changed();
            crate::logger::log("remote_ipc: session_drain_started reason=local".to_owned());
            self.notify_ui();
        }
    }

    pub(crate) fn note_ai_client_seen(&self, owner: &RemoteSessionIdentity) {
        let changed = self
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .note_ai_client_seen(self.now(), owner);
        if changed {
            self.notify_ui();
        }
    }

    fn notify_ai_drain(&self, reason: ReleaseReason) {
        let Some(jobs) = self.ai_job_registry() else {
            return;
        };
        let cause = match reason {
            ReleaseReason::Local | ReleaseReason::AcquireBarrierTimeout => {
                super::ai_job::RemoteAiDrainCause::DiscardedByHost
            }
            ReleaseReason::BackgroundExpired => {
                super::ai_job::RemoteAiDrainCause::BackgroundExpired
            }
            ReleaseReason::Superseded => super::ai_job::RemoteAiDrainCause::Superseded,
            ReleaseReason::LivenessTimeout | ReleaseReason::IdleTimeout => return,
        };
        jobs.on_session_drain(cause);
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

    fn notify_phase_changed(&self) {
        let (epoch, wake) = &*self.phase_wake;
        let mut epoch = epoch.lock().unwrap_or_else(|error| error.into_inner());
        *epoch = epoch.wrapping_add(1);
        wake.notify_all();
    }

    fn expire(&self) -> Option<ReleaseReason> {
        let has_nonterminal_ai = self
            .ai_job_registry()
            .is_some_and(|jobs| jobs.has_nonterminal_jobs());
        let (reason, phase_changed) = {
            let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            let phase = state.lifecycle.phase;
            let reason = state.expire_with_ai(self.now(), has_nonterminal_ai);
            (reason, state.lifecycle.phase != phase)
        };
        if reason.is_some() {
            self.clear_video_stream(None);
        }
        if let Some(reason) = reason {
            self.notify_ai_drain(reason);
        }
        if phase_changed {
            self.notify_phase_changed();
            self.notify_ui();
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
            && state.lifecycle.phase.accepts_remote_work()
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
            worker_lease: Some(RemoteStreamingWorkerLease {
                activity: RemoteStreamingActivity {
                    handle: self.handle.clone(),
                    generation: self.generation,
                    registration_id,
                },
            }),
        })
    }
}

pub(crate) struct RemoteStreamingRegistration {
    activity: RemoteStreamingActivity,
    cancel: Arc<AtomicBool>,
    worker_lease: Option<RemoteStreamingWorkerLease>,
}

impl RemoteStreamingRegistration {
    pub(crate) fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    pub(crate) fn activity(&self) -> RemoteStreamingActivity {
        self.activity.clone()
    }

    pub(crate) fn take_worker_lease(&mut self) -> RemoteStreamingWorkerLease {
        self.worker_lease
            .take()
            .expect("streaming registration worker lease was already taken")
    }
}

impl Drop for RemoteStreamingRegistration {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        let released = self
            .activity
            .handle
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .unregister_streaming(self.activity.generation, self.activity.registration_id);
        if released {
            self.activity.handle.clear_video_stream(None);
            self.activity.handle.notify_phase_changed();
            self.activity.handle.notify_ui();
        }
    }
}

pub(crate) struct RemoteStreamingWorkerLease {
    activity: RemoteStreamingActivity,
}

impl Drop for RemoteStreamingWorkerLease {
    fn drop(&mut self) {
        let released = self
            .activity
            .handle
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .finish_streaming_worker(self.activity.generation, self.activity.registration_id);
        if released {
            self.activity.handle.clear_video_stream(None);
            self.activity.handle.notify_phase_changed();
            self.activity.handle.notify_ui();
        }
    }
}

#[derive(Clone)]
pub(crate) struct RemoteStreamingActivity {
    handle: SessionHandle,
    generation: u64,
    registration_id: u64,
}

impl RemoteStreamingActivity {
    pub(crate) fn set_playing(&self, playing: bool) -> Result<(), RemoteStreamingControlError> {
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
    owner: RemoteSessionIdentity,
    cancel: Arc<AtomicBool>,
    finished: bool,
}

impl SessionOperation {
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn wait_until_active(&self) -> Result<(), SessionResponse> {
        loop {
            let (epoch, wake) = &*self.handle.phase_wake;
            let epoch = epoch.lock().unwrap_or_else(|error| error.into_inner());
            let response = {
                let state = self
                    .handle
                    .inner
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if state.generation != self.generation
                    || !state.active.as_ref().is_some_and(|active| {
                        active.client_id == self.owner.client_id
                            && active.session_id == self.owner.session_id
                    })
                {
                    Some(Err(state.inactive_response(&self.owner.client_id)))
                } else {
                    match state.lifecycle.phase {
                        RemoteControlPhase::RemoteActive => Some(Ok(())),
                        RemoteControlPhase::DrainingRemote | RemoteControlPhase::Local => {
                            Some(Err(state.draining_owner_response()))
                        }
                        RemoteControlPhase::AcquiringRemote => None,
                    }
                }
            };
            if let Some(response) = response {
                return response;
            }
            drop(wake.wait(epoch).unwrap_or_else(|error| error.into_inner()));
        }
    }

    /// Wait for the acquisition barrier without outliving an operation-level budget.
    /// `Ok(false)` means ownership is still valid but the barrier did not open in time.
    pub(crate) fn wait_until_active_for(&self, timeout: Duration) -> Result<bool, SessionResponse> {
        let deadline = Instant::now() + timeout;
        loop {
            let (epoch, wake) = &*self.handle.phase_wake;
            let epoch = epoch.lock().unwrap_or_else(|error| error.into_inner());
            let response = {
                let state = self
                    .handle
                    .inner
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if state.generation != self.generation
                    || !state.active.as_ref().is_some_and(|active| {
                        active.client_id == self.owner.client_id
                            && active.session_id == self.owner.session_id
                    })
                {
                    Some(Err(state.inactive_response(&self.owner.client_id)))
                } else {
                    match state.lifecycle.phase {
                        RemoteControlPhase::RemoteActive => Some(Ok(true)),
                        RemoteControlPhase::DrainingRemote | RemoteControlPhase::Local => {
                            Some(Err(state.draining_owner_response()))
                        }
                        RemoteControlPhase::AcquiringRemote => None,
                    }
                }
            };
            if let Some(response) = response {
                return response;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            let (epoch, _) = wake
                .wait_timeout(epoch, remaining)
                .unwrap_or_else(|error| error.into_inner());
            drop(epoch);
        }
    }

    pub(crate) fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    pub(crate) fn started(&self) {
        self.handle
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .start_operation(self.generation, self.token);
    }

    pub(crate) fn finish(mut self, success: bool) {
        self.finish_inner(success);
        self.finished = true;
    }

    fn finish_inner(&self, success: bool) {
        let released = self
            .handle
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .finish_operation(self.generation, self.token, success);
        if released {
            self.handle.clear_video_stream(None);
            self.handle.notify_phase_changed();
            self.handle.notify_ui();
        }
    }

    pub(crate) fn ownership_response(&self) -> SessionResponse {
        let state = self
            .handle
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match state.active.as_ref() {
            Some(active)
                if state.generation == self.generation
                    && active.client_id == self.owner.client_id
                    && active.session_id == self.owner.session_id
                    && state.lifecycle.phase != RemoteControlPhase::DrainingRemote =>
            {
                SessionResponse::active(active.session_id.clone())
            }
            Some(active)
                if state.generation == self.generation
                    && active.client_id == self.owner.client_id
                    && active.session_id == self.owner.session_id =>
            {
                state.draining_owner_response()
            }
            Some(_) => status_response(
                SessionStatus::Superseded,
                "別のリモート接続が操作権を取得しました。",
            ),
            None => state.inactive_response(&self.owner.client_id),
        }
    }
}

impl Drop for SessionOperation {
    fn drop(&mut self) {
        if !self.finished {
            self.finish_inner(false);
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
                "remote_ipc: session_drain_started reason={}",
                match reason {
                    ReleaseReason::LivenessTimeout => "liveness_timeout",
                    ReleaseReason::IdleTimeout => "idle_timeout",
                    ReleaseReason::BackgroundExpired => "background_expired",
                    ReleaseReason::Local => "local",
                    ReleaseReason::AcquireBarrierTimeout => "acquire_barrier_timeout",
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
                address: page.clone(),
                context_address: container.clone(),
                page_index: 3,
                bookmark_supported: true,
            },
            RemoteWriteRequest::ListBookBookmarks {
                address: page.clone(),
                context_address: container.clone(),
                page_index: 3,
                bookmark_supported: true,
            },
            RemoteWriteRequest::SetBookBookmarkTitle {
                address: page.clone(),
                context_address: container.clone(),
                page_index: 3,
                id: 17,
                title: "要確認".to_owned(),
            },
            RemoteWriteRequest::RemoveBookBookmark {
                address: page.clone(),
                context_address: container.clone(),
                page_index: 3,
                id: 17,
            },
            RemoteWriteRequest::SetAdjustment {
                address: page.clone(),
                scope: mimageviewer_ipc::RemoteAdjustmentScope::Page,
                values: mimageviewer_ipc::RemoteAdjustmentValues {
                    brightness: 12.0,
                    contrast: -4.0,
                    gamma: 1.1,
                    saturation: 5.0,
                    temperature: 2.0,
                    black_point: 3,
                    white_point: 250,
                    midtone: 0.9,
                    auto_mode: None,
                    colorize: mimageviewer_ipc::RemoteColorizeParams::default(),
                    ai: None,
                },
            },
            RemoteWriteRequest::GetAdjustmentState {
                address: page.clone(),
            },
            RemoteWriteRequest::SetViewTrim {
                address: page.clone(),
                context_address: container.clone(),
                state: serde_json::json!({}),
            },
            RemoteWriteRequest::GetViewTrimState {
                address: page,
                context_address: container.clone(),
            },
            RemoteWriteRequest::SetSortOrder {
                scope: mimageviewer_ipc::RemoteGridScope::Address { address: container },
                sort_order: "FileName".to_owned(),
            },
        ]
    }

    fn peer() -> SessionPeerInfo {
        SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: Some("phone".to_owned()),
        }
    }

    fn state_owner(state: &SessionStateMachine, client_id: &str) -> RemoteSessionIdentity {
        let active = state
            .active
            .as_ref()
            .filter(|active| active.client_id == client_id)
            .expect("test client owns the active session");
        RemoteSessionIdentity {
            client_id: active.client_id.clone(),
            session_id: active.session_id.clone(),
        }
    }

    fn missing_owner(client_id: &str) -> RemoteSessionIdentity {
        RemoteSessionIdentity {
            client_id: client_id.to_owned(),
            session_id: "0123456789abcdef0123456789abcdef".to_owned(),
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
        assert!(state.finish_acquire(state.generation));
    }

    #[test]
    fn remote_control_lifecycle_has_one_valid_linear_path() {
        let mut lifecycle = RemoteControlLifecycle::default();
        assert_eq!(lifecycle.phase, RemoteControlPhase::Local);
        assert!(!lifecycle.phase.blocks_local_control());
        assert!(
            lifecycle
                .transition(RemoteControlTransition::BeginAcquire)
                .is_ok()
        );
        assert_eq!(lifecycle.phase, RemoteControlPhase::AcquiringRemote);
        assert!(lifecycle.phase.blocks_local_control());
        assert!(!lifecycle.phase.accepts_remote_work());
        assert!(
            lifecycle
                .transition(RemoteControlTransition::FinishAcquire)
                .is_ok()
        );
        assert_eq!(lifecycle.phase, RemoteControlPhase::RemoteActive);
        assert!(lifecycle.phase.accepts_remote_work());
        assert!(
            lifecycle
                .transition(RemoteControlTransition::BeginDrain)
                .is_ok()
        );
        assert_eq!(lifecycle.phase, RemoteControlPhase::DrainingRemote);
        assert!(lifecycle.phase.blocks_local_control());
        assert!(!lifecycle.phase.accepts_remote_work());
        assert!(
            lifecycle
                .transition(RemoteControlTransition::FinishDrain)
                .is_ok()
        );
        assert_eq!(lifecycle.phase, RemoteControlPhase::Local);
    }

    #[test]
    fn remote_control_lifecycle_rejects_skipped_and_duplicate_transitions() {
        let mut lifecycle = RemoteControlLifecycle::default();
        assert!(
            lifecycle
                .transition(RemoteControlTransition::FinishAcquire)
                .is_err()
        );
        assert!(
            lifecycle
                .transition(RemoteControlTransition::BeginDrain)
                .is_err()
        );
        assert!(
            lifecycle
                .transition(RemoteControlTransition::FinishDrain)
                .is_err()
        );
        assert!(
            lifecycle
                .transition(RemoteControlTransition::BeginAcquire)
                .is_ok()
        );
        assert!(
            lifecycle
                .transition(RemoteControlTransition::BeginAcquire)
                .is_err()
        );
        assert!(
            lifecycle
                .transition(RemoteControlTransition::FinishDrain)
                .is_err()
        );
        assert!(
            lifecycle
                .transition(RemoteControlTransition::BeginDrain)
                .is_ok()
        );
        assert!(
            lifecycle
                .transition(RemoteControlTransition::FinishAcquire)
                .is_err()
        );
        assert!(
            lifecycle
                .transition(RemoteControlTransition::BeginDrain)
                .is_err()
        );
        assert!(
            lifecycle
                .transition(RemoteControlTransition::FinishDrain)
                .is_ok()
        );
    }

    #[test]
    fn nonterminal_ai_detaches_on_liveness_and_same_client_recovers_it() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);

        assert_eq!(state.expire_with_ai(LIVENESS_TIMEOUT, true), None);
        assert_eq!(state.lifecycle.phase, RemoteControlPhase::RemoteActive);
        assert!(matches!(
            state.active.as_ref().map(|active| active.presence),
            Some(RemoteClientPresence::Detached { .. })
        ));

        let owner = state_owner(&state, "client");
        assert!(state.note_ai_client_seen(LIVENESS_TIMEOUT + Duration::from_secs(1), &owner));
        assert!(matches!(
            state.active.as_ref().map(|active| active.presence),
            Some(RemoteClientPresence::Foreground)
        ));
        assert_eq!(state.lifecycle.phase, RemoteControlPhase::RemoteActive);
    }

    #[test]
    fn nonterminal_ai_background_expiry_enters_typed_drain() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);

        assert_eq!(
            state.expire_with_ai(IDLE_TIMEOUT, true),
            Some(ReleaseReason::BackgroundExpired)
        );
        assert_eq!(state.lifecycle.phase, RemoteControlPhase::DrainingRemote);
        assert_eq!(state.drain_reason, Some(ReleaseReason::BackgroundExpired));
    }

    #[test]
    fn final_drain_without_active_payload_returns_local_exactly_once() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);
        state.local_disconnect();
        let draining_generation = state.generation;
        let control_before = state.control_return_sequence;

        // This reproduces the broken intermediate state seen in dev-runtime: the session
        // payload was already taken while the typed lifecycle still said DrainingRemote.
        let _detached_payload = state.active.take().expect("active session payload");

        assert!(state.complete_app_drain(draining_generation));
        assert_eq!(state.lifecycle.phase, RemoteControlPhase::Local);
        assert!(!state.lifecycle.phase.blocks_local_control());
        assert_eq!(
            state.control_return_sequence,
            control_before.wrapping_add(1)
        );

        let released_generation = state.generation;
        assert!(!state.complete_app_drain(released_generation));
        assert_eq!(state.lifecycle.phase, RemoteControlPhase::Local);
        assert_eq!(
            state.control_return_sequence,
            control_before.wrapping_add(1),
            "a completed drain must not publish control return twice"
        );
    }

    #[test]
    fn shared_runtime_initialization_has_exactly_one_claimant() {
        let bridge = RemoteAiExecutionBridge::new(
            None,
            Arc::new(crate::ai::model_manager::ModelManager::new()),
            0,
        );
        assert!(matches!(
            bridge.claim_local_runtime_init(),
            RemoteLocalRuntimeClaim::Initialize
        ));
        assert!(matches!(
            bridge.claim_local_runtime_init(),
            RemoteLocalRuntimeClaim::WaitingForRemote
        ));
        assert!(bridge.ready_runtime().is_none());
    }

    #[test]
    fn queued_operation_waits_for_acquire_barrier_and_wakes_on_activation() {
        let handle = SessionHandle::new();
        handle.acquire(SessionAcquireRequest {
            client_id: "client".to_owned(),
            peer: peer(),
        });
        let generation = handle.snapshot().generation;
        let owner = handle.owner_for_test("client");
        let operation = handle.begin_operation(&owner, "queued".to_owned()).unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = operation.wait_until_active();
            let _ = tx.send(result.as_ref().map(|_| ()).map_err(|value| value.status));
            operation.finish(result.is_ok());
        });

        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(handle.finish_acquire(generation));
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), Ok(()));
        worker.join().unwrap();
    }

    #[test]
    fn bounded_operation_wait_ends_while_acquire_barrier_remains_closed() {
        let handle = SessionHandle::new();
        handle.acquire(SessionAcquireRequest {
            client_id: "client".to_owned(),
            peer: peer(),
        });
        let owner = handle.owner_for_test("client");
        let operation = handle
            .begin_operation(&owner, "queued start".to_owned())
            .unwrap();

        assert!(
            !operation
                .wait_until_active_for(Duration::from_millis(10))
                .unwrap()
        );
        assert_eq!(handle.snapshot().phase, RemoteControlPhase::AcquiringRemote);
        operation.finish(false);
    }

    #[test]
    fn acquire_barrier_timeout_drains_instead_of_forcing_remote_active() {
        let handle = SessionHandle::new();
        handle.acquire(SessionAcquireRequest {
            client_id: "client".to_owned(),
            peer: peer(),
        });
        let generation = handle.snapshot().generation;
        let owner = handle.owner_for_test("client");

        assert!(handle.abort_acquire_barrier(generation));
        assert_eq!(handle.snapshot().phase, RemoteControlPhase::DrainingRemote);
        assert!(!handle.finish_acquire(generation));
        assert!(handle.complete_app_drain(generation));
        assert_eq!(handle.snapshot().phase, RemoteControlPhase::Local);

        let response = handle.ping(&SessionPingRequest {
            owner,
            user_active: false,
            media_playing: false,
        });
        assert_eq!(response.status, SessionStatus::LocalInUse);
        assert!(response.message.contains("安全に停止できなかった"));
    }

    #[test]
    fn liveness_timeout_releases_session() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);
        let owner = state_owner(&state, "client");
        assert_eq!(
            state
                .ping(
                    LIVENESS_TIMEOUT,
                    &SessionPingRequest {
                        owner,
                        user_active: false,
                        media_playing: false,
                    },
                )
                .status,
            SessionStatus::Expired
        );
        assert_eq!(state.lifecycle.phase, RemoteControlPhase::DrainingRemote);
        assert_eq!(state.drain_reason, Some(ReleaseReason::LivenessTimeout));
        assert!(state.active.is_some());
        assert!(state.complete_app_drain(state.generation));
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
                    owner: state_owner(&state, "client"),
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
                owner: state_owner(&state, "client"),
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
        let owner = state_owner(state, "client");
        let generation = state.streaming_owner(now, &owner).unwrap();
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
        let (generation, registration, cancel) = register_test_stream(&mut state, Duration::ZERO);

        state.local_disconnect();

        assert!(cancel.load(Ordering::Acquire));
        assert_eq!(state.lifecycle.phase, RemoteControlPhase::DrainingRemote);
        assert!(state.active.is_some());
        assert!(!state.complete_app_drain(state.generation));
        assert_eq!(state.lifecycle.phase, RemoteControlPhase::DrainingRemote);
        assert!(!state.unregister_streaming(generation, registration));
        assert!(state.finish_streaming_worker(generation, registration));
        assert!(state.active.is_none());
        assert_eq!(state.last_release_reason, Some(ReleaseReason::Local));
    }

    #[test]
    fn next_owner_cannot_start_streaming_before_previous_worker_unregisters() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);
        let old_owner = state_owner(&state, "client");
        let (old_generation, old_registration, old_cancel) =
            register_test_stream(&mut state, Duration::ZERO);

        state.local_disconnect();
        assert!(old_cancel.load(Ordering::Acquire));
        assert!(!state.complete_app_drain(old_generation));
        assert_eq!(
            state
                .acquire(
                    Duration::from_secs(1),
                    2,
                    SessionAcquireRequest {
                        client_id: "next-client".to_owned(),
                        peer: peer(),
                    },
                )
                .status,
            SessionStatus::LocalInUse
        );
        assert_eq!(state.drain_reason, Some(ReleaseReason::Local));
        let old_owner_status = state.ping(
            Duration::from_secs(1),
            &SessionPingRequest {
                owner: old_owner,
                user_active: false,
                media_playing: false,
            },
        );
        assert_eq!(old_owner_status.status, SessionStatus::LocalInUse);
        assert!(old_owner_status.message.contains("本体で切断"));

        assert!(!state.unregister_streaming(old_generation, old_registration));
        assert!(state.finish_streaming_worker(old_generation, old_registration));
        assert_eq!(state.lifecycle.phase, RemoteControlPhase::Local);
        assert_eq!(
            state
                .acquire(
                    Duration::from_secs(2),
                    3,
                    SessionAcquireRequest {
                        client_id: "next-client".to_owned(),
                        peer: peer(),
                    },
                )
                .status,
            SessionStatus::Active
        );
        assert_eq!(state.lifecycle.phase, RemoteControlPhase::AcquiringRemote);
        assert!(state.finish_acquire(state.generation));
        let owner = state_owner(&state, "next-client");
        let new_generation = state
            .streaming_owner(Duration::from_secs(2), &owner)
            .unwrap();
        let new_cancel = Arc::new(AtomicBool::new(false));
        state
            .register_streaming(
                Duration::from_secs(2),
                new_generation,
                "next-client",
                Arc::clone(&new_cancel),
            )
            .unwrap();
        assert!(!new_cancel.load(Ordering::Acquire));
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
        assert_eq!(
            state.set_streaming_playing(generation, registration, false),
            Ok(())
        );

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
    fn completed_worker_keeps_the_stream_control_registration() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);
        let (generation, registration, _) = register_test_stream(&mut state, Duration::ZERO);

        assert!(!state.finish_streaming_worker(generation, registration));
        assert_eq!(
            state.set_streaming_playing(generation, registration, false),
            Ok(())
        );
        assert!(state.note_segment_fetch(Duration::from_secs(1), generation, registration));
        assert!(state.snapshot().active.unwrap().streaming);
    }

    #[test]
    fn stream_control_failures_identify_the_broken_ownership_boundary() {
        let mut state = SessionStateMachine::default();
        assert_eq!(
            state.set_streaming_playing(0, 1, false),
            Err(RemoteStreamingControlError::SessionMissing)
        );

        acquire(&mut state, Duration::ZERO);
        let (generation, registration, _) = register_test_stream(&mut state, Duration::ZERO);
        assert_eq!(
            state.set_streaming_playing(generation.wrapping_add(1), registration, false),
            Err(RemoteStreamingControlError::SessionGenerationMismatch)
        );
        assert_eq!(
            state.set_streaming_playing(generation, registration.wrapping_add(1), false),
            Err(RemoteStreamingControlError::RegistrationMismatch)
        );
        assert!(!state.unregister_streaming(generation, registration));
        assert_eq!(
            state.set_streaming_playing(generation, registration, false),
            Err(RemoteStreamingControlError::RegistrationMissing)
        );
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
                            owner: state_owner(&state, "client"),
                            user_active: false,
                            media_playing: false,
                        },
                    )
                    .status,
                SessionStatus::Active
            );
        }
        let owner = state_owner(&state, "client");
        let (generation, token, _cancel) = state
            .begin_operation(now, &owner, "一覧を取得中".to_owned())
            .unwrap();
        state.finish_operation(generation, token, true);
        assert_eq!(state.expire(now + Duration::from_secs(2)), None);
    }

    #[test]
    fn remote_can_reacquire_after_local_disconnect() {
        let mut state = SessionStateMachine::default();
        acquire(&mut state, Duration::ZERO);
        state.local_disconnect();
        let owner = state_owner(&state, "client");
        assert_eq!(
            state
                .begin_operation(Duration::from_secs(1), &owner, "test".to_owned())
                .unwrap_err()
                .status,
            SessionStatus::LocalInUse
        );
        assert!(state.complete_app_drain(state.generation));
        acquire(&mut state, Duration::from_secs(2));
        let owner = state_owner(&state, "client");
        assert!(
            state
                .begin_operation(Duration::from_secs(3), &owner, "test".to_owned())
                .is_ok()
        );
    }

    #[test]
    fn write_without_session_is_rejected_before_ui_queue() {
        for request in write_requests() {
            let handle = SessionHandle::new();
            let owner = missing_owner("client");
            let response = match handle
                .begin_operation(&owner, format!("{} を適用中", request.kind_name()))
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
                let owner = worker_handle.owner_for_test("client");
                let operation = worker_handle
                    .begin_operation(&owner, format!("{kind} を適用中"))
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
                let owner = worker_handle.owner_for_test("client");
                let operation = worker_handle
                    .begin_operation(&owner, format!("{kind} を適用中"))
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
            let owner = worker_handle.owner_for_test("client");
            let operation = worker_handle
                .begin_operation(&owner, "見開き設定を書き込み中".to_owned())
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
        let owner = handle.owner_for_test("client");
        let operation = handle
            .begin_operation(&owner, "見開き設定を書き込み中".to_owned())
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
        let generation = handle.snapshot().generation;
        let owner = handle.owner_for_test("client");
        let operation = handle.begin_operation(&owner, "test".to_owned()).unwrap();
        let cancel = operation.cancel_flag();
        handle.local_disconnect();
        assert_eq!(handle.snapshot().phase, RemoteControlPhase::DrainingRemote);
        assert!(handle.snapshot().active.is_some());
        assert!(cancel.load(Ordering::Acquire));
        assert_eq!(
            operation.ownership_response().status,
            SessionStatus::LocalInUse
        );
        assert!(!handle.complete_app_drain(generation));
        operation.finish(false);
        let released = handle.snapshot();
        assert_eq!(released.phase, RemoteControlPhase::Local);
        assert!(released.active.is_none());
        assert_eq!(released.control_return_sequence, 1);
    }

    #[test]
    fn streaming_worker_lease_drop_completes_an_app_drained_session() {
        let handle = SessionHandle::new();
        handle.acquire(SessionAcquireRequest {
            client_id: "client".to_owned(),
            peer: peer(),
        });
        assert!(handle.finish_acquire(handle.snapshot().generation));
        let owner = handle.owner_for_test("client");
        let streaming_owner = handle.streaming_owner(&owner).unwrap();
        let mut registration = streaming_owner.register_streaming().unwrap();
        let worker_lease = registration.take_worker_lease();
        let generation = handle.snapshot().generation;

        handle.local_disconnect();
        assert!(!handle.complete_app_drain(generation));
        assert_eq!(handle.snapshot().phase, RemoteControlPhase::DrainingRemote);

        drop(registration);
        assert_eq!(
            handle.snapshot().phase,
            RemoteControlPhase::DrainingRemote,
            "dropping the control registration must not stand in for worker completion"
        );
        drop(worker_lease);
        let released = handle.snapshot();
        assert_eq!(released.phase, RemoteControlPhase::Local);
        assert!(released.active.is_none());
        assert_eq!(released.control_return_sequence, 1);
    }

    #[test]
    fn later_remote_client_waits_for_the_previous_owner_to_drain() {
        let handle = SessionHandle::new();
        handle.acquire(SessionAcquireRequest {
            client_id: "client-a".to_owned(),
            peer: peer(),
        });
        let generation = handle.snapshot().generation;
        let owner_a = handle.owner_for_test("client-a");
        let old_operation = handle.begin_operation(&owner_a, "old".to_owned()).unwrap();
        let takeover = handle.acquire(SessionAcquireRequest {
            client_id: "client-b".to_owned(),
            peer: peer(),
        });
        assert_eq!(takeover.status, SessionStatus::LocalInUse);
        assert_eq!(
            old_operation.ownership_response().status,
            SessionStatus::Superseded
        );
        assert!(
            handle
                .begin_operation(&missing_owner("client-b"), "new".to_owned())
                .is_err()
        );
        old_operation.finish(false);
        assert!(handle.complete_app_drain(generation));
        assert_eq!(
            handle
                .acquire(SessionAcquireRequest {
                    client_id: "client-b".to_owned(),
                    peer: peer(),
                })
                .status,
            SessionStatus::Active
        );
    }

    #[test]
    fn acquisition_sequence_changes_for_every_completed_acquisition() {
        let handle = SessionHandle::new();
        let request = |client_id: &str| SessionAcquireRequest {
            client_id: client_id.to_owned(),
            peer: peer(),
        };
        handle.acquire(request("client-a"));
        assert!(handle.finish_acquire(handle.snapshot().generation));
        let first = handle.snapshot().acquisition_sequence;
        let first_session_id = handle.owner_for_test("client-a").session_id;
        assert_eq!(
            handle.acquire(request("client-a")).status,
            SessionStatus::LocalInUse
        );
        let draining = handle.snapshot();
        assert_eq!(handle.snapshot().acquisition_sequence, first);
        assert!(handle.complete_app_drain(draining.generation));
        handle.acquire(request("client-a"));
        assert_eq!(handle.snapshot().acquisition_sequence, first + 1);
        assert_ne!(
            handle.owner_for_test("client-a").session_id,
            first_session_id,
            "every successful acquisition rotates the lease identity"
        );
    }

    #[test]
    fn reacquiring_the_same_device_invalidates_its_previous_session_id() {
        let handle = SessionHandle::new();
        let request = SessionAcquireRequest {
            client_id: "client".to_owned(),
            peer: peer(),
        };
        handle.acquire(request.clone());
        assert!(handle.finish_acquire(handle.snapshot().generation));
        let generation = handle.snapshot().generation;
        let previous = handle.owner_for_test("client");
        let operation = handle
            .begin_operation(&previous, "old request".to_owned())
            .unwrap();
        let cancel = operation.cancel_flag();

        assert_eq!(
            handle.acquire(request.clone()).status,
            SessionStatus::LocalInUse
        );
        assert!(cancel.load(Ordering::Acquire));
        operation.finish(false);
        assert!(handle.complete_app_drain(generation));

        let response = handle.acquire(request);
        let current = handle.owner_for_test("client");
        assert_eq!(
            response.session_id.as_deref(),
            Some(current.session_id.as_str())
        );
        assert_ne!(previous.session_id, current.session_id);
        let stale = match handle.begin_operation(&previous, "stale request".to_owned()) {
            Ok(_) => panic!("the previous session id remained valid"),
            Err(response) => response,
        };
        assert_eq!(stale.status, SessionStatus::Superseded);
        assert!(handle.finish_acquire(handle.snapshot().generation));
        assert!(
            handle
                .begin_operation(&current, "current request".to_owned())
                .is_ok()
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
