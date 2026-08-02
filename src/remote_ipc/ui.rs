use mimageviewer_ipc::{
    RemoteItemState, RemoteReadingDirection, RemoteSpreadMode, RemoteSubresource,
    RemoteWebFeatureStatus, RemoteWriteError, RemoteWriteErrorCode, RemoteWriteRequest,
    RemoteWriteResponse, RemoteWriteResult, SessionConnectionKind, SessionResponse, SessionStatus,
    VideoStreamControlAction, VideoStreamError, VideoStreamErrorCode,
};
use qrcode::{Color, QrCode};

use crate::fs_animation::FsCacheEntry;
use crate::video::stream::session::{
    RemoteVideoStreamingSession, StreamGenerationStatus, StreamReconcile, StreamingGeneration,
};

use super::path_guard::logical_favorite_path;
use super::session::{
    ActiveSessionSnapshot, ClaimedRemoteUiRequest, ClaimedRemoteWrite, ClaimedVideoStreamUiRequest,
    PublishedVideoStream, SessionHandle, UiWriteOutcome, VideoStreamPlaybackState,
    VideoStreamUiOutcome, VideoStreamUiRequest,
};
use super::video_stream::{VideoStreamStartBudget, VideoStreamStartStage};

#[derive(Default)]
pub(crate) struct RemoteSessionUiState {
    handle: Option<SessionHandle>,
    last_acquisition_sequence: u64,
    last_control_return_sequence: u64,
    pending_fullscreen_restore: Option<PendingFullscreenRestore>,
    paused_animation_restore_key: Option<String>,
    pending_bookmark_writes: std::collections::HashMap<u64, PendingRemoteBookmarkWrite>,
    show_connection_dialog: bool,
    video_stream: Option<AppRemoteVideoStreamState>,
}

enum AppRemoteVideoStreamState {
    Opening(AppRemoteVideoOpening),
    Starting(AppRemoteVideoStarting),
    Streaming(AppRemoteVideoStreaming),
}

struct AppRemoteVideoOpening {
    claimed: ClaimedVideoStreamUiRequest,
    client_id: String,
    requested_path: std::path::PathBuf,
    quality: crate::video::stream::quality::QualityPreset,
    fs_idx: usize,
    budget: VideoStreamStartBudget,
}

struct AppRemoteVideoStreaming {
    fs_idx: usize,
    requested_path: std::path::PathBuf,
    session: RemoteVideoStreamingSession,
    playback: std::sync::Arc<VideoStreamPlaybackState>,
}

struct AppRemoteVideoStarting {
    claimed: ClaimedVideoStreamUiRequest,
    streaming: AppRemoteVideoStreaming,
    budget: VideoStreamStartBudget,
    waiting_stage: VideoStreamStartStage,
}
// リモート配信の local surface は streaming UI state owner と同居させる。
pub(crate) fn draw_remote_video_local_surface(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    source_name: &str,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, crate::ui_music_timeline::MUSIC_VIEW_BG);
    let center = rect.center();
    let content_y = center.y - (rect.height() * 0.08).min(72.0);
    draw_remote_video_local_surface_status(&painter, center, content_y);
    draw_remote_video_local_surface_details(&painter, rect, content_y, source_name);
}

fn draw_remote_video_local_surface_status(
    painter: &egui::Painter,
    center: egui::Pos2,
    content_y: f32,
) {
    let badge = egui::Rect::from_center_size(
        egui::pos2(center.x, content_y - 64.0),
        egui::vec2(104.0, 28.0),
    );
    painter.rect_filled(
        badge,
        badge.height() * 0.5,
        egui::Color32::from_rgb(36, 82, 126),
    );
    painter.text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        "REMOTE",
        egui::FontId::proportional(13.0),
        egui::Color32::from_rgb(224, 240, 255),
    );
    painter.text(
        egui::pos2(center.x, content_y - 24.0),
        egui::Align2::CENTER_CENTER,
        "リモートへ配信中",
        egui::FontId::proportional(24.0),
        egui::Color32::from_gray(232),
    );
}

fn draw_remote_video_local_surface_details(
    painter: &egui::Painter,
    rect: egui::Rect,
    content_y: f32,
    source_name: &str,
) {
    let filename = painter.layout(
        source_name.to_owned(),
        egui::FontId::proportional(18.0),
        egui::Color32::from_rgb(126, 184, 238),
        (rect.width() - 64.0).clamp(1.0, 720.0),
    );
    painter.galley(
        egui::pos2(rect.center().x - filename.size().x * 0.5, content_y + 4.0),
        filename,
        egui::Color32::from_rgb(126, 184, 238),
    );
    painter.text(
        egui::pos2(rect.center().x, content_y + 54.0),
        egui::Align2::CENTER_CENTER,
        "この PC では映像を非表示にしています",
        egui::FontId::proportional(14.0),
        egui::Color32::from_gray(158),
    );
}

struct PendingRemoteBookmarkWrite {
    claimed: ClaimedRemoteWrite,
    kind: PendingRemoteBookmarkWriteKind,
}

enum PendingRemoteBookmarkWriteKind {
    ReadState { rating: u8 },
    SetPresence,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteVideoStartReadiness {
    Stable,
    WaitingForSeek,
    WaitingForEncoder,
}

impl RemoteVideoStartReadiness {
    fn wait_stage(self) -> VideoStreamStartStage {
        match self {
            Self::Stable | Self::WaitingForSeek => VideoStreamStartStage::Seek,
            Self::WaitingForEncoder => VideoStreamStartStage::Encoder,
        }
    }
}

fn remote_video_start_outcome_for_player(
    starting: &mut AppRemoteVideoStarting,
    player: &crate::video::VideoPlayer,
) -> Result<(StreamReconcile, RemoteVideoStartReadiness), String> {
    starting.streaming.playback.update(
        player.position_secs(),
        player.duration(),
        player.volume(),
        player.intent_playing(),
    );
    let reconcile = starting.streaming.session.reconcile(player);
    let seek_is_stable = matches!(reconcile, StreamReconcile::Active)
        && player.remote_stream_start_seek_is_settled()
        && starting
            .streaming
            .session
            .generation_matches_player_seek(player);
    let readiness = if !seek_is_stable {
        RemoteVideoStartReadiness::WaitingForSeek
    } else if matches!(
        starting.streaming.session.status(),
        StreamGenerationStatus::Ready(_)
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
        self.remote_session_ui.handle = Some(handle);
    }

    pub(crate) fn open_remote_connection_dialog(&mut self) {
        self.remote_session_ui.show_connection_dialog = true;
    }

    pub(crate) fn remote_connection_dialog_open(&self) -> bool {
        self.remote_session_ui.show_connection_dialog
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

    pub(crate) fn remote_session_active(&self) -> bool {
        self.remote_session_ui
            .handle
            .as_ref()
            .is_some_and(|handle| handle.snapshot().active.is_some())
    }

    pub(crate) fn poll_remote_session(&mut self, ctx: &egui::Context) {
        let handle = self.remote_session_ui.handle.clone();
        if let Some(handle) = handle.as_ref() {
            handle.install_repaint_context(ctx);
        }
        let snapshot = self
            .remote_session_ui
            .handle
            .as_ref()
            .map(SessionHandle::snapshot);
        let active = snapshot
            .as_ref()
            .is_some_and(|value| value.active.is_some());
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
        if let Some(snapshot) = snapshot.as_ref()
            && acquisition_changed
        {
            self.remote_session_ui.last_acquisition_sequence = snapshot.acquisition_sequence;
            let (media, slideshow, animations, continuous) =
                self.pause_local_progress_for_remote_session();
            crate::logger::log(format!(
                "remote_ipc: local playback paused on session acquire media={media} slideshow={slideshow} animations={animations} continuous_pending={continuous}"
            ));
        }
        if let Some(snapshot) = snapshot
            && control_returned
        {
            self.remote_session_ui.last_control_return_sequence = snapshot.control_return_sequence;
            self.reload_after_remote_session_release();
        }
        // Reconcile the previous frame's stream before draining new requests. A Start therefore
        // gets one complete UI-frame boundary after the normal open path creates its player;
        // metadata/tap readiness remains an asynchronous state transition instead of being
        // sampled in the same call stack that initiated the open.
        self.poll_remote_video_streaming();
        // Observe owner transitions before draining its requests. In particular, an acquire and
        // the first video start can both arrive before the next UI frame; cancelling after the
        // drain would incorrectly reject that new owner's freshly opened stream.
        if let Some(handle) = handle.as_ref() {
            self.apply_pending_remote_ui_requests(handle);
        }
        self.poll_remote_fullscreen_restore();
        if matches!(
            self.remote_session_ui.video_stream.as_ref(),
            Some(AppRemoteVideoStreamState::Opening(_) | AppRemoteVideoStreamState::Starting(_))
        ) {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        } else if active || self.remote_session_ui.pending_fullscreen_restore.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    fn create_remote_video_streaming(
        &mut self,
        client_id: &str,
        requested_path: &std::path::Path,
        quality: crate::video::stream::quality::QualityPreset,
        fs_idx: usize,
    ) -> Result<AppRemoteVideoStreaming, String> {
        if !self.settings.remote_video_streaming_enabled {
            return Err("remote video streaming is disabled".to_owned());
        }
        let owner = self
            .remote_session_ui
            .handle
            .as_ref()
            .ok_or_else(|| "remote session service is unavailable".to_owned())?
            .streaming_owner(client_id)
            .map_err(|response| response.message)?;
        let encoder = self.settings.remote_video_encoder.into();
        let segment_capacity = self.settings.remote_video_segment_window;
        let mute_local_output = self.settings.remote_video_mute_local_output;
        let hide_local_video_output = self.settings.remote_video_hide_local_output;
        let player = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player,
            _ => return Err("the fullscreen item is not playable media".to_owned()),
        };
        if !crate::folder_tree::path_eq(player.path(), requested_path) {
            return Err("the requested video is not the current fullscreen media".to_owned());
        }
        let previous_speed = player.playback_speed();
        let speed_changed = (previous_speed - 1.0).abs() > 1.0e-6;
        if speed_changed {
            player.set_playback_speed(1.0);
        }
        let session = match RemoteVideoStreamingSession::start(
            owner,
            player,
            encoder,
            quality,
            segment_capacity,
            mute_local_output,
            hide_local_video_output,
        ) {
            Ok(session) => session,
            Err(error) => {
                if speed_changed {
                    player.set_playback_speed(previous_speed);
                }
                return Err(error);
            }
        };
        player.set_playing(true);
        let playback = std::sync::Arc::new(VideoStreamPlaybackState::new(
            player.position_secs(),
            player.duration(),
            player.volume(),
            player.intent_playing(),
        ));
        Ok(AppRemoteVideoStreaming {
            fs_idx,
            requested_path: requested_path.to_path_buf(),
            session,
            playback,
        })
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
                if let Some(FsCacheEntry::Video { player, .. }) =
                    self.fs_cache.get(&streaming.fs_idx)
                {
                    player.set_playing(false);
                }
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
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&opening.fs_idx) {
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
        if let Some(FsCacheEntry::Video { player, .. }) =
            self.fs_cache.get(&starting.streaming.fs_idx)
        {
            player.set_playing(false);
        }
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

    fn poll_remote_video_streaming(&mut self) {
        let Some(state) = self.remote_session_ui.video_stream.take() else {
            return;
        };
        let mut streaming = match state {
            AppRemoteVideoStreamState::Opening(opening) => {
                self.poll_remote_video_opening(opening);
                return;
            }
            AppRemoteVideoStreamState::Starting(starting) => {
                self.poll_remote_video_starting(starting);
                return;
            }
            AppRemoteVideoStreamState::Streaming(streaming) => streaming,
        };
        let outcome = match self.fs_cache.get(&streaming.fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                streaming.playback.update(
                    player.position_secs(),
                    player.duration(),
                    player.volume(),
                    player.intent_playing(),
                );
                streaming.session.reconcile(player)
            }
            _ => StreamReconcile::Stop("streaming media player was closed".to_owned()),
        };
        match outcome {
            StreamReconcile::Active => {
                self.remote_session_ui.video_stream =
                    Some(AppRemoteVideoStreamState::Streaming(streaming));
            }
            StreamReconcile::GenerationChanged(generation) => {
                crate::logger::log(format!(
                    "remote-stream generation changed after seek: {generation:?}"
                ));
                if let Some(handle) = self.remote_session_ui.handle.as_ref() {
                    handle.publish_video_stream(PublishedVideoStream {
                        session: streaming.session.id(),
                        generation: streaming.session.access(),
                        playback: std::sync::Arc::clone(&streaming.playback),
                    });
                }
                self.remote_session_ui.video_stream =
                    Some(AppRemoteVideoStreamState::Streaming(streaming));
            }
            StreamReconcile::Stop(reason) => {
                if let Some(FsCacheEntry::Video { player, .. }) =
                    self.fs_cache.get(&streaming.fs_idx)
                {
                    player.set_playing(false);
                }
                if let Some(handle) = self.remote_session_ui.handle.as_ref() {
                    handle.clear_video_stream(Some(streaming.session.id().0));
                }
                crate::logger::log(format!("remote-stream session stopped: {reason}"));
            }
        }
    }

    fn poll_remote_video_starting(&mut self, starting: AppRemoteVideoStarting) {
        if starting.claimed.ownership_response().status != SessionStatus::Active {
            self.fail_remote_video_starting(
                starting,
                VideoStreamErrorCode::SessionMismatch,
                "リモートセッションの操作権が移動しました".to_owned(),
            );
            return;
        }
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
        let player = match self.fs_cache.get(&starting.streaming.fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player,
            _ => return Err("streaming media player was closed".to_owned()),
        };
        if !crate::folder_tree::path_eq(player.path(), &starting.streaming.requested_path) {
            return Err("streaming media player was replaced".to_owned());
        }
        remote_video_start_outcome_for_player(starting, player)
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
            Ok((StreamReconcile::GenerationChanged(generation), readiness)) => {
                crate::logger::log(format!(
                    "remote-stream generation changed while start was stabilizing: {generation:?}"
                ));
                self.continue_or_timeout_remote_video_starting(starting, readiness);
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

    fn poll_remote_video_opening(&mut self, opening: AppRemoteVideoOpening) {
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

        let readiness = if self.fullscreen_idx != Some(opening.fs_idx) {
            Err((
                VideoStreamErrorCode::Failed,
                "要求した動画を開いている間に本体の表示対象が変更されました".to_owned(),
            ))
        } else {
            match self.fs_cache.get(&opening.fs_idx) {
                Some(FsCacheEntry::Video { player, .. })
                    if !crate::folder_tree::path_eq(player.path(), &opening.requested_path) =>
                {
                    Err((
                        VideoStreamErrorCode::Failed,
                        "本体が開いた動画と要求された動画が一致しません".to_owned(),
                    ))
                }
                Some(FsCacheEntry::Video { player, .. }) => {
                    if let Some(error) = player.error() {
                        Err((
                            VideoStreamErrorCode::Failed,
                            format!("要求された動画を再生できません: {error}"),
                        ))
                    } else if let Some(info) = player.info() {
                        if !info.has_video || !info.has_audio {
                            Err((
                                VideoStreamErrorCode::Failed,
                                "remote streaming requires both video and audio streams".to_owned(),
                            ))
                        } else {
                            Ok(player.audio_tap_source().is_some())
                        }
                    } else {
                        Ok(false)
                    }
                }
                Some(_) => Err((
                    VideoStreamErrorCode::Failed,
                    "要求された動画の player を本体で作成できませんでした".to_owned(),
                )),
                None => Ok(false),
            }
        };

        match readiness {
            Ok(true) => {
                if let Some(error) = opening.budget.expired_error(VideoStreamStartStage::Player) {
                    self.fail_remote_video_opening(opening, error.code, error.message);
                    return;
                }
                let result = self.create_remote_video_streaming(
                    &opening.client_id,
                    &opening.requested_path,
                    opening.quality,
                    opening.fs_idx,
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
            Ok(false) => {
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

    pub(crate) fn remote_video_local_surface_source(
        &self,
        fs_idx: usize,
    ) -> Option<&std::path::Path> {
        match self.remote_session_ui.video_stream.as_ref()? {
            AppRemoteVideoStreamState::Opening(_) => None,
            AppRemoteVideoStreamState::Starting(starting)
                if starting.streaming.fs_idx == fs_idx
                    && starting.streaming.session.hides_local_video_output() =>
            {
                Some(starting.streaming.requested_path.as_path())
            }
            AppRemoteVideoStreamState::Streaming(streaming)
                if streaming.fs_idx == fs_idx && streaming.session.hides_local_video_output() =>
            {
                Some(streaming.requested_path.as_path())
            }
            AppRemoteVideoStreamState::Starting(_) | AppRemoteVideoStreamState::Streaming(_) => {
                None
            }
        }
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
                    streaming.session.set_playing(playing)
                }
            })
    }

    #[allow(dead_code)] // Increment 6 exposes quality selection and returns the new generation.
    pub(crate) fn change_remote_video_streaming_quality(
        &mut self,
        quality: crate::video::stream::quality::QualityPreset,
    ) -> Result<StreamingGeneration, String> {
        let streaming = self
            .remote_session_ui
            .video_stream
            .as_mut()
            .ok_or_else(|| "remote video streaming is not active".to_owned())?;
        let AppRemoteVideoStreamState::Streaming(streaming) = streaming else {
            return Err("remote video streaming is still opening".to_owned());
        };
        let player = match self.fs_cache.get(&streaming.fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player,
            _ => return Err("streaming media player was closed".to_owned()),
        };
        streaming.session.change_quality(player, quality)
    }

    fn apply_pending_remote_ui_requests(&mut self, handle: &SessionHandle) {
        for pending in handle.take_pending_ui_requests() {
            match pending {
                ClaimedRemoteUiRequest::Write(pending) => self.apply_pending_remote_write(pending),
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
                client_id,
                path,
                quality,
                budget,
            } => {
                self.begin_remote_video_start(pending, client_id, path, quality.into(), budget);
                return;
            }
            VideoStreamUiRequest::Control { session, action } => {
                self.apply_remote_video_control(session, action)
            }
            VideoStreamUiRequest::Seek {
                session,
                position_secs,
            } => self.apply_remote_video_seek(session, position_secs),
            VideoStreamUiRequest::Stop { .. } => unreachable!("stop is handled before ownership"),
        };
        pending.complete(outcome);
    }

    fn begin_remote_video_start(
        &mut self,
        claimed: ClaimedVideoStreamUiRequest,
        client_id: String,
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
                    .streaming_owner(&client_id)
                    .map(|_| ())
                    .map_err(|response| response.message)
            });
        if let Err(error) = ownership {
            claimed.complete(video_stream_ui_failure(error));
            return;
        }

        // A new remote start replaces the old remote stream before normal folder/viewer routing
        // can drop its player. The requested address is authoritative while remote owns control.
        self.cancel_remote_video_stream_state(
            VideoStreamErrorCode::Failed,
            "新しい動画ストリーミング開始要求に置き換えられました",
        );
        let fs_idx = match self.open_remote_video_target(&requested_path) {
            Ok(fs_idx) => fs_idx,
            Err(error) => {
                claimed.complete(video_stream_ui_failure(error));
                return;
            }
        };
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            // Opening the normal viewer can honor local autoplay. Remote start owns transport,
            // so hold the new player at its initial position until the stream worker is attached.
            player.set_playing(false);
        }
        self.remote_session_ui.video_stream =
            Some(AppRemoteVideoStreamState::Opening(AppRemoteVideoOpening {
                claimed,
                client_id,
                requested_path,
                quality,
                fs_idx,
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
        let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&streaming.fs_idx) else {
            if let Some(handle) = self.remote_session_ui.handle.as_ref() {
                handle.clear_video_stream(Some(session));
            }
            return video_stream_ui_failure("streaming media player was closed".to_owned());
        };
        let result = match action {
            VideoStreamControlAction::Play => {
                player.set_playing(true);
                Ok(())
            }
            VideoStreamControlAction::Pause => {
                player.set_playing(false);
                Ok(())
            }
            VideoStreamControlAction::Volume { volume }
                if volume.is_finite() && (0.0..=1.0).contains(&volume) =>
            {
                player.set_volume(volume);
                Ok(())
            }
            VideoStreamControlAction::Volume { .. } => {
                Err("volume must be finite and between 0 and 1".to_owned())
            }
            VideoStreamControlAction::Quality { quality } => streaming
                .session
                .change_quality(player, quality.into())
                .map(|_| ()),
        };
        streaming.playback.update(
            player.position_secs(),
            player.duration(),
            player.volume(),
            player.intent_playing(),
        );
        if result.is_ok()
            && let Some(handle) = self.remote_session_ui.handle.as_ref()
        {
            handle.publish_video_stream(PublishedVideoStream {
                session: streaming.session.id(),
                generation: streaming.session.access(),
                playback: std::sync::Arc::clone(&streaming.playback),
            });
        }
        self.remote_session_ui.video_stream = Some(AppRemoteVideoStreamState::Streaming(streaming));
        match result {
            Ok(()) => VideoStreamUiOutcome::Controlled(SessionResponse::active()),
            Err(error) => video_stream_ui_failure(error),
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
        let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&streaming.fs_idx) else {
            if let Some(handle) = self.remote_session_ui.handle.as_ref() {
                handle.clear_video_stream(Some(session));
            }
            return video_stream_ui_failure("streaming media player was closed".to_owned());
        };
        let result = streaming.session.seek(player, position_secs);
        if result.is_ok()
            && let Some(handle) = self.remote_session_ui.handle.as_ref()
        {
            handle.publish_video_stream(PublishedVideoStream {
                session: streaming.session.id(),
                generation: streaming.session.access(),
                playback: std::sync::Arc::clone(&streaming.playback),
            });
        }
        self.remote_session_ui.video_stream = Some(AppRemoteVideoStreamState::Streaming(streaming));
        result
            .map(VideoStreamUiOutcome::Seeked)
            .unwrap_or_else(video_stream_ui_failure)
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
                if let Some(FsCacheEntry::Video { player, .. }) =
                    self.fs_cache.get(&streaming.fs_idx)
                {
                    player.set_playing(false);
                }
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

    fn apply_pending_remote_write(&mut self, pending: ClaimedRemoteWrite) {
        let ownership = pending.ownership_response();
        if ownership.status != SessionStatus::Active {
            pending.complete(UiWriteOutcome::Session(ownership));
            return;
        }
        if matches!(
            pending.request(),
            RemoteWriteRequest::SetBookmark { .. } | RemoteWriteRequest::GetItemState { .. }
        ) {
            self.begin_remote_bookmark_write(pending);
            return;
        }
        let response = self.apply_remote_write(pending.request());
        pending.complete(UiWriteOutcome::Write(response));
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
            RemoteWriteRequest::SetBookmark { .. } | RemoteWriteRequest::GetItemState { .. } => {
                write_error(
                    RemoteWriteErrorCode::Internal,
                    "ブックマーク要求の非同期経路が使われませんでした",
                )
            }
        }
    }

    fn begin_remote_bookmark_write(&mut self, pending: ClaimedRemoteWrite) {
        let request = pending.request().clone();
        let (address, context_address, page_index, requested_presence, supported) = match request {
            RemoteWriteRequest::SetBookmark {
                address,
                context_address,
                page_index,
                bookmarked,
            } => (address, context_address, page_index, Some(bookmarked), true),
            RemoteWriteRequest::GetItemState {
                address,
                context_address,
                page_index,
                bookmark_supported,
            } => (
                address,
                context_address,
                page_index,
                None,
                bookmark_supported,
            ),
            _ => unreachable!("only bookmark-backed requests are deferred"),
        };
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
        if requested_presence.is_none() && !supported {
            pending.complete(UiWriteOutcome::Write(RemoteWriteResponse::Success(
                RemoteWriteResult::item_state(RemoteItemState {
                    rating,
                    bookmark_supported: false,
                    bookmarked: false,
                }),
            )));
            return;
        }
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
        let kind = if let Some(present) = requested_presence {
            service.set_page_presence(request_id, draft, present);
            PendingRemoteBookmarkWriteKind::SetPresence
        } else {
            service.get_page_presence(request_id, draft);
            PendingRemoteBookmarkWriteKind::ReadState { rating }
        };
        self.book_bookmark_pending_requests.insert(request_id);
        self.remote_session_ui.pending_bookmark_writes.insert(
            request_id,
            PendingRemoteBookmarkWrite {
                claimed: pending,
                kind,
            },
        );
    }

    fn persist_remote_spread(
        &mut self,
        address: &mimageviewer_ipc::RemoteAddress,
        spread_mode: RemoteSpreadMode,
        reading_direction: RemoteReadingDirection,
    ) -> RemoteWriteResponse {
        let key = match remote_spread_key(&self.settings, address) {
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
        let target = match remote_reading_target(&self.settings, address, context_address) {
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

    pub(crate) fn finish_remote_bookmark_event(
        &mut self,
        event: &crate::book_bookmarks::BookBookmarkEvent,
    ) -> bool {
        let (request_id, result, is_read) = match event {
            crate::book_bookmarks::BookBookmarkEvent::PagePresenceRead {
                request_id,
                result,
                ..
            } => (*request_id, result, true),
            crate::book_bookmarks::BookBookmarkEvent::PagePresenceSet {
                request_id,
                result,
                ..
            } => (*request_id, result, false),
            _ => return false,
        };
        let Some(pending) = self
            .remote_session_ui
            .pending_bookmark_writes
            .remove(&request_id)
        else {
            return true;
        };
        self.book_bookmark_pending_requests.remove(&request_id);
        let response = match (&pending.kind, result, is_read) {
            (PendingRemoteBookmarkWriteKind::ReadState { rating }, Ok(bookmarked), true) => {
                RemoteWriteResponse::Success(RemoteWriteResult::item_state(RemoteItemState {
                    rating: *rating,
                    bookmark_supported: true,
                    bookmarked: *bookmarked,
                }))
            }
            (PendingRemoteBookmarkWriteKind::SetPresence, Ok(_), false) => {
                self.notify_bookmarks_changed();
                RemoteWriteResponse::Success(RemoteWriteResult::applied())
            }
            (_, Err(error), _) => {
                crate::logger::log(format!(
                    "remote_ipc: bookmark service request failed request_id={request_id} error={error}"
                ));
                write_error(
                    RemoteWriteErrorCode::PersistenceFailed,
                    "ブックマーク DB の操作に失敗しました",
                )
            }
            _ => write_error(
                RemoteWriteErrorCode::Internal,
                "ブックマーク応答の種別が一致しません",
            ),
        };
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
        let snapshot = self
            .remote_session_ui
            .handle
            .as_ref()
            .and_then(|handle| handle.snapshot().active);
        let Some(snapshot) = snapshot else {
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
        let mut disconnect = false;
        egui::Modal::new(egui::Id::new("remote_session_modal")).show(ctx, |ui| {
            ui.heading("リモート接続中");
            ui.add_space(6.0);
            show_connection_summary(ui, &snapshot);
            ui.separator();
            if let Some(source) = streaming_source.as_deref() {
                ui.label(egui::RichText::new(format!("リモートへ配信中: {source}")).strong());
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
                .add_sized([160.0, 34.0], egui::Button::new("切断する"))
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
        if !self.remote_session_ui.show_connection_dialog {
            return;
        }
        let ipc_enabled = self.remote_session_ui.handle.is_some();
        let snapshot = self
            .remote_session_ui
            .handle
            .as_ref()
            .map(SessionHandle::snapshot);
        let remote_web_connected = snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.remote_web_connected);
        let info = snapshot.and_then(|snapshot| snapshot.remote_web);
        let mut open = true;
        egui::Window::new("リモート接続")
            .id(egui::Id::new("remote_connection_dialog"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                if !ipc_enabled {
                    ui.heading("リモート接続は無効です");
                    ui.label("mIV 本体を --remote-ipc 付きで起動してください。");
                    return;
                }
                let Some(info) = info.as_ref() else {
                    if remote_web_connected {
                        ui.heading("remote-web は起動しています");
                        ui.label("接続状態: IPC 接続済み / 接続情報を受信中");
                        ui.label("通常は間もなく URL と QR コードが表示されます。");
                        ui.small(
                            "この表示が続く場合は remote-web と本体の版が一致しているか確認してください。",
                        );
                    } else {
                        ui.heading("remote-web が起動していません");
                        ui.label("接続状態: 本体への IPC 接続なし");
                        ui.label("mimageviewer-remote.exe を起動してください。");
                        ui.small(
                            "既に起動済みなら、自動再接続のため最大 5 秒ほど待ってください。",
                        );
                    }
                    return;
                };

                ui.label("接続状態: remote-web 接続済み");
                ui.label(format!(
                    "tailscale serve: {}",
                    remote_feature_status_label(info.tailscale_serve)
                ));
                ui.label(format!(
                    "PIN: {}",
                    if info.pin_configured {
                        "設定済み"
                    } else {
                        "未設定"
                    }
                ));
                ui.add_space(6.0);
                paint_qr(ui, &info.public_url);
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.monospace(&info.public_url);
                    if ui.button("コピー").clicked() {
                        ui.ctx().copy_text(info.public_url.clone());
                    }
                });
                ui.small("QR コードには URL だけを含み、PIN や Bearer トークンは含みません。");
            });
        self.remote_session_ui.show_connection_dialog = open;
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
                self.open_fullscreen(index);
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
    settings: &crate::settings::Settings,
    address: &mimageviewer_ipc::RemoteAddress,
) -> Result<crate::spread_db::SpreadContainerKey, RemoteWriteError> {
    address.validate_syntax().map_err(|_| {
        RemoteWriteError::new(
            RemoteWriteErrorCode::BadRequest,
            "コンテンツアドレスが不正です",
        )
    })?;
    let favorite_id = uuid::Uuid::parse_str(&address.favorite_id).map_err(|_| {
        RemoteWriteError::new(RemoteWriteErrorCode::BadRequest, "favorite_id が不正です")
    })?;
    let favorite = settings
        .favorites
        .iter()
        .find(|favorite| favorite.id == favorite_id)
        .ok_or_else(|| {
            RemoteWriteError::new(
                RemoteWriteErrorCode::FavoriteNotFound,
                "お気に入りが登録されていません",
            )
        })?;
    let root = logical_favorite_path(&favorite.path, &address.relative_path);
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

fn remote_rating_target(
    settings: &crate::settings::Settings,
    address: &mimageviewer_ipc::RemoteAddress,
) -> Result<RemoteRatingTarget, RemoteWriteError> {
    use crate::rating_db::{RatingItemKind, RatingMeta};

    let root = remote_logical_path(settings, address)?;
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
    settings: &crate::settings::Settings,
    address: &mimageviewer_ipc::RemoteAddress,
    context_address: &mimageviewer_ipc::RemoteAddress,
) -> Result<RemoteReadingTarget, RemoteWriteError> {
    let container_path = remote_logical_path(settings, context_address)?;
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

    let container_path = remote_logical_path(settings, context_address)?;
    let (container_kind, page_identity) = match &address.subresource {
        RemoteSubresource::File => {
            let page_path = remote_logical_path(settings, address)?;
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
    settings: &crate::settings::Settings,
    address: &mimageviewer_ipc::RemoteAddress,
) -> Result<std::path::PathBuf, RemoteWriteError> {
    address.validate_syntax().map_err(|_| {
        RemoteWriteError::new(
            RemoteWriteErrorCode::BadRequest,
            "コンテンツアドレスが不正です",
        )
    })?;
    let favorite_id = uuid::Uuid::parse_str(&address.favorite_id).map_err(|_| {
        RemoteWriteError::new(RemoteWriteErrorCode::BadRequest, "favorite_id が不正です")
    })?;
    let favorite = settings
        .favorites
        .iter()
        .find(|favorite| favorite.id == favorite_id)
        .ok_or_else(|| {
            RemoteWriteError::new(
                RemoteWriteErrorCode::FavoriteNotFound,
                "お気に入りが登録されていません",
            )
        })?;
    Ok(logical_favorite_path(
        &favorite.path,
        &address.relative_path,
    ))
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

    #[test]
    fn remote_video_local_surface_snapshot_dark() {
        use egui_kittest::Harness;

        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(640.0, 360.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(crate::ui_music_timeline::MUSIC_VIEW_BG))
                    .show(ctx, |ui| {
                        draw_remote_video_local_surface(ui, ui.max_rect(), "配信サンプル動画.mp4");
                    });
            });

        harness.run();
        harness.snapshot("remote_video_local_surface_dark");
    }

    #[test]
    fn remote_spread_key_matches_worker_logical_favorite_path() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        std::fs::create_dir_all(root.join("album")).unwrap();
        std::fs::write(root.join("album/book.zip"), b"zip").unwrap();
        let favorite = crate::settings::FavoriteEntry::new("test".to_owned(), root);
        let favorite_id = favorite.id.to_string();
        let mut settings = crate::settings::Settings::default();
        settings.favorites = vec![favorite.clone()];

        for relative in ["", "album/book.zip"] {
            let address = mimageviewer_ipc::RemoteAddress::file(&favorite_id, relative);
            let worker = crate::remote_ipc::path_guard::resolve_existing(
                std::slice::from_ref(&favorite),
                &favorite_id,
                relative,
            )
            .unwrap();
            let ui_key = remote_spread_key(&settings, &address).unwrap();
            assert_eq!(ui_key.exact, worker.logical, "relative={relative:?}");
        }
    }

    #[test]
    fn remote_page_keys_match_local_rating_history_and_bookmark_rules() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let favorite = crate::settings::FavoriteEntry::new("test".to_owned(), root.clone());
        let favorite_id = favorite.id.to_string();
        let settings = crate::settings::Settings {
            favorites: vec![favorite],
            ..Default::default()
        };

        let folder = mimageviewer_ipc::RemoteAddress::file(&favorite_id, "album");
        let image = mimageviewer_ipc::RemoteAddress::file(&favorite_id, "album/page.jpg");
        let image_target = remote_rating_target(&settings, &image).unwrap();
        assert_eq!(
            image_target.key,
            crate::adjustment_db::normalize_path(&root.join("album/page.jpg"))
        );
        let reading = remote_reading_target(&settings, &image, &folder).unwrap();
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
            favorite_id,
            relative_path: "books/book.cbz".to_owned(),
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
