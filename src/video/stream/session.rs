use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::encoder::{EncoderPreference, H264EncoderKind, SEGMENT_DURATION_SECS};
use super::quality::{OutputDimensions, QualityPreset};
use crate::remote_ipc::session::{RemoteSessionOwner, RemoteStreamingActivity};
use crate::video::clockless_transcode::{
    ClocklessAudioProcessing, ClocklessOutputInfo, ClocklessSegmentBytes, ClocklessStreamOutput,
    ClocklessTranscodeControl, ClocklessTranscodeOptions, ClocklessVstStatus,
    ClocklessVstStatusSnapshot, run_clockless_stream,
};

const RESOURCE_TIMEOUT: Duration = Duration::from_secs(2);

static NEXT_STREAMING_SESSION_ID: AtomicU64 = AtomicU64::new(1);
static GENERATION_RESOURCE_GATE: OnceLock<Mutex<()>> = OnceLock::new();

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

type SharedGenerationStatus = Arc<(Mutex<StreamGenerationStatus>, Condvar)>;

struct GenerationConfig {
    generation: StreamingGeneration,
    path: PathBuf,
    encoder: EncoderPreference,
    quality: QualityPreset,
    source_origin_secs: f64,
    segment_capacity: usize,
    hw_decode: bool,
    audio_processing: ClocklessAudioProcessing,
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
        encoder: info.encoder,
        output_dimensions: info.output_dimensions,
        video_bitrate_bps: info.video_bitrate_bps,
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
        audio_processing: ClocklessAudioProcessing,
    ) -> Result<Self, String> {
        let registration = owner
            .register_streaming()
            .map_err(|response| response.message)?;
        let activity = registration.activity();
        let cancel = registration.cancel_flag();
        let config = GenerationConfig {
            generation,
            path: source_path,
            encoder,
            quality,
            source_origin_secs,
            segment_capacity,
            hw_decode,
            audio_processing,
        };
        let audio_status = config.audio_processing.vst3_status();
        let output = ClocklessStreamOutput::new(segment_capacity, source_origin_secs)?;
        let control = ClocklessTranscodeControl::manual(segment_capacity)?;
        control.bind_cancel_flag(Arc::clone(&cancel));
        let status = Arc::new((Mutex::new(StreamGenerationStatus::Opening), Condvar::new()));
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
                // The remote-session drain owns the actual FFmpeg/GPU worker lifetime, not just
                // the UI handle lifetime. Releasing this registration is what may return control
                // to the PC or allow the next remote owner to acquire the session.
                drop(registration);
            })
            .map_err(|error| format!("failed to spawn streaming worker: {error}"))?;
        Ok(Self {
            generation,
            status,
            output,
            control,
            activity,
            audio_status,
            cancel,
            worker: Some(worker),
        })
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
            output: self.output.clone(),
            control: self.control.clone(),
            activity: self.activity.clone(),
            audio_status: self.audio_status.clone(),
        }
    }

    pub(crate) fn stop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.control.cancel();
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
    hw_decode: bool,
    audio_processing: ClocklessAudioProcessing,
    next_generation: u64,
    current: StreamingGenerationHandle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StreamReconcile {
    Active,
    Stop(String),
}

impl RemoteVideoStreamingSession {
    pub(crate) fn start(
        owner: RemoteSessionOwner,
        player: &crate::video::VideoPlayer,
        inputs: crate::video::RemoteStreamStartInputs,
        encoder: EncoderPreference,
        quality: QualityPreset,
        segment_capacity: usize,
        hw_decode: bool,
        audio_processing: ClocklessAudioProcessing,
    ) -> Result<Self, String> {
        if segment_capacity == 0 {
            return Err("remote streaming segment capacity must be non-zero".to_owned());
        }
        if !inputs.has_video || !inputs.has_audio {
            return Err("remote streaming requires both video and audio streams".to_owned());
        }
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

    pub(crate) fn access(&self) -> StreamingGenerationAccess {
        self.current.access()
    }

    pub(crate) fn buffer_target_secs(&self) -> f64 {
        self.segment_capacity as f64 * f64::from(SEGMENT_DURATION_SECS)
    }

    pub(crate) fn set_playing(&self, playing: bool) -> bool {
        self.current.set_playing(playing)
    }

    pub(crate) fn change_quality(
        &mut self,
        quality: QualityPreset,
        position_secs: f64,
    ) -> Result<StreamingGeneration, String> {
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

    pub(crate) fn seek(&mut self, position_secs: f64) -> Result<StreamingGeneration, String> {
        if !position_secs.is_finite() || position_secs < 0.0 {
            return Err("stream seek position must be finite and non-negative".to_owned());
        }
        self.start_new_generation(position_secs)
    }

    fn start_new_generation(
        &mut self,
        source_origin_secs: f64,
    ) -> Result<StreamingGeneration, String> {
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
            self.audio_processing.clone(),
        )?;
        self.next_generation = self.next_generation.saturating_add(1);
        let mut previous = std::mem::replace(&mut self.current, replacement);
        previous.stop();
        Ok(generation)
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

fn run_generation_worker(
    config: GenerationConfig,
    control: &ClocklessTranscodeControl,
    output: ClocklessStreamOutput,
    on_ready: impl FnOnce(ClocklessOutputInfo),
) -> Result<(), String> {
    let quality = match config.quality {
        QualityPreset::Minimum => crate::video::clockless_transcode::ClocklessQuality::Minimum,
        QualityPreset::Low => crate::video::clockless_transcode::ClocklessQuality::Low,
        QualityPreset::Standard => crate::video::clockless_transcode::ClocklessQuality::Standard,
        QualityPreset::High => crate::video::clockless_transcode::ClocklessQuality::High,
    };
    let options = ClocklessTranscodeOptions {
        path: config.path,
        include_audio: true,
        hw_decode: config.hw_decode,
        quality,
        encoder: config.encoder,
        max_source_secs: None,
        segment_capacity: config.segment_capacity,
        profile_swscale: false,
        source_origin_secs: config.source_origin_secs,
        diagnostic_generation: Some(config.generation.0),
    };
    run_clockless_stream(&options, control, output, config.audio_processing, on_ready).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::stream::audio_encoder::open_aac_encoder;
    use crate::video::stream::timeline::StreamTimeline;

    struct MarkDroppedOnDrop(Arc<AtomicBool>);

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
        let status = Arc::new((Mutex::new(StreamGenerationStatus::Opening), Condvar::new()));
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
