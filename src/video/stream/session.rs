use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::encoder::{EncoderPreference, H264EncoderKind, SEGMENT_DURATION_SECS};
use super::quality::{OutputDimensions, QualityPreset};
use crate::remote_ipc::session::{
    RemoteSessionOwner, RemoteStreamingActivity, RemoteStreamingControlError,
    RemoteStreamingRegistration,
};
use crate::video::clockless_transcode::{
    ClocklessAudioProcessing, ClocklessOutputInfo, ClocklessSegmentBytes, ClocklessStreamOutput,
    ClocklessTranscodeControl, ClocklessTranscodeOptions, ClocklessVstStatus,
    ClocklessVstStatusSnapshot, run_clockless_stream,
};
use mimageviewer_ipc::RemoteAudioTrack;

const RESOURCE_TIMEOUT: Duration = Duration::from_secs(2);

static NEXT_STREAMING_SESSION_ID: AtomicU64 = AtomicU64::new(1);
static GENERATION_RESOURCE_GATE: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct StreamingSessionId(pub(crate) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct StreamingGeneration(pub(crate) u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StreamReadyVideoInfo {
    pub(crate) encoder: H264EncoderKind,
    pub(crate) output_dimensions: OutputDimensions,
    pub(crate) bitrate_bps: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StreamReadyInfo {
    pub(crate) video: Option<StreamReadyVideoInfo>,
    pub(crate) audio_stream_index: Option<usize>,
    pub(crate) audio_bitrate_bps: u64,
    pub(crate) codecs: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StreamGenerationStatus {
    Opening,
    Ready(StreamReadyInfo),
    Ended(StreamReadyInfo),
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
    pub(crate) source_origin_secs: f64,
    pub(crate) generated_start_secs: f64,
    pub(crate) generated_end_secs: f64,
    pub(crate) ring_start_secs: f64,
    pub(crate) ring_end_secs: f64,
    pub(crate) earliest_sequence: Option<u64>,
    pub(crate) latest_sequence: Option<u64>,
    pub(crate) buffered_secs: f64,
    pub(crate) effective_bitrate_bps: u64,
    pub(crate) ended: bool,
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

struct GenerationStatusState {
    status: StreamGenerationStatus,
    /// A successful Ready publication remains observable if ownership cancellation later
    /// changes the public status to Stopped before the UI retires the generation.
    last_ready: Option<StreamReadyInfo>,
}

type SharedGenerationStatus = Arc<(Mutex<GenerationStatusState>, Condvar)>;

fn new_generation_status() -> SharedGenerationStatus {
    Arc::new((
        Mutex::new(GenerationStatusState {
            status: StreamGenerationStatus::Opening,
            last_ready: None,
        }),
        Condvar::new(),
    ))
}

struct GenerationConfig {
    generation: StreamingGeneration,
    path: PathBuf,
    encoder: EncoderPreference,
    quality: QualityPreset,
    source_origin_secs: f64,
    segment_capacity: usize,
    hw_decode: bool,
    audio_stream_index: usize,
    default_audio_stream_index: Option<usize>,
    normalize_snapshot: RemoteNormalizeSnapshot,
    audio_processing: ClocklessAudioProcessing,
}

#[derive(Clone)]
pub(crate) struct RemoteNormalizeSnapshot {
    pub(crate) enabled: bool,
    pub(crate) target_lufs_milli: i32,
    pub(crate) db_path: PathBuf,
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

fn publish_generation_worker_completion(
    result: Result<(), String>,
    cancel: &AtomicBool,
    status: &SharedGenerationStatus,
    output: &ClocklessStreamOutput,
) {
    if result.is_ok()
        && !cancel.load(Ordering::Acquire)
        && let Some(info) = output.info()
    {
        set_generation_status(
            status,
            StreamGenerationStatus::Ended(stream_ready_info(info)),
        );
        return;
    }
    let completion = generation_worker_completion(result, cancel.load(Ordering::Acquire));
    if let Some(line) = completion.log_line {
        crate::logger::log(line);
    }
    set_generation_status(status, completion.status);
}

fn stream_ready_info(info: ClocklessOutputInfo) -> StreamReadyInfo {
    StreamReadyInfo {
        video: info.video.map(|video| StreamReadyVideoInfo {
            encoder: video.encoder,
            output_dimensions: video.output_dimensions,
            bitrate_bps: video.bitrate_bps,
        }),
        audio_stream_index: info.audio_stream_index,
        audio_bitrate_bps: info.audio_bitrate_bps,
        codecs: info.codecs,
    }
}

/// FFmpeg owns the auxiliary decoder's D3D11 device and the selected H.264 encoder session.
/// Keep their entire lifetimes under one process-wide lease: a replacement may be queued before
/// the UI has finished dropping the old handle, but it cannot allocate GPU resources until the
/// cancelled worker has returned and all of its stack-owned FFmpeg contexts have been dropped.
fn run_with_generation_resource_lease(
    cancel: &AtomicBool,
    run: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let gate = GENERATION_RESOURCE_GATE.get_or_init(|| Mutex::new(()));
    let _lease = gate.lock().unwrap_or_else(|error| error.into_inner());
    if cancel.load(Ordering::Acquire) {
        return Err("clockless transcode cancelled before resource allocation".to_owned());
    }
    run()
}

#[derive(Clone)]
pub(crate) struct StreamingGenerationAccess {
    generation: StreamingGeneration,
    status: SharedGenerationStatus,
    output: ClocklessStreamOutput,
    control: ClocklessTranscodeControl,
    activity: RemoteStreamingActivity,
    audio_status: ClocklessVstStatus,
}

pub(crate) struct StreamingGenerationHandle {
    generation: StreamingGeneration,
    status: SharedGenerationStatus,
    output: ClocklessStreamOutput,
    control: ClocklessTranscodeControl,
    activity: RemoteStreamingActivity,
    audio_status: ClocklessVstStatus,
    _registration: RemoteStreamingRegistration,
    cancel: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl StreamingGenerationHandle {
    fn start(
        generation: StreamingGeneration,
        owner: &RemoteSessionOwner,
        source_path: PathBuf,
        source_origin_secs: f64,
        encoder: EncoderPreference,
        quality: QualityPreset,
        segment_capacity: usize,
        hw_decode: bool,
        audio_stream_index: usize,
        default_audio_stream_index: Option<usize>,
        normalize_snapshot: RemoteNormalizeSnapshot,
        audio_processing: ClocklessAudioProcessing,
    ) -> Result<Self, String> {
        let config = GenerationConfig {
            generation,
            path: source_path,
            encoder,
            quality,
            source_origin_secs,
            segment_capacity,
            hw_decode,
            audio_stream_index,
            default_audio_stream_index,
            normalize_snapshot,
            audio_processing,
        };
        let audio_status = config.audio_processing.vst3_status();
        let output = ClocklessStreamOutput::new(segment_capacity, source_origin_secs)?;
        let control = ClocklessTranscodeControl::manual(segment_capacity)?;
        let mut registration = owner
            .register_streaming()
            .map_err(|response| response.message)?;
        let activity = registration.activity();
        let cancel = registration.cancel_flag();
        let worker_lease = registration.take_worker_lease();
        control.bind_cancel_flag(Arc::clone(&cancel));
        let status = new_generation_status();
        let worker_status = Arc::clone(&status);
        let worker_cancel = Arc::clone(&cancel);
        let worker_output = output.clone();
        let worker_control = control.clone();
        let worker = std::thread::Builder::new()
            .name("remote-stream-generation".to_owned())
            .spawn(move || {
                let ready_status = Arc::clone(&worker_status);
                let result = run_with_generation_resource_lease(&worker_cancel, || {
                    run_generation_worker(
                        config,
                        &worker_control,
                        worker_output.clone(),
                        move |info| {
                            set_generation_status(
                                &ready_status,
                                StreamGenerationStatus::Ready(stream_ready_info(info)),
                            );
                        },
                    )
                });
                publish_generation_worker_completion(
                    result,
                    &worker_cancel,
                    &worker_status,
                    &worker_output,
                );
                // The drain lease follows the actual FFmpeg/GPU worker. The control registration
                // remains with the generation handle so an already-encoded stream can still be
                // paused while the browser consumes its buffered tail.
                drop(worker_lease);
            })
            .map_err(|error| format!("failed to spawn streaming worker: {error}"))?;
        Ok(Self {
            generation,
            status,
            output,
            control,
            activity,
            audio_status,
            _registration: registration,
            cancel,
            worker: Some(worker),
        })
    }

    pub(crate) fn status(&self) -> StreamGenerationStatus {
        generation_status(&self.status)
    }

    pub(crate) fn set_playing(&self, playing: bool) -> Result<(), RemoteStreamingControlError> {
        self.activity.set_playing(playing)
    }

    pub(crate) fn access(&self) -> StreamingGenerationAccess {
        StreamingGenerationAccess {
            generation: self.generation,
            status: Arc::clone(&self.status),
            output: self.output.clone(),
            control: self.control.clone(),
            activity: self.activity.clone(),
            audio_status: self.audio_status.clone(),
        }
    }

    /// Read the last successful Ready publication and close admission under the same status
    /// lock. A worker publishing Ready immediately before this transition is included; one
    /// arriving afterward cannot turn the retired generation Ready again.
    fn retire(&mut self) -> Option<StreamReadyInfo> {
        let ready = retire_generation_status(&self.status, &self.cancel);
        self.control.cancel();
        ready
    }

    pub(crate) fn stop(&mut self) {
        self.retire();
    }
}

impl StreamingGenerationAccess {
    pub(crate) fn generation(&self) -> StreamingGeneration {
        self.generation
    }

    pub(crate) fn status(&self) -> StreamGenerationStatus {
        generation_status(&self.status)
    }

    pub(crate) fn audio_status(&self) -> ClocklessVstStatusSnapshot {
        self.audio_status.snapshot()
    }

    pub(crate) fn wait_ready(&self, timeout: Duration) -> StreamGenerationStatus {
        let deadline = Instant::now() + timeout;
        let (status, ready) = &*self.status;
        let mut status = status.lock().unwrap_or_else(|error| error.into_inner());
        while matches!(status.status, StreamGenerationStatus::Opening) {
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
        status.status.clone()
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
        _timeout: Duration,
    ) -> Result<StreamResource, StreamResourceError> {
        validate_resource_generation(self.generation, generation)?;
        if matches!(kind, StreamResourceKind::MediaSegment(_)) {
            self.activity.note_segment_fetch();
        }
        match self.status() {
            StreamGenerationStatus::Opening => return Err(StreamResourceError::NotReady),
            StreamGenerationStatus::Ready(_) | StreamGenerationStatus::Ended(_) => {}
            StreamGenerationStatus::Failed(error) => {
                return Err(StreamResourceError::Failed(error));
            }
            StreamGenerationStatus::Stopped => return Err(StreamResourceError::Stopped),
        }
        let resource = match kind {
            StreamResourceKind::MasterPlaylist => {
                StreamResource::Playlist(self.output.master_playlist())
            }
            StreamResourceKind::MediaPlaylist => {
                StreamResource::Playlist(self.output.media_playlist())
            }
            StreamResourceKind::InitSegment => {
                StreamResource::InitSegment(self.output.init_segment())
            }
            StreamResourceKind::MediaSegment(sequence) => {
                let bytes = match self.output.segment(sequence) {
                    ClocklessSegmentBytes::Found(bytes) => {
                        self.control.release_through(sequence);
                        StreamSegmentBytes::Found(bytes)
                    }
                    ClocklessSegmentBytes::Gone => StreamSegmentBytes::Gone,
                    ClocklessSegmentBytes::NotFound => StreamSegmentBytes::NotFound,
                };
                StreamResource::MediaSegment(bytes)
            }
            StreamResourceKind::State => {
                let metrics = self.output.metrics();
                StreamResource::State(StreamGenerationMetrics {
                    source_origin_secs: metrics.source_origin_secs,
                    generated_start_secs: metrics.generated_start_secs,
                    generated_end_secs: metrics.generated_end_secs,
                    ring_start_secs: metrics.ring_start_secs,
                    ring_end_secs: metrics.ring_end_secs,
                    earliest_sequence: metrics.earliest_sequence,
                    latest_sequence: metrics.latest_sequence,
                    buffered_secs: metrics.buffered_secs,
                    effective_bitrate_bps: metrics.effective_bitrate_bps,
                    ended: metrics.ended,
                })
            }
        };
        Ok(resource)
    }
}

fn generation_status(status: &SharedGenerationStatus) -> StreamGenerationStatus {
    status
        .0
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .status
        .clone()
}

fn set_generation_status(status: &SharedGenerationStatus, next: StreamGenerationStatus) {
    let mut state = status.0.lock().unwrap_or_else(|error| error.into_inner());
    if publish_generation_status(&mut state, next) {
        status.1.notify_all();
    }
}

fn publish_generation_status(
    state: &mut GenerationStatusState,
    next: StreamGenerationStatus,
) -> bool {
    if matches!(state.status, StreamGenerationStatus::Stopped) {
        return false;
    }
    match &next {
        StreamGenerationStatus::Ready(info) | StreamGenerationStatus::Ended(info) => {
            state.last_ready = Some(info.clone());
        }
        StreamGenerationStatus::Failed(_) => state.last_ready = None,
        StreamGenerationStatus::Opening | StreamGenerationStatus::Stopped => {}
    }
    state.status = next;
    true
}

fn generation_confirmation_snapshot(
    status: &SharedGenerationStatus,
) -> (StreamGenerationStatus, Option<StreamReadyInfo>) {
    let state = status.0.lock().unwrap_or_else(|error| error.into_inner());
    (state.status.clone(), state.last_ready.clone())
}

fn retire_generation_status(
    status: &SharedGenerationStatus,
    cancel: &AtomicBool,
) -> Option<StreamReadyInfo> {
    let mut state = status.0.lock().unwrap_or_else(|error| error.into_inner());
    let ready = state.last_ready.take();
    cancel.store(true, Ordering::Release);
    state.status = StreamGenerationStatus::Stopped;
    status.1.notify_all();
    ready
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
    hw_decode: bool,
    audio_selection: RemoteAudioSelection,
    audio_tracks: Vec<RemoteAudioTrack>,
    default_audio_stream_index: Option<usize>,
    normalize_snapshot: RemoteNormalizeSnapshot,
    audio_processing: ClocklessAudioProcessing,
    next_generation: u64,
    current: StreamingGenerationHandle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GenerationChange {
    pub(crate) generation: StreamingGeneration,
    pub(crate) confirmed_audio_choice: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StreamReconcile {
    Active,
    Stop(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteAudioSelection {
    Settled(usize),
    Requested {
        generation: StreamingGeneration,
        stream_index: usize,
    },
}

impl RemoteAudioSelection {
    fn stream_index(self) -> usize {
        match self {
            Self::Settled(index)
            | Self::Requested {
                stream_index: index,
                ..
            } => index,
        }
    }
}

fn confirmed_audio_choice(
    selection: RemoteAudioSelection,
    current: StreamingGeneration,
    ready: Option<&StreamReadyInfo>,
) -> Option<usize> {
    let RemoteAudioSelection::Requested {
        generation,
        stream_index,
    } = selection
    else {
        return None;
    };
    if generation != current {
        return None;
    }
    ready
        .filter(|ready| ready.audio_stream_index == Some(stream_index))
        .map(|_| stream_index)
}

fn resolve_remote_audio_start(
    tracks: &[RemoteAudioTrack],
    opened_stream_index: usize,
    requested: Option<usize>,
) -> (usize, bool) {
    let valid_request =
        requested.filter(|index| tracks.iter().any(|track| track.stream_index == *index));
    (
        valid_request.unwrap_or(opened_stream_index),
        valid_request.is_some(),
    )
}

impl RemoteVideoStreamingSession {
    pub(crate) fn start(
        owner: RemoteSessionOwner,
        player: &crate::video::VideoPlayer,
        inputs: crate::video::RemoteStreamStartInputs,
        requested_audio_track: Option<usize>,
        encoder: EncoderPreference,
        quality: QualityPreset,
        segment_capacity: usize,
        hw_decode: bool,
        normalize_snapshot: RemoteNormalizeSnapshot,
        audio_processing: ClocklessAudioProcessing,
    ) -> Result<Self, String> {
        if segment_capacity == 0 {
            return Err("remote streaming segment capacity must be non-zero".to_owned());
        }
        validate_remote_stream_tracks(inputs)?;
        let info = player
            .info()
            .ok_or("remote streaming player has no media information")?;
        let audio_tracks: Vec<_> = info
            .audio_tracks
            .iter()
            .map(|track| RemoteAudioTrack {
                stream_index: track.stream_index,
                label: crate::video::audio_track_ui::audio_track_label(
                    track,
                    info.default_audio_stream_index,
                    crate::video::audio_track_selection::AudioTrackSelectionDisplayState::Applied,
                ),
                is_default: info.default_audio_stream_index == Some(track.stream_index),
            })
            .collect();
        let (audio_stream_index, explicit_choice) = resolve_remote_audio_start(
            &audio_tracks,
            inputs.audio_stream_index,
            requested_audio_track,
        );
        let generation = StreamingGeneration(1);
        let current = StreamingGenerationHandle::start(
            generation,
            &owner,
            player.path().clone(),
            inputs.source_origin_secs,
            encoder,
            quality,
            segment_capacity,
            hw_decode,
            audio_stream_index,
            inputs.default_audio_stream_index,
            normalize_snapshot.clone(),
            audio_processing.clone(),
        )?;
        Ok(Self {
            id: StreamingSessionId(NEXT_STREAMING_SESSION_ID.fetch_add(1, Ordering::Relaxed)),
            owner,
            source_path: player.path().clone(),
            encoder,
            quality,
            segment_capacity,
            hw_decode,
            audio_selection: if explicit_choice {
                RemoteAudioSelection::Requested {
                    generation,
                    stream_index: audio_stream_index,
                }
            } else {
                RemoteAudioSelection::Settled(audio_stream_index)
            },
            audio_tracks,
            default_audio_stream_index: inputs.default_audio_stream_index,
            normalize_snapshot,
            audio_processing,
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

    pub(crate) fn generation(&self) -> StreamingGeneration {
        self.current.generation
    }

    pub(crate) fn audio_tracks(&self) -> &[RemoteAudioTrack] {
        &self.audio_tracks
    }

    /// Only the current, successfully opened generation can confirm a user selection.
    pub(crate) fn take_confirmed_audio_choice(&mut self) -> Option<usize> {
        let (status, ready) = generation_confirmation_snapshot(&self.current.status);
        let confirmed =
            confirmed_audio_choice(self.audio_selection, self.generation(), ready.as_ref());
        if let RemoteAudioSelection::Requested {
            generation,
            stream_index,
        } = self.audio_selection
            && (generation != self.generation()
                || !matches!(status, StreamGenerationStatus::Opening))
        {
            self.audio_selection = RemoteAudioSelection::Settled(stream_index);
        }
        confirmed
    }

    /// Retire the current generation and return a choice confirmed at that exact boundary.
    /// The App remains the only writer of the file-scoped settings table.
    pub(crate) fn retire(&mut self) -> Option<usize> {
        let ready = self.current.retire();
        let confirmed = confirmed_audio_choice(
            self.audio_selection,
            self.current.generation,
            ready.as_ref(),
        );
        self.audio_selection = RemoteAudioSelection::Settled(self.audio_selection.stream_index());
        confirmed
    }

    pub(crate) fn access(&self) -> StreamingGenerationAccess {
        self.current.access()
    }

    pub(crate) fn buffer_target_secs(&self) -> f64 {
        self.segment_capacity as f64 * f64::from(SEGMENT_DURATION_SECS)
    }

    pub(crate) fn set_playing(&self, playing: bool) -> Result<(), RemoteStreamingControlError> {
        self.current.set_playing(playing)
    }

    pub(crate) fn change_quality(
        &mut self,
        quality: QualityPreset,
        position_secs: f64,
    ) -> Result<GenerationChange, String> {
        let previous = self.quality;
        self.quality = quality;
        match self.start_new_generation(position_secs) {
            Ok(generation) => Ok(generation),
            Err(error) => {
                self.quality = previous;
                Err(error)
            }
        }
    }

    pub(crate) fn change_audio_track(
        &mut self,
        stream_index: usize,
        position_secs: f64,
    ) -> Result<GenerationChange, String> {
        if !self
            .audio_tracks
            .iter()
            .any(|track| track.stream_index == stream_index)
        {
            return Err("audio stream index is not available".to_owned());
        }
        if !position_secs.is_finite() || position_secs < 0.0 {
            return Err("audio track position must be finite and non-negative".to_owned());
        }
        let change = self.start_new_generation_with_audio(position_secs, stream_index)?;
        self.audio_selection = RemoteAudioSelection::Requested {
            generation: change.generation,
            stream_index,
        };
        Ok(change)
    }

    pub(crate) fn seek(&mut self, position_secs: f64) -> Result<GenerationChange, String> {
        if !position_secs.is_finite() || position_secs < 0.0 {
            return Err("stream seek position must be finite and non-negative".to_owned());
        }
        self.start_new_generation(position_secs)
    }

    fn start_new_generation(
        &mut self,
        source_origin_secs: f64,
    ) -> Result<GenerationChange, String> {
        self.start_new_generation_with_audio(
            source_origin_secs,
            self.audio_selection.stream_index(),
        )
    }

    fn start_new_generation_with_audio(
        &mut self,
        source_origin_secs: f64,
        audio_stream_index: usize,
    ) -> Result<GenerationChange, String> {
        let generation = StreamingGeneration(self.next_generation);
        let replacement = StreamingGenerationHandle::start(
            generation,
            &self.owner,
            self.source_path.clone(),
            source_origin_secs,
            self.encoder,
            self.quality,
            self.segment_capacity,
            self.hw_decode,
            audio_stream_index,
            self.default_audio_stream_index,
            self.normalize_snapshot.clone(),
            self.audio_processing.clone(),
        )?;
        self.next_generation = self.next_generation.saturating_add(1);
        let mut previous = std::mem::replace(&mut self.current, replacement);
        let ready = previous.retire();
        let confirmed_audio_choice =
            confirmed_audio_choice(self.audio_selection, previous.generation, ready.as_ref());
        self.audio_selection = RemoteAudioSelection::Settled(audio_stream_index);
        Ok(GenerationChange {
            generation,
            confirmed_audio_choice,
        })
    }

    /// UI polling only reconciles ownership and worker status. The headless metadata player is
    /// deliberately not a transport clock for this session.
    pub(crate) fn reconcile(&mut self) -> StreamReconcile {
        if !self.owner.is_current() {
            return StreamReconcile::Stop("remote session ownership was lost".to_owned());
        }
        match self.current.status() {
            StreamGenerationStatus::Failed(error) => return StreamReconcile::Stop(error),
            StreamGenerationStatus::Stopped => {
                return StreamReconcile::Stop("streaming worker stopped".to_owned());
            }
            StreamGenerationStatus::Opening
            | StreamGenerationStatus::Ready(_)
            | StreamGenerationStatus::Ended(_) => {}
        }
        StreamReconcile::Active
    }
}

fn validate_remote_stream_tracks(
    inputs: crate::video::RemoteStreamStartInputs,
) -> Result<(), String> {
    if inputs.has_audio {
        Ok(())
    } else {
        Err("remote streaming requires an audio stream".to_owned())
    }
}

fn run_generation_worker(
    config: GenerationConfig,
    control: &ClocklessTranscodeControl,
    output: ClocklessStreamOutput,
    on_ready: impl FnOnce(ClocklessOutputInfo),
) -> Result<(), String> {
    let normalize_gain = remote_generation_normalize_gain(&config);
    let mut audio_processing = config.audio_processing.clone();
    audio_processing.normalize_gain = normalize_gain;
    let options = generation_transcode_options(&config);
    run_clockless_stream(&options, control, output, audio_processing, on_ready).map(|_| ())
}

fn generation_transcode_options(config: &GenerationConfig) -> ClocklessTranscodeOptions {
    let quality = match config.quality {
        QualityPreset::Minimum => crate::video::clockless_transcode::ClocklessQuality::Minimum,
        QualityPreset::Low => crate::video::clockless_transcode::ClocklessQuality::Low,
        QualityPreset::Standard => crate::video::clockless_transcode::ClocklessQuality::Standard,
        QualityPreset::High => crate::video::clockless_transcode::ClocklessQuality::High,
    };
    ClocklessTranscodeOptions {
        path: config.path.clone(),
        include_audio: true,
        audio_stream_index: config.audio_stream_index,
        hw_decode: config.hw_decode,
        quality,
        encoder: config.encoder,
        max_source_secs: None,
        segment_capacity: config.segment_capacity,
        profile_swscale: false,
        source_origin_secs: config.source_origin_secs,
        diagnostic_generation: Some(config.generation.0),
    }
}

fn remote_generation_normalize_gain(config: &GenerationConfig) -> f64 {
    if !config.normalize_snapshot.enabled {
        return 1.0;
    }
    let result = crate::audio_normalize_db::AudioNormalizeDb::open_read_only_at(
        &config.normalize_snapshot.db_path,
    )
    .map_err(|error| error.to_string())
    .and_then(|db| {
        db.lookup_checked(
            &config.path,
            config.normalize_snapshot.target_lufs_milli,
            config.audio_stream_index,
            config.default_audio_stream_index,
        )
    });
    match result {
        Ok(Some(result)) => 10.0_f64.powf(result.gain_db as f64 / 20.0),
        Ok(None) => 1.0,
        Err(error) => {
            crate::logger::log(format!("remote-stream Norm lookup failed: {error}"));
            1.0
        }
    }
}

#[cfg(test)]
pub(crate) fn stream_and_gain_for_start_inputs_for_test(
    path: std::path::PathBuf,
    inputs: crate::video::RemoteStreamStartInputs,
    db_path: std::path::PathBuf,
) -> (usize, f64) {
    let config = GenerationConfig {
        generation: StreamingGeneration(1),
        path,
        encoder: EncoderPreference::Auto,
        quality: QualityPreset::Standard,
        source_origin_secs: inputs.source_origin_secs,
        segment_capacity: 4,
        hw_decode: false,
        audio_stream_index: inputs.audio_stream_index,
        default_audio_stream_index: inputs.default_audio_stream_index,
        normalize_snapshot: RemoteNormalizeSnapshot {
            enabled: true,
            target_lufs_milli: -14000,
            db_path,
        },
        audio_processing: ClocklessAudioProcessing::without_vst3(1.0),
    };
    (
        generation_transcode_options(&config).audio_stream_index,
        remote_generation_normalize_gain(&config),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::stream::audio_encoder::open_aac_encoder;
    use crate::video::stream::timeline::StreamTimeline;
    use mimageviewer_ipc::{
        SessionAcquireRequest, SessionConnectionKind, SessionPeerInfo, SessionStatus,
    };

    struct MarkDroppedOnDrop(Arc<AtomicBool>);

    fn stream_inputs(has_video: bool, has_audio: bool) -> crate::video::RemoteStreamStartInputs {
        crate::video::RemoteStreamStartInputs {
            duration_secs: 60.0,
            has_video,
            has_audio,
            source_origin_secs: 0.0,
            audio_stream_index: 1,
            default_audio_stream_index: Some(1),
        }
    }

    #[test]
    fn remote_generation_stream_and_norm_use_the_same_index() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("normalize.db");
        let db = crate::audio_normalize_db::AudioNormalizeDb::open_at(&db_path).unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/audio-tracks/multi.mkv");
        let result = |gain_db| crate::video::normalize_types::NormalizeResult {
            gain_db,
            integrated_lufs: -20.0,
            true_peak_db: -5.0,
            target_lufs_milli: -14000,
        };
        db.upsert(&path, 2, &result(6.0)).unwrap();
        db.upsert(&path, 3, &result(-6.0)).unwrap();
        let mut config = GenerationConfig {
            generation: StreamingGeneration(1),
            path,
            encoder: EncoderPreference::Auto,
            quality: QualityPreset::Standard,
            source_origin_secs: 0.0,
            segment_capacity: 4,
            hw_decode: false,
            audio_stream_index: 2,
            default_audio_stream_index: Some(2),
            normalize_snapshot: RemoteNormalizeSnapshot {
                enabled: true,
                target_lufs_milli: -14000,
                db_path,
            },
            audio_processing: ClocklessAudioProcessing::without_vst3(1.0),
        };
        assert_eq!(generation_transcode_options(&config).audio_stream_index, 2);
        assert!(
            (remote_generation_normalize_gain(&config) - 10.0_f64.powf(6.0 / 20.0)).abs() < 1e-6
        );
        config.generation = StreamingGeneration(2);
        config.audio_stream_index = 3;
        assert_eq!(generation_transcode_options(&config).audio_stream_index, 3);
        assert!(
            (remote_generation_normalize_gain(&config) - 10.0_f64.powf(-6.0 / 20.0)).abs() < 1e-6
        );
        config.normalize_snapshot.enabled = false;
        assert_eq!(remote_generation_normalize_gain(&config), 1.0);
    }

    #[test]
    fn remote_audio_selection_is_confirmed_only_by_matching_ready_generation_and_stream() {
        let pending = RemoteAudioSelection::Requested {
            generation: StreamingGeneration(2),
            stream_index: 3,
        };
        let ready = |audio_stream_index| StreamReadyInfo {
            video: None,
            audio_stream_index,
            audio_bitrate_bps: 96_000,
            codecs: "mp4a.40.2".to_owned(),
        };
        assert_eq!(
            confirmed_audio_choice(pending, StreamingGeneration(2), None),
            None
        );
        assert_eq!(
            confirmed_audio_choice(pending, StreamingGeneration(3), Some(&ready(Some(3)))),
            None
        );
        assert_eq!(
            confirmed_audio_choice(pending, StreamingGeneration(2), Some(&ready(Some(2)))),
            None
        );
        assert_eq!(
            confirmed_audio_choice(pending, StreamingGeneration(2), Some(&ready(Some(3)))),
            Some(3)
        );
        assert_eq!(
            confirmed_audio_choice(
                RemoteAudioSelection::Settled(3),
                StreamingGeneration(2),
                Some(&ready(Some(3)))
            ),
            None
        );
    }

    #[test]
    fn retirement_includes_ready_published_while_it_waits_for_the_status_lock() {
        let status = new_generation_status();
        let cancel = Arc::new(AtomicBool::new(false));
        let info = StreamReadyInfo {
            video: None,
            audio_stream_index: Some(3),
            audio_bitrate_bps: 96_000,
            codecs: "mp4a.40.2".to_owned(),
        };
        assert_eq!(generation_confirmation_snapshot(&status).1, None);
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (publish_tx, publish_rx) = std::sync::mpsc::channel();
        let worker_status = Arc::clone(&status);
        let worker_info = info.clone();
        let worker = std::thread::spawn(move || {
            let mut state = worker_status
                .0
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            locked_tx.send(()).unwrap();
            publish_rx.recv().unwrap();
            assert!(publish_generation_status(
                &mut state,
                StreamGenerationStatus::Ready(worker_info)
            ));
            worker_status.1.notify_all();
        });
        locked_rx.recv().unwrap();
        let retire_status = Arc::clone(&status);
        let retire_cancel = Arc::clone(&cancel);
        let retirement =
            std::thread::spawn(move || retire_generation_status(&retire_status, &retire_cancel));
        publish_tx.send(()).unwrap();
        worker.join().unwrap();
        let confirmed_ready = retirement.join().unwrap();
        assert_eq!(confirmed_ready, Some(info.clone()));
        assert!(cancel.load(Ordering::Acquire));
        assert_eq!(generation_status(&status), StreamGenerationStatus::Stopped);
        assert_eq!(
            confirmed_audio_choice(
                RemoteAudioSelection::Requested {
                    generation: StreamingGeneration(2),
                    stream_index: 3,
                },
                StreamingGeneration(2),
                confirmed_ready.as_ref(),
            ),
            Some(3)
        );

        // A Ready callback queued after retirement cannot confirm the replacement.
        set_generation_status(&status, StreamGenerationStatus::Ready(info.clone()));
        assert_eq!(generation_confirmation_snapshot(&status).1, None);
        let owner_cancelled = new_generation_status();
        set_generation_status(
            &owner_cancelled,
            StreamGenerationStatus::Ready(info.clone()),
        );
        set_generation_status(&owner_cancelled, StreamGenerationStatus::Stopped);
        assert_eq!(
            retire_generation_status(&owner_cancelled, &AtomicBool::new(true)),
            Some(info)
        );
        let failed = new_generation_status();
        set_generation_status(
            &failed,
            StreamGenerationStatus::Ready(StreamReadyInfo {
                video: None,
                audio_stream_index: Some(3),
                audio_bitrate_bps: 96_000,
                codecs: "mp4a.40.2".to_owned(),
            }),
        );
        set_generation_status(
            &failed,
            StreamGenerationStatus::Failed("encoder failed".to_owned()),
        );
        assert_eq!(
            retire_generation_status(&failed, &AtomicBool::new(false)),
            None
        );
    }

    #[test]
    fn remote_start_uses_headless_opened_track_unless_valid_request_overrides_it() {
        let tracks = [
            RemoteAudioTrack {
                stream_index: 2,
                label: "日本語".to_owned(),
                is_default: false,
            },
            RemoteAudioTrack {
                stream_index: 3,
                label: "English".to_owned(),
                is_default: true,
            },
        ];
        // The opened index comes from demux even when there is no PC audio output lane.
        assert_eq!(resolve_remote_audio_start(&tracks, 2, None), (2, false));
        assert_eq!(resolve_remote_audio_start(&tracks, 2, Some(3)), (3, true));
        assert_eq!(resolve_remote_audio_start(&tracks, 2, Some(99)), (2, false));
    }

    #[test]
    fn audio_track_change_starts_new_generation_and_restores_index_if_start_fails() {
        let handle = crate::remote_ipc::session::SessionHandle::new();
        let response = handle.acquire(SessionAcquireRequest {
            client_id: "audio-track-test".to_owned(),
            peer: SessionPeerInfo {
                connection_kind: SessionConnectionKind::Direct,
                device_name: None,
            },
        });
        assert_eq!(response.status, SessionStatus::Active);
        assert!(handle.finish_acquire(handle.snapshot().generation));
        let identity = handle.owner_for_test("audio-track-test");
        let owner = handle.streaming_owner(&identity).unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/audio-tracks/multi-audio.m4a");
        let normalize_snapshot = RemoteNormalizeSnapshot {
            enabled: false,
            target_lufs_milli: -14000,
            db_path: path.with_extension("unused-db"),
        };
        let audio_processing = ClocklessAudioProcessing::without_vst3(1.0);
        let current = StreamingGenerationHandle::start(
            StreamingGeneration(1),
            &owner,
            path.clone(),
            0.0,
            EncoderPreference::Auto,
            QualityPreset::Standard,
            4,
            false,
            0,
            Some(1),
            normalize_snapshot.clone(),
            audio_processing.clone(),
        )
        .unwrap();
        let mut session = RemoteVideoStreamingSession {
            id: StreamingSessionId(1),
            owner,
            source_path: path,
            encoder: EncoderPreference::Auto,
            quality: QualityPreset::Standard,
            segment_capacity: 4,
            hw_decode: false,
            audio_selection: RemoteAudioSelection::Settled(0),
            audio_tracks: vec![
                RemoteAudioTrack {
                    stream_index: 0,
                    label: "1".to_owned(),
                    is_default: false,
                },
                RemoteAudioTrack {
                    stream_index: 1,
                    label: "2".to_owned(),
                    is_default: true,
                },
            ],
            default_audio_stream_index: Some(1),
            normalize_snapshot,
            audio_processing,
            next_generation: 2,
            current,
        };
        assert_eq!(
            session.change_audio_track(1, 0.0).unwrap(),
            GenerationChange {
                generation: StreamingGeneration(2),
                confirmed_audio_choice: None,
            }
        );
        assert_eq!(session.generation(), StreamingGeneration(2));
        assert_eq!(session.audio_selection.stream_index(), 1);
        assert_eq!(
            session.audio_selection,
            RemoteAudioSelection::Requested {
                generation: StreamingGeneration(2),
                stream_index: 1,
            }
        );
        session.segment_capacity = 0; // output allocation rejects before replacement
        assert!(session.change_audio_track(0, 0.0).is_err());
        assert_eq!(session.generation(), StreamingGeneration(2));
        assert_eq!(session.audio_selection.stream_index(), 1);
        assert_eq!(
            session.audio_selection,
            RemoteAudioSelection::Requested {
                generation: StreamingGeneration(2),
                stream_index: 1,
            }
        );
    }

    #[test]
    fn audio_only_and_av_use_the_same_session_track_gate() {
        assert!(validate_remote_stream_tracks(stream_inputs(true, true)).is_ok());
        assert!(validate_remote_stream_tracks(stream_inputs(false, true)).is_ok());
        assert!(validate_remote_stream_tracks(stream_inputs(false, false)).is_err());
        assert!(validate_remote_stream_tracks(stream_inputs(true, false)).is_err());
    }

    impl Drop for MarkDroppedOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[test]
    fn consecutive_generations_wait_until_previous_gpu_resources_are_dropped() {
        let first_cancel = Arc::new(AtomicBool::new(false));
        let second_cancel = Arc::new(AtomicBool::new(false));
        let first_dropped = Arc::new(AtomicBool::new(false));
        let (first_entered_tx, first_entered_rx) = std::sync::mpsc::channel();
        let (release_first_tx, release_first_rx) = std::sync::mpsc::channel();
        let first_cancel_worker = Arc::clone(&first_cancel);
        let first_dropped_worker = Arc::clone(&first_dropped);
        let first = std::thread::spawn(move || {
            run_with_generation_resource_lease(&first_cancel_worker, || {
                let _resource = MarkDroppedOnDrop(first_dropped_worker);
                first_entered_tx.send(()).unwrap();
                release_first_rx.recv().unwrap();
                Ok(())
            })
        });
        first_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();

        let (second_attempted_tx, second_attempted_rx) = std::sync::mpsc::channel();
        let (second_entered_tx, second_entered_rx) = std::sync::mpsc::channel();
        let second_cancel_worker = Arc::clone(&second_cancel);
        let first_dropped_for_second = Arc::clone(&first_dropped);
        let second = std::thread::spawn(move || {
            second_attempted_tx.send(()).unwrap();
            run_with_generation_resource_lease(&second_cancel_worker, || {
                assert!(
                    first_dropped_for_second.load(Ordering::Acquire),
                    "replacement allocated before the prior generation resource was dropped"
                );
                second_entered_tx.send(()).unwrap();
                Ok(())
            })
        });
        second_attempted_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert!(
            second_entered_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "replacement entered the GPU resource lifetime concurrently"
        );

        release_first_tx.send(()).unwrap();
        second_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
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
    fn worker_completion_does_not_end_the_generation_ownership_lifetime() {
        let cancel = Arc::new(AtomicBool::new(false));
        let status = new_generation_status();
        let output = ClocklessStreamOutput::new(30, 0.0).unwrap();

        publish_generation_worker_completion(
            Err("video tap disconnected".to_owned()),
            &cancel,
            &status,
            &output,
        );

        assert!(!cancel.load(Ordering::Acquire));
        assert_eq!(
            generation_status(&status),
            StreamGenerationStatus::Failed("video tap disconnected".to_owned())
        );
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
