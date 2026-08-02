use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};
use ffmpeg_the_third as ffmpeg;

use super::audio_encoder::open_aac_encoder;
use super::encoder::{EncoderPreference, FrameRate, H264EncoderKind};
use super::playlist::SegmentLookup;
use super::quality::{OutputDimensions, QualityPreset};
use super::segmenter::{Fmp4Segmenter, Fmp4SegmenterError};
use super::timeline::StreamTimeline;
use super::video_tap::open_video_stream_encoder;
use crate::remote_ipc::session::{
    RemoteSessionOwner, RemoteStreamingActivity, RemoteStreamingRegistration,
};

/// Software frame queue。3 枚なら 30fps で 100ms、60fps で 50ms の scheduler jitter を
/// 吸収しつつ、4K YUV420 でも通常 36MiB 程度に留まる。drop 後も CFR slot は詰めない。
pub(crate) const VIDEO_TAP_SOFTWARE_FRAME_CAPACITY: usize = 3;
const AUDIO_TAP_CHUNK_CAPACITY: usize = 32;
const WORKER_COMMAND_CAPACITY: usize = 16;
const RESOURCE_TIMEOUT: Duration = Duration::from_secs(2);

/// 2 秒 GOP の 3 倍。ここまで IDR が来なければ encoder が正常な live 出力を維持できて
/// いないとみなし、generation の自動再試行ではなく streaming session 自体を停止する。
pub(crate) const MAX_PENDING_FRAGMENT_DURATION_SECS: f64 = 6.0;

static NEXT_STREAMING_SESSION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct StreamingSessionId(pub(crate) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct StreamingGeneration(pub(crate) u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StreamReadyInfo {
    pub(crate) encoder: H264EncoderKind,
    pub(crate) output_dimensions: OutputDimensions,
    pub(crate) video_bitrate_bps: u64,
    pub(crate) audio_bitrate_bps: u64,
    pub(crate) codecs: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StreamGenerationStatus {
    Opening,
    Ready(StreamReadyInfo),
    Failed(String),
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamResourceKind {
    MasterPlaylist,
    MediaPlaylist,
    InitSegment,
    MediaSegment(u64),
    State,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StreamSegmentBytes {
    Found(Vec<u8>),
    Gone,
    NotFound,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StreamResource {
    Playlist(Option<String>),
    InitSegment(Option<Vec<u8>>),
    MediaSegment(StreamSegmentBytes),
    State(StreamGenerationMetrics),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StreamGenerationMetrics {
    pub(crate) buffered_secs: f64,
    pub(crate) effective_bitrate_bps: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StreamResourceError {
    GenerationMismatch,
    NotReady,
    Failed(String),
    Stopped,
    Timeout,
}

impl fmt::Display for StreamResourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GenerationMismatch => formatter.write_str("stream generation mismatch"),
            Self::NotReady => formatter.write_str("stream generation is not ready"),
            Self::Failed(error) => write!(formatter, "stream generation failed: {error}"),
            Self::Stopped => formatter.write_str("stream generation stopped"),
            Self::Timeout => formatter.write_str("stream resource request timed out"),
        }
    }
}

struct WorkerCommand {
    kind: StreamResourceKind,
    reply: Sender<StreamResource>,
}

type SharedGenerationStatus = Arc<(Mutex<StreamGenerationStatus>, Condvar)>;

struct GenerationConfig {
    encoder: EncoderPreference,
    quality: QualityPreset,
    source_width: u32,
    source_height: u32,
    frame_rate: FrameRate,
    audio_sample_rate: u32,
    expected_seek_serial: u64,
    source_start_secs: f64,
    segment_capacity: usize,
}

struct GenerationResources {
    _registration: RemoteStreamingRegistration,
    _audio_lease: crate::video::audio::AudioTapLease,
    _video_lease: super::video_tap::VideoTapLease,
    _local_mute: Option<crate::video::clock::RemoteLocalOutputMuteLease>,
}

struct GenerationWorkerCompletion {
    status: StreamGenerationStatus,
    log_line: Option<String>,
}

fn generation_worker_completion(
    result: Result<(), String>,
    cancelled: bool,
) -> GenerationWorkerCompletion {
    if cancelled {
        return GenerationWorkerCompletion {
            status: StreamGenerationStatus::Stopped,
            log_line: None,
        };
    }
    let error = match result {
        Ok(()) => "streaming worker exited without cancellation".to_owned(),
        Err(error) => error,
    };
    GenerationWorkerCompletion {
        log_line: Some(format!("remote-stream generation worker failed: {error}")),
        status: StreamGenerationStatus::Failed(error),
    }
}

fn publish_generation_worker_completion<T>(
    result: Result<(), String>,
    cancel: &AtomicBool,
    status: &SharedGenerationStatus,
    resources: T,
) {
    let completion = generation_worker_completion(result, cancel.load(Ordering::Acquire));
    if let Some(line) = completion.log_line {
        crate::logger::log(line);
    }
    set_generation_status(status, completion.status);
    drop(resources);
}

#[derive(Clone)]
pub(crate) struct StreamingGenerationAccess {
    generation: StreamingGeneration,
    status: SharedGenerationStatus,
    command_tx: Sender<WorkerCommand>,
    activity: RemoteStreamingActivity,
}

pub(crate) struct StreamingGenerationHandle {
    generation: StreamingGeneration,
    expected_seek_serial: u64,
    status: SharedGenerationStatus,
    command_tx: Sender<WorkerCommand>,
    activity: RemoteStreamingActivity,
    cancel: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl StreamingGenerationHandle {
    fn start(
        generation: StreamingGeneration,
        owner: &RemoteSessionOwner,
        player: &crate::video::VideoPlayer,
        encoder: EncoderPreference,
        quality: QualityPreset,
        segment_capacity: usize,
        mute_local_output: bool,
    ) -> Result<Self, String> {
        let info = player
            .info()
            .ok_or_else(|| "video metadata is not ready".to_owned())?;
        if !info.has_video || !info.has_audio {
            return Err("remote streaming requires both video and audio streams".to_owned());
        }
        let frame_rate = FrameRate::new(info.fps_num, info.fps_den)?;
        let (audio_controller, audio_sample_rate) = player
            .audio_tap_source()
            .ok_or_else(|| "audio output is not ready".to_owned())?;
        let expected_seek_serial = player.current_seek_serial();
        let registration = owner
            .register_streaming()
            .map_err(|response| response.message)?;
        let activity = registration.activity();
        let cancel = registration.cancel_flag();
        let (audio_lease, audio_rx) = audio_controller
            .attach(AUDIO_TAP_CHUNK_CAPACITY)
            .map_err(str::to_owned)?;
        let (video_lease, video_rx) = player
            .video_tap_controller()
            .attach(VIDEO_TAP_SOFTWARE_FRAME_CAPACITY)
            .map_err(str::to_owned)?;
        let local_mute = mute_local_output.then(|| player.acquire_remote_local_output_mute());
        let resources = GenerationResources {
            _registration: registration,
            _audio_lease: audio_lease,
            _video_lease: video_lease,
            _local_mute: local_mute,
        };
        let config = GenerationConfig {
            encoder,
            quality,
            source_width: info.width,
            source_height: info.height,
            frame_rate,
            audio_sample_rate,
            expected_seek_serial,
            source_start_secs: player.position_secs(),
            segment_capacity,
        };
        let (command_tx, command_rx) = bounded(WORKER_COMMAND_CAPACITY);
        let status = Arc::new((Mutex::new(StreamGenerationStatus::Opening), Condvar::new()));
        let worker_status = Arc::clone(&status);
        let worker_cancel = Arc::clone(&cancel);
        let worker = std::thread::Builder::new()
            .name("remote-stream-generation".to_owned())
            .spawn(move || {
                let result = run_generation_worker(
                    config,
                    &resources,
                    video_rx,
                    audio_rx,
                    command_rx,
                    Arc::clone(&worker_status),
                    Arc::clone(&worker_cancel),
                );
                // Keep the registration and tap leases alive until after the Result has been
                // classified. RemoteStreamingRegistration::drop sets this same cancel flag, so
                // moving resources into run_generation_worker used to turn every real Err into
                // the observation-only Stopped status and discard its reason.
                publish_generation_worker_completion(
                    result,
                    &worker_cancel,
                    &worker_status,
                    resources,
                );
            })
            .map_err(|error| format!("failed to spawn streaming worker: {error}"))?;
        Ok(Self {
            generation,
            expected_seek_serial,
            status,
            command_tx,
            activity,
            cancel,
            worker: Some(worker),
        })
    }

    pub(crate) fn expected_seek_serial(&self) -> u64 {
        self.expected_seek_serial
    }

    pub(crate) fn status(&self) -> StreamGenerationStatus {
        generation_status(&self.status)
    }

    pub(crate) fn set_playing(&self, playing: bool) -> bool {
        self.activity.set_playing(playing)
    }

    pub(crate) fn access(&self) -> StreamingGenerationAccess {
        StreamingGenerationAccess {
            generation: self.generation,
            status: Arc::clone(&self.status),
            command_tx: self.command_tx.clone(),
            activity: self.activity.clone(),
        }
    }

    pub(crate) fn stop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl StreamingGenerationAccess {
    pub(crate) fn generation(&self) -> StreamingGeneration {
        self.generation
    }

    pub(crate) fn status(&self) -> StreamGenerationStatus {
        generation_status(&self.status)
    }

    pub(crate) fn wait_ready(&self, timeout: Duration) -> StreamGenerationStatus {
        let deadline = Instant::now() + timeout;
        let (status, ready) = &*self.status;
        let mut status = status.lock().unwrap_or_else(|error| error.into_inner());
        while matches!(*status, StreamGenerationStatus::Opening) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (next, wait) = ready
                .wait_timeout(status, remaining)
                .unwrap_or_else(|error| error.into_inner());
            status = next;
            if wait.timed_out() {
                break;
            }
        }
        status.clone()
    }

    pub(crate) fn resource(
        &self,
        generation: StreamingGeneration,
        kind: StreamResourceKind,
    ) -> Result<StreamResource, StreamResourceError> {
        self.resource_with_timeout(generation, kind, RESOURCE_TIMEOUT)
    }

    pub(crate) fn resource_with_timeout(
        &self,
        generation: StreamingGeneration,
        kind: StreamResourceKind,
        timeout: Duration,
    ) -> Result<StreamResource, StreamResourceError> {
        validate_resource_generation(self.generation, generation)?;
        if matches!(kind, StreamResourceKind::MediaSegment(_)) {
            self.activity.note_segment_fetch();
        }
        match self.status() {
            StreamGenerationStatus::Opening => return Err(StreamResourceError::NotReady),
            StreamGenerationStatus::Ready(_) => {}
            StreamGenerationStatus::Failed(error) => {
                return Err(StreamResourceError::Failed(error));
            }
            StreamGenerationStatus::Stopped => return Err(StreamResourceError::Stopped),
        }
        let (reply_tx, reply_rx) = bounded(1);
        self.command_tx
            .try_send(WorkerCommand {
                kind,
                reply: reply_tx,
            })
            .map_err(|_| StreamResourceError::NotReady)?;
        reply_rx
            .recv_timeout(timeout)
            .map_err(|_| StreamResourceError::Timeout)
    }
}

fn generation_status(status: &SharedGenerationStatus) -> StreamGenerationStatus {
    status
        .0
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

fn set_generation_status(status: &SharedGenerationStatus, next: StreamGenerationStatus) {
    *status.0.lock().unwrap_or_else(|error| error.into_inner()) = next;
    status.1.notify_all();
}

fn validate_resource_generation(
    current: StreamingGeneration,
    requested: StreamingGeneration,
) -> Result<(), StreamResourceError> {
    if requested == current {
        Ok(())
    } else {
        Err(StreamResourceError::GenerationMismatch)
    }
}

impl Drop for StreamingGenerationHandle {
    fn drop(&mut self) {
        self.stop();
        let Some(worker) = self.worker.take() else {
            return;
        };
        // Generation replacement and remote-session polling happen on the UI thread. The worker
        // owns heavyweight FFmpeg contexts, so even teardown/join is delegated off that thread.
        let _ = std::thread::Builder::new()
            .name("remote-stream-generation-join".to_owned())
            .spawn(move || {
                let _ = worker.join();
            });
    }
}

pub(crate) struct RemoteVideoStreamingSession {
    id: StreamingSessionId,
    owner: RemoteSessionOwner,
    source_path: PathBuf,
    encoder: EncoderPreference,
    quality: QualityPreset,
    segment_capacity: usize,
    mute_local_output: bool,
    next_generation: u64,
    current: StreamingGenerationHandle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StreamReconcile {
    Active,
    GenerationChanged(StreamingGeneration),
    Stop(String),
}

impl RemoteVideoStreamingSession {
    pub(crate) fn start(
        owner: RemoteSessionOwner,
        player: &crate::video::VideoPlayer,
        encoder: EncoderPreference,
        quality: QualityPreset,
        segment_capacity: usize,
        mute_local_output: bool,
    ) -> Result<Self, String> {
        if segment_capacity == 0 {
            return Err("remote streaming segment capacity must be non-zero".to_owned());
        }
        let generation = StreamingGeneration(1);
        let current = StreamingGenerationHandle::start(
            generation,
            &owner,
            player,
            encoder,
            quality,
            segment_capacity,
            mute_local_output,
        )?;
        Ok(Self {
            id: StreamingSessionId(NEXT_STREAMING_SESSION_ID.fetch_add(1, Ordering::Relaxed)),
            owner,
            source_path: player.path().clone(),
            encoder,
            quality,
            segment_capacity,
            mute_local_output,
            next_generation: 2,
            current,
        })
    }

    pub(crate) fn id(&self) -> StreamingSessionId {
        self.id
    }

    pub(crate) fn status(&self) -> StreamGenerationStatus {
        self.current.status()
    }

    pub(crate) fn access(&self) -> StreamingGenerationAccess {
        self.current.access()
    }

    pub(crate) fn generation_matches_player_seek(
        &self,
        player: &crate::video::VideoPlayer,
    ) -> bool {
        self.current.expected_seek_serial() == player.current_seek_serial()
    }

    pub(crate) fn set_playing(&self, playing: bool) -> bool {
        self.current.set_playing(playing)
    }

    pub(crate) fn change_quality(
        &mut self,
        player: &crate::video::VideoPlayer,
        quality: QualityPreset,
    ) -> Result<StreamingGeneration, String> {
        let previous = self.quality;
        self.quality = quality;
        match self.start_new_generation(player) {
            Ok(generation) => Ok(generation),
            Err(error) => {
                self.quality = previous;
                Err(error)
            }
        }
    }

    pub(crate) fn seek(
        &mut self,
        player: &crate::video::VideoPlayer,
        position_secs: f64,
    ) -> Result<StreamingGeneration, String> {
        if !position_secs.is_finite() || position_secs < 0.0 {
            return Err("stream seek position must be finite and non-negative".to_owned());
        }
        player.seek_for_remote_streaming(position_secs);
        self.start_new_generation(player)
    }

    fn start_new_generation(
        &mut self,
        player: &crate::video::VideoPlayer,
    ) -> Result<StreamingGeneration, String> {
        let generation = StreamingGeneration(self.next_generation);
        let replacement = StreamingGenerationHandle::start(
            generation,
            &self.owner,
            player,
            self.encoder,
            self.quality,
            self.segment_capacity,
            self.mute_local_output,
        )?;
        self.next_generation = self.next_generation.saturating_add(1);
        let mut previous = std::mem::replace(&mut self.current, replacement);
        previous.stop();
        Ok(generation)
    }

    /// UI polling only reconciles cheap atomic/state observations. A seek serial change starts a
    /// fresh worker; encoder open and all FFmpeg ownership remain inside that worker.
    pub(crate) fn reconcile(&mut self, player: &crate::video::VideoPlayer) -> StreamReconcile {
        if !self.owner.is_current() {
            return StreamReconcile::Stop("remote session ownership was lost".to_owned());
        }
        if player.path() != &self.source_path {
            return StreamReconcile::Stop("streaming source was replaced".to_owned());
        }
        match self.current.status() {
            StreamGenerationStatus::Failed(error) => return StreamReconcile::Stop(error),
            StreamGenerationStatus::Stopped => {
                return StreamReconcile::Stop("streaming worker stopped".to_owned());
            }
            StreamGenerationStatus::Opening | StreamGenerationStatus::Ready(_) => {}
        }
        if !self.current.set_playing(player.intent_playing()) {
            return StreamReconcile::Stop("remote session ownership was lost".to_owned());
        }
        if player.current_seek_serial() != self.current.expected_seek_serial() {
            return match self.start_new_generation(player) {
                Ok(generation) => StreamReconcile::GenerationChanged(generation),
                Err(error) => StreamReconcile::Stop(error),
            };
        }
        StreamReconcile::Active
    }
}

struct TimedPacket {
    dts_secs: f64,
    end_secs: f64,
    packet: ffmpeg::Packet,
}

#[derive(Default)]
struct MuxCoordinator {
    video: VecDeque<TimedPacket>,
    audio: VecDeque<TimedPacket>,
    latest_video_dts_secs: Option<f64>,
    latest_audio_end_secs: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FragmentLimitDecision {
    Continue,
    StopSession,
}

fn fragment_limit_decision(pending_duration_secs: f64) -> FragmentLimitDecision {
    if pending_duration_secs > MAX_PENDING_FRAGMENT_DURATION_SECS {
        FragmentLimitDecision::StopSession
    } else {
        FragmentLimitDecision::Continue
    }
}

impl MuxCoordinator {
    fn enqueue_video(
        &mut self,
        packet: ffmpeg::Packet,
        frame_rate: FrameRate,
    ) -> Result<(), String> {
        let seconds_per_tick = f64::from(frame_rate.denominator) / f64::from(frame_rate.numerator);
        let timed = timed_packet(packet, seconds_per_tick, "video")?;
        self.latest_video_dts_secs = Some(timed.dts_secs);
        self.video.push_back(timed);
        self.check_backlog()
    }

    fn enqueue_audio(&mut self, packet: ffmpeg::Packet, sample_rate: u32) -> Result<(), String> {
        let timed = timed_packet(packet, 1.0 / f64::from(sample_rate), "audio")?;
        self.latest_audio_end_secs = Some(timed.end_secs);
        self.audio.push_back(timed);
        self.check_backlog()
    }

    fn check_backlog(&self) -> Result<(), String> {
        let first = self
            .video
            .front()
            .map(|packet| packet.dts_secs)
            .into_iter()
            .chain(self.audio.front().map(|packet| packet.dts_secs))
            .reduce(f64::min);
        let latest = self
            .latest_video_dts_secs
            .into_iter()
            .chain(self.latest_audio_end_secs)
            .reduce(f64::max);
        if first.zip(latest).is_some_and(|(first, latest)| {
            fragment_limit_decision((latest - first).max(0.0)) == FragmentLimitDecision::StopSession
        }) {
            return Err(fragment_limit_error("encoder interleave queue"));
        }
        Ok(())
    }

    fn drain(&mut self, segmenter: &mut Fmp4Segmenter) -> Result<(), String> {
        let Some(watermark) = self
            .latest_video_dts_secs
            .zip(self.latest_audio_end_secs)
            .map(|(video, audio)| video.min(audio))
        else {
            return Ok(());
        };
        loop {
            let video_dts = self.video.front().map(|packet| packet.dts_secs);
            let audio_dts = self.audio.front().map(|packet| packet.dts_secs);
            let take_audio = match (video_dts, audio_dts) {
                (Some(video), Some(audio)) => audio <= video && audio <= watermark,
                (None, Some(audio)) => audio <= watermark,
                _ => false,
            };
            if take_audio {
                let packet = self.audio.pop_front().expect("audio front checked");
                segmenter
                    .push_audio_packet(&packet.packet)
                    .map_err(segmenter_error)?;
            } else if video_dts.is_some_and(|dts| dts <= watermark) {
                let packet = self.video.pop_front().expect("video front checked");
                segmenter
                    .push_packet(&packet.packet)
                    .map_err(segmenter_error)?;
            } else {
                break;
            }
            if fragment_limit_decision(segmenter.pending_fragment_duration_secs())
                == FragmentLimitDecision::StopSession
            {
                return Err(fragment_limit_error("unfinished fMP4 fragment"));
            }
        }
        Ok(())
    }
}

fn timed_packet(
    packet: ffmpeg::Packet,
    seconds_per_tick: f64,
    stream_name: &str,
) -> Result<TimedPacket, String> {
    let dts = packet
        .dts()
        .ok_or_else(|| format!("encoded {stream_name} packet has no DTS"))?;
    let duration = packet.duration().max(1);
    let dts_secs = dts as f64 * seconds_per_tick;
    Ok(TimedPacket {
        dts_secs,
        end_secs: dts.saturating_add(duration) as f64 * seconds_per_tick,
        packet,
    })
}

fn segmenter_error(error: Fmp4SegmenterError) -> String {
    error.to_string()
}

fn fragment_limit_error(owner: &str) -> String {
    format!(
        "{owner} exceeded the {:.1}s pending-fragment limit; stopping streaming session",
        MAX_PENDING_FRAGMENT_DURATION_SECS
    )
}

fn run_generation_worker(
    config: GenerationConfig,
    _resources: &GenerationResources,
    video_rx: Receiver<super::video_tap::TappedVideoFrame>,
    audio_rx: Receiver<crate::video::audio::ProcessedChunk>,
    command_rx: Receiver<WorkerCommand>,
    status: SharedGenerationStatus,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    let timeline =
        StreamTimeline::new(config.source_start_secs).map_err(|error| error.to_string())?;
    // Both encoder opens are deliberately below the worker boundary. App::poll_remote_session
    // never calls either open function and only observes status / seek serials.
    let mut video = open_video_stream_encoder(
        config.encoder,
        config.quality,
        config.source_width,
        config.source_height,
        config.frame_rate,
        config.expected_seek_serial,
        timeline,
    )
    .map_err(|error| error.to_string())?;
    let audio_bitrate_bps = config.quality.parameters().audio_bitrate_bps;
    let mut audio = open_aac_encoder(
        config.audio_sample_rate,
        audio_bitrate_bps,
        config.expected_seek_serial,
        timeline,
    )
    .map_err(|error| error.to_string())?;
    let mut segmenter = Fmp4Segmenter::with_capacity(
        video.encoder(),
        &audio.encoder,
        config.frame_rate,
        config.segment_capacity,
    )
    .map_err(segmenter_error)?;
    let output_dimensions = video.output_parameters().dimensions;
    let ready = StreamGenerationStatus::Ready(StreamReadyInfo {
        encoder: video.encoder_kind(),
        output_dimensions,
        video_bitrate_bps: video.effective_video_bitrate_bps(),
        audio_bitrate_bps: audio.effective_bitrate_bps(),
        codecs: segmenter.codecs().to_owned(),
    });
    set_generation_status(&status, ready);
    let mut mux = MuxCoordinator::default();

    loop {
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        match receive_generation_worker_event(
            &command_rx,
            &video_rx,
            &audio_rx,
            Duration::from_millis(20),
        )? {
            GenerationWorkerEvent::Command(command) => reply_with_resource(command, &segmenter),
            GenerationWorkerEvent::Video(tapped) => {
                for packet in video
                    .encode_frame(tapped, &segmenter)
                    .map_err(|error| error.to_string())?
                {
                    mux.enqueue_video(packet, config.frame_rate)?;
                }
            }
            GenerationWorkerEvent::Audio(chunk) => {
                for packet in audio.push_chunk(chunk).map_err(|error| error.to_string())? {
                    mux.enqueue_audio(packet, audio.output_sample_rate())?;
                }
            }
            GenerationWorkerEvent::Idle => {}
        }
        mux.drain(&mut segmenter)?;
    }
}

enum GenerationWorkerEvent {
    Command(WorkerCommand),
    Video(super::video_tap::TappedVideoFrame),
    Audio(crate::video::audio::ProcessedChunk),
    Idle,
}

fn receive_generation_worker_event(
    command_rx: &Receiver<WorkerCommand>,
    video_rx: &Receiver<super::video_tap::TappedVideoFrame>,
    audio_rx: &Receiver<crate::video::audio::ProcessedChunk>,
    wait: Duration,
) -> Result<GenerationWorkerEvent, String> {
    crossbeam_channel::select! {
        recv(command_rx) -> command => command
            .map(GenerationWorkerEvent::Command)
            .map_err(|_| "stream resource command channel disconnected".to_owned()),
        recv(video_rx) -> tapped => tapped
            .map(GenerationWorkerEvent::Video)
            .map_err(|_| "video tap disconnected".to_owned()),
        recv(audio_rx) -> chunk => chunk
            .map(GenerationWorkerEvent::Audio)
            .map_err(|_| "audio tap disconnected".to_owned()),
        default(wait) => Ok(GenerationWorkerEvent::Idle),
    }
}

fn reply_with_resource(command: WorkerCommand, segmenter: &Fmp4Segmenter) {
    let resource = match command.kind {
        StreamResourceKind::MasterPlaylist => StreamResource::Playlist(segmenter.master_playlist()),
        StreamResourceKind::MediaPlaylist => StreamResource::Playlist(segmenter.media_playlist()),
        StreamResourceKind::InitSegment => {
            StreamResource::InitSegment(segmenter.init_segment().map(<[u8]>::to_vec))
        }
        StreamResourceKind::MediaSegment(sequence) => {
            StreamResource::MediaSegment(match segmenter.segment(sequence) {
                SegmentLookup::Found(segment) => StreamSegmentBytes::Found(segment.bytes.clone()),
                SegmentLookup::Gone => StreamSegmentBytes::Gone,
                SegmentLookup::NotFound => StreamSegmentBytes::NotFound,
            })
        }
        StreamResourceKind::State => StreamResource::State(StreamGenerationMetrics {
            buffered_secs: segmenter.buffered_duration_secs(),
            effective_bitrate_bps: segmenter.effective_bitrate_bps(),
        }),
    };
    let _ = command.reply.send(resource);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SetCancelOnDrop(Arc<AtomicBool>);

    impl Drop for SetCancelOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[test]
    fn worker_error_reason_survives_as_failed_status_and_log_line() {
        let reason = "video tap disconnected";
        let completion = generation_worker_completion(Err(reason.to_owned()), false);

        assert_eq!(
            completion.status,
            StreamGenerationStatus::Failed(reason.to_owned())
        );
        assert_eq!(
            completion.log_line.as_deref(),
            Some("remote-stream generation worker failed: video tap disconnected")
        );
    }

    #[test]
    fn excessive_pre_session_audio_becomes_failed_status_with_values() {
        const SAMPLE_RATE: u32 = 48_000;
        let source_start_secs = 67.267_2;
        let timeline = StreamTimeline::new(source_start_secs).unwrap();
        let mut audio = open_aac_encoder(SAMPLE_RATE, 96_000, 1, timeline).unwrap();
        let result = audio
            .push_chunk(crate::video::audio::ProcessedChunk {
                samples: vec![0.0; 1_024 * 2],
                audible_pts_secs: source_start_secs - 1.0,
                duration_secs: 1_024.0 / f64::from(SAMPLE_RATE),
                source_secs_per_output_sec: 1.0,
                seek_serial: 1,
                pdc_latency_secs_at_process: 0.070_227,
            })
            .map(|_| ())
            .map_err(|error| error.to_string());

        let completion = generation_worker_completion(result, false);
        let StreamGenerationStatus::Failed(reason) = completion.status else {
            panic!("audio timeline error did not fail the generation");
        };
        assert!(reason.contains("audible_pts_secs=66.267200000"));
        assert!(reason.contains("source_start_secs=67.267200000"));
        assert!(reason.contains("allowed_lead_secs=0.091560333"));
        assert!(reason.contains("excess_secs=0.908439667"));
        assert_eq!(
            completion.log_line.as_deref(),
            Some(format!("remote-stream generation worker failed: {reason}").as_str())
        );
    }

    #[test]
    fn worker_error_is_published_before_resource_drop_sets_cancel() {
        let cancel = Arc::new(AtomicBool::new(false));
        let status = Arc::new((Mutex::new(StreamGenerationStatus::Opening), Condvar::new()));

        publish_generation_worker_completion(
            Err("video tap disconnected".to_owned()),
            &cancel,
            &status,
            SetCancelOnDrop(Arc::clone(&cancel)),
        );

        assert!(cancel.load(Ordering::Acquire));
        assert_eq!(
            generation_status(&status),
            StreamGenerationStatus::Failed("video tap disconnected".to_owned())
        );
    }

    #[test]
    fn generation_worker_remains_idle_while_waiting_for_first_video_frame() {
        let (command_tx, command_rx) = bounded::<WorkerCommand>(1);
        let (video_tx, video_rx) = bounded::<crate::video::stream::video_tap::TappedVideoFrame>(1);
        let (audio_tx, audio_rx) = bounded::<crate::video::audio::ProcessedChunk>(1);

        for _ in 0..3 {
            assert!(matches!(
                receive_generation_worker_event(
                    &command_rx,
                    &video_rx,
                    &audio_rx,
                    Duration::from_millis(1),
                ),
                Ok(GenerationWorkerEvent::Idle)
            ));
        }

        // Keep every producer alive across all idle waits. No video frame yet is an idle state,
        // not a disconnected or completed worker.
        drop((command_tx, video_tx, audio_tx));
    }

    #[test]
    fn unfinished_fragment_limit_stops_instead_of_restarting_a_generation() {
        assert_eq!(
            fragment_limit_decision(MAX_PENDING_FRAGMENT_DURATION_SECS),
            FragmentLimitDecision::Continue
        );
        assert_eq!(
            fragment_limit_decision(MAX_PENDING_FRAGMENT_DURATION_SECS + f64::EPSILON * 8.0),
            FragmentLimitDecision::StopSession
        );
        assert!(fragment_limit_error("test").contains("stopping streaming session"));

        let rate = FrameRate::new(30, 1).unwrap();
        let packet_at = |dts| {
            let mut packet = ffmpeg::Packet::empty();
            packet.set_dts(Some(dts));
            packet.set_duration(1);
            packet
        };
        let mut mux = MuxCoordinator::default();
        mux.enqueue_video(packet_at(0), rate).unwrap();
        mux.enqueue_video(packet_at(180), rate).unwrap();
        let error = mux.enqueue_video(packet_at(181), rate).unwrap_err();
        assert!(error.contains("6.0s pending-fragment limit"));
        assert!(error.contains("stopping streaming session"));
    }

    #[test]
    fn old_generation_is_rejected_before_any_current_resource_can_be_read() {
        assert_eq!(
            validate_resource_generation(StreamingGeneration(8), StreamingGeneration(7)),
            Err(StreamResourceError::GenerationMismatch)
        );
        assert_eq!(
            validate_resource_generation(StreamingGeneration(8), StreamingGeneration(8)),
            Ok(())
        );
    }
}
