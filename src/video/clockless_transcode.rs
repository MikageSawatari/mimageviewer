//! Clock-free remote-video transcode driver.
//!
//! This path owns a separate demuxer/decoder and never consults `VideoPlayer`, the presentation
//! clock, or the audio device. It is intentionally not wired to `VideoStreamSession` yet. The
//! development benchmark is its only caller in this increment.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use crossbeam_channel::Receiver;
use ffmpeg::format::Pixel;
use ffmpeg::format::sample::{Sample, Type as SampleType};
use ffmpeg::media::Type as MediaType;
use ffmpeg::software::scaling::{Context as ScaleContext, Flags as ScaleFlags};
use ffmpeg::util::frame::{Audio, Video};
use ffmpeg_the_third as ffmpeg;

use super::audio::ProcessedChunk;
use super::stream::audio_encoder::{OpenedAacEncoder, open_aac_encoder};
use super::stream::encoder::{EncoderPreference, FrameRate, H264InputFormat};
use super::stream::quality::{OutputDimensions, QualityPreset};
use super::stream::segmenter::Fmp4Segmenter;
use super::stream::timeline::StreamTimeline;
use super::stream::video_tap::{
    TappedVideoFrame, VideoStreamEncoder, VideoTapProducer, open_video_stream_encoder,
    video_tap_channel,
};

const AUDIO_OUTPUT_RATE: u32 = 48_000;
const SEEK_SERIAL: u64 = 0;
const FFMPEG_EAGAIN: i32 = 11;

/// Public preset surface for the standalone driver. Runtime streaming settings are deliberately
/// not read here, so a benchmark cannot mutate or depend on the running application's profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClocklessQuality {
    Minimum,
    Low,
    #[default]
    Standard,
    High,
}

impl ClocklessQuality {
    fn internal(self) -> QualityPreset {
        match self {
            Self::Minimum => QualityPreset::Minimum,
            Self::Low => QualityPreset::Low,
            Self::Standard => QualityPreset::Standard,
            Self::High => QualityPreset::High,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClocklessTranscodeOptions {
    pub path: PathBuf,
    pub include_audio: bool,
    pub hw_decode: bool,
    pub quality: ClocklessQuality,
    /// `None` decodes to EOF. The limit is measured from the selected streams' earliest start.
    pub max_source_secs: Option<f64>,
    pub segment_capacity: usize,
    /// Run one extra swscale operation per video frame for stage attribution. This intentionally
    /// reduces throughput and must be false for the headline real-time multiple.
    pub profile_swscale: bool,
}

impl ClocklessTranscodeOptions {
    pub fn benchmark(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            include_audio: true,
            hw_decode: true,
            quality: ClocklessQuality::Standard,
            max_source_secs: Some(30.0),
            segment_capacity: 30,
            profile_swscale: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClocklessStageTimes {
    pub demux_secs: f64,
    pub video_decode_secs: f64,
    pub video_download_secs: f64,
    /// Existing `VideoStreamEncoder::encode_frame`, including its real swscale and encoder calls.
    pub video_scale_encode_secs: f64,
    pub profiled_swscale_secs: f64,
    pub audio_decode_resample_secs: f64,
    pub audio_encode_secs: f64,
    pub mux_secs: f64,
}

#[derive(Clone, Debug)]
pub struct ClocklessTranscodeReport {
    pub source_path: PathBuf,
    pub source_codec: String,
    pub source_width: u32,
    pub source_height: u32,
    pub frame_rate_num: u32,
    pub frame_rate_den: u32,
    pub decoder_name: String,
    pub hardware_decode_active: bool,
    pub encoder: String,
    pub output_width: u32,
    pub output_height: u32,
    pub include_audio: bool,
    pub audio_codec: Option<String>,
    pub source_secs_processed: f64,
    pub wall_secs: f64,
    pub realtime_multiple: f64,
    pub input_packets: u64,
    pub video_frames: u64,
    pub audio_frames: u64,
    pub completed_segments: u64,
    pub scale_profile_samples: u64,
    pub times: ClocklessStageTimes,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClocklessAheadSnapshot {
    pub produced_segments: u64,
    pub released_segments: u64,
    pub waiting_for_capacity: bool,
    pub cancelled: bool,
}

#[derive(Debug)]
struct AheadState {
    produced_segments: u64,
    released_segments: u64,
    waiting_for_capacity: bool,
}

#[derive(Debug)]
struct ClocklessControlInner {
    capacity: u64,
    auto_release: bool,
    cancel: AtomicBool,
    state: Mutex<AheadState>,
    wake: Condvar,
}

/// Consumer-facing control for the standalone worker. Stage 1 uses `auto_releasing` for
/// throughput measurement; the manual form fixes the future ring ownership contract now.
#[derive(Clone, Debug)]
pub struct ClocklessTranscodeControl {
    inner: Arc<ClocklessControlInner>,
}

impl ClocklessTranscodeControl {
    pub fn manual(segment_capacity: usize) -> Result<Self, String> {
        Self::new(segment_capacity, false)
    }

    pub fn auto_releasing(segment_capacity: usize) -> Result<Self, String> {
        Self::new(segment_capacity, true)
    }

    fn new(segment_capacity: usize, auto_release: bool) -> Result<Self, String> {
        let capacity = u64::try_from(segment_capacity)
            .map_err(|_| "segment capacity exceeds u64".to_owned())?;
        if capacity == 0 {
            return Err("segment capacity must be non-zero".to_owned());
        }
        Ok(Self {
            inner: Arc::new(ClocklessControlInner {
                capacity,
                auto_release,
                cancel: AtomicBool::new(false),
                state: Mutex::new(AheadState {
                    produced_segments: 0,
                    released_segments: 0,
                    waiting_for_capacity: false,
                }),
                wake: Condvar::new(),
            }),
        })
    }

    pub fn cancel(&self) {
        self.inner.cancel.store(true, Ordering::Release);
        self.inner.wake.notify_all();
    }

    /// Release every segment through `sequence` (inclusive). Stale or future acknowledgements
    /// are clamped, so consumer retry/reordering cannot manufacture ring capacity.
    pub fn release_through(&self, sequence: u64) {
        let mut state = self.inner.state.lock().unwrap();
        let requested = sequence.saturating_add(1).min(state.produced_segments);
        if requested > state.released_segments {
            state.released_segments = requested;
            state.waiting_for_capacity = false;
            self.inner.wake.notify_all();
        }
    }

    pub fn snapshot(&self) -> ClocklessAheadSnapshot {
        let state = self.inner.state.lock().unwrap();
        ClocklessAheadSnapshot {
            produced_segments: state.produced_segments,
            released_segments: state.released_segments,
            waiting_for_capacity: state.waiting_for_capacity,
            cancelled: self.inner.cancel.load(Ordering::Acquire),
        }
    }

    fn checkpoint(&self) -> Result<(), ClocklessStop> {
        if self.inner.cancel.load(Ordering::Acquire) {
            return Err(ClocklessStop::Cancelled);
        }
        Ok(())
    }

    fn wait_for_capacity(&self) -> Result<(), ClocklessStop> {
        let mut state = self.inner.state.lock().unwrap();
        loop {
            if self.inner.cancel.load(Ordering::Acquire) {
                state.waiting_for_capacity = false;
                return Err(ClocklessStop::Cancelled);
            }
            let ahead = state
                .produced_segments
                .saturating_sub(state.released_segments);
            if ahead < self.inner.capacity {
                state.waiting_for_capacity = false;
                return Ok(());
            }
            state.waiting_for_capacity = true;
            state = self.inner.wake.wait(state).unwrap();
        }
    }

    fn record_produced(&self, produced_segments: u64) {
        let mut state = self.inner.state.lock().unwrap();
        state.produced_segments = state.produced_segments.max(produced_segments);
        if self.inner.auto_release {
            state.released_segments = state.produced_segments;
        }
    }

    #[cfg(test)]
    fn run_stage<T>(&self, stage: impl FnOnce() -> T) -> Result<T, ClocklessStop> {
        self.checkpoint()?;
        let value = stage();
        self.checkpoint()?;
        Ok(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClocklessStop {
    Cancelled,
}

struct TimedPacket {
    dts_secs: f64,
    end_secs: f64,
    packet: ffmpeg::Packet,
}

#[derive(Default)]
struct ClocklessMux {
    video: VecDeque<TimedPacket>,
    audio: VecDeque<TimedPacket>,
    latest_video_dts_secs: Option<f64>,
    latest_audio_end_secs: Option<f64>,
}

impl ClocklessMux {
    fn enqueue_video(&mut self, packet: ffmpeg::Packet, rate: FrameRate) -> Result<(), String> {
        let timed = timed_packet(
            packet,
            f64::from(rate.denominator) / f64::from(rate.numerator),
            "video",
        )?;
        self.latest_video_dts_secs = Some(timed.dts_secs);
        self.video.push_back(timed);
        Ok(())
    }

    fn enqueue_audio(&mut self, packet: ffmpeg::Packet, sample_rate: u32) -> Result<(), String> {
        let timed = timed_packet(packet, 1.0 / f64::from(sample_rate), "audio")?;
        self.latest_audio_end_secs = Some(timed.end_secs);
        self.audio.push_back(timed);
        Ok(())
    }

    fn drain_ready(&mut self, segmenter: &mut Fmp4Segmenter) -> Result<Option<u64>, String> {
        let Some(watermark) = self
            .latest_video_dts_secs
            .zip(self.latest_audio_end_secs)
            .map(|(video, audio)| video.min(audio))
        else {
            return Ok(None);
        };
        self.drain_while(segmenter, |dts| dts <= watermark)
    }

    fn drain_while(
        &mut self,
        segmenter: &mut Fmp4Segmenter,
        ready: impl Fn(f64) -> bool,
    ) -> Result<Option<u64>, String> {
        let mut last_completed = None;
        loop {
            let video_dts = self.video.front().map(|packet| packet.dts_secs);
            let audio_dts = self.audio.front().map(|packet| packet.dts_secs);
            let take_audio = match (video_dts, audio_dts) {
                (Some(video), Some(audio)) => audio <= video && ready(audio),
                (None, Some(audio)) => ready(audio),
                _ => false,
            };
            if take_audio {
                let packet = self.audio.pop_front().expect("audio front checked");
                segmenter
                    .push_audio_packet(&packet.packet)
                    .map_err(|error| error.to_string())?;
            } else if video_dts.is_some_and(&ready) {
                let packet = self.video.pop_front().expect("video front checked");
                if let Some(sequence) = segmenter
                    .push_packet(&packet.packet)
                    .map_err(|error| error.to_string())?
                {
                    last_completed = Some(sequence);
                }
            } else {
                return Ok(last_completed);
            }
        }
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
    Ok(TimedPacket {
        dts_secs: dts as f64 * seconds_per_tick,
        end_secs: dts.saturating_add(duration) as f64 * seconds_per_tick,
        packet,
    })
}

struct AudioPath {
    decoder: ffmpeg::decoder::Audio,
    resampler: ffmpeg::software::resampling::Context,
    time_base_secs: f64,
    input_rate: u32,
    next_pts_secs: Option<f64>,
    codec_name: String,
}

impl AudioPath {
    fn open(stream: ffmpeg::Stream<'_>) -> Result<Self, String> {
        let time_base = stream.time_base();
        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|error| format!("audio decoder context: {error}"))?;
        let codec_name = context.id().name().to_owned();
        let mut decoder = context
            .decoder()
            .audio()
            .map_err(|error| format!("audio decoder open: {error}"))?;
        let input_rate = decoder.rate();
        let input_layout = normalized_layout(decoder.ch_layout());
        decoder.set_ch_layout(input_layout.clone());
        let resampler = ffmpeg::software::resampling::Context::get2(
            decoder.format(),
            input_layout,
            input_rate,
            Sample::F32(SampleType::Packed),
            ffmpeg::ChannelLayout::STEREO,
            AUDIO_OUTPUT_RATE,
        )
        .map_err(|error| format!("audio resampler open: {error}"))?;
        Ok(Self {
            decoder,
            resampler,
            time_base_secs: f64::from(time_base.numerator()) / f64::from(time_base.denominator()),
            input_rate,
            next_pts_secs: None,
            codec_name,
        })
    }

    fn frame_to_chunk(&mut self, frame: &mut Audio) -> Result<Option<ProcessedChunk>, String> {
        let pts = self.next_pts_secs.unwrap_or_else(|| {
            frame
                .pts()
                .map(|pts| pts as f64 * self.time_base_secs)
                .unwrap_or(0.0)
        });
        let delay = self
            .resampler
            .delay()
            .map(|delay| delay.output.max(0) as u64)
            .unwrap_or(0);
        let output_samples = resample_output_capacity(
            frame.samples() as u64,
            self.input_rate,
            AUDIO_OUTPUT_RATE,
            delay,
        );
        if output_samples == 0 {
            return Ok(None);
        }
        let mut output = Audio::empty();
        unsafe {
            output.alloc(
                Sample::F32(SampleType::Packed),
                output_samples,
                ffmpeg::ChannelLayoutMask::STEREO,
            );
            output.set_rate(AUDIO_OUTPUT_RATE);
        }
        self.resampler
            .run(frame, &mut output)
            .map_err(|error| format!("audio resample: {error}"))?;
        self.output_to_chunk(output, pts)
    }

    fn output_to_chunk(
        &mut self,
        output: Audio,
        pts: f64,
    ) -> Result<Option<ProcessedChunk>, String> {
        let frames = output.samples();
        if frames == 0 {
            return Ok(None);
        }
        let element_count = frames
            .checked_mul(2)
            .ok_or_else(|| "audio output sample count overflow".to_owned())?;
        let samples = unsafe {
            let ptr = (*output.as_ptr()).data[0] as *const f32;
            if ptr.is_null() {
                return Err("audio resampler returned a null plane".to_owned());
            }
            std::slice::from_raw_parts(ptr, element_count).to_vec()
        };
        let duration_secs = frames as f64 / f64::from(AUDIO_OUTPUT_RATE);
        self.next_pts_secs = Some(pts + duration_secs);
        Ok(Some(ProcessedChunk {
            samples,
            audible_pts_secs: pts,
            duration_secs,
            source_secs_per_output_sec: 1.0,
            seek_serial: SEEK_SERIAL,
            pdc_latency_secs_at_process: 0.0,
        }))
    }
}

fn normalized_layout(layout: ffmpeg::ChannelLayout<'_>) -> ffmpeg::ChannelLayout<'static> {
    if layout.mask().is_some() {
        return ffmpeg::ChannelLayout::from(layout.into_owned());
    }
    let channels = layout.channels();
    let default = ffmpeg::ChannelLayout::default_for_channels(channels);
    if default.mask().is_some() {
        default
    } else if channels >= 2 {
        ffmpeg::ChannelLayout::STEREO
    } else {
        ffmpeg::ChannelLayout::MONO
    }
}

fn resample_output_capacity(
    input_samples: u64,
    input_rate: u32,
    output_rate: u32,
    delay_output_samples: u64,
) -> usize {
    if input_rate == 0 || output_rate == 0 {
        return 0;
    }
    let scaled = input_samples
        .saturating_mul(u64::from(output_rate))
        .saturating_add(u64::from(input_rate) - 1)
        / u64::from(input_rate);
    usize::try_from(
        scaled
            .saturating_add(delay_output_samples)
            .saturating_add(32),
    )
    .unwrap_or(usize::MAX)
}

struct ScaleProfiler {
    context: ScaleContext,
    output_format: Pixel,
    output_dimensions: OutputDimensions,
}

impl ScaleProfiler {
    fn new(
        input: &Video,
        output_format: H264InputFormat,
        output_dimensions: OutputDimensions,
    ) -> Result<Self, String> {
        let context = ScaleContext::get(
            input.format(),
            input.width(),
            input.height(),
            output_format.ffmpeg_pixel(),
            output_dimensions.width,
            output_dimensions.height,
            ScaleFlags::BILINEAR,
        )
        .map_err(|error| format!("profile swscale init: {error}"))?;
        Ok(Self {
            context,
            output_format: output_format.ffmpeg_pixel(),
            output_dimensions,
        })
    }

    fn run(&mut self, input: &Video) -> Result<(), String> {
        let mut output = Video::new(
            self.output_format,
            self.output_dimensions.width,
            self.output_dimensions.height,
        );
        self.context
            .run(input, &mut output)
            .map_err(|error| format!("profile swscale: {error}"))
    }
}

struct DriverState<'a> {
    options: &'a ClocklessTranscodeOptions,
    control: &'a ClocklessTranscodeControl,
    frame_rate: FrameRate,
    video: VideoStreamEncoder,
    video_tap: VideoTapProducer,
    video_rx: Receiver<TappedVideoFrame>,
    audio_encoder: OpenedAacEncoder,
    segmenter: Fmp4Segmenter,
    mux: ClocklessMux,
    scale_profiler: Option<ScaleProfiler>,
    times: ClocklessStageTimes,
    packets: u64,
    video_frames: u64,
    audio_frames: u64,
    scale_profile_samples: u64,
    completed_segments: u64,
    max_source_pts: f64,
}

impl DriverState<'_> {
    fn checkpoint(&self) -> Result<(), String> {
        self.control
            .checkpoint()
            .map_err(|_| "clockless transcode cancelled".to_owned())
    }

    fn wait_for_capacity(&self) -> Result<(), String> {
        self.control
            .wait_for_capacity()
            .map_err(|_| "clockless transcode cancelled".to_owned())
    }

    fn observe_segments(&self) {
        self.control.record_produced(self.completed_segments);
    }

    fn push_video_frame(&mut self, frame: &Video, pts_secs: f64) -> Result<(), String> {
        self.checkpoint()?;
        let started = Instant::now();
        self.video_tap.try_publish(frame, pts_secs, SEEK_SERIAL);
        self.times.video_download_secs += started.elapsed().as_secs_f64();
        let tapped = self
            .video_rx
            .try_recv()
            .map_err(|error| format!("independent video tap did not publish a frame: {error}"))?;
        if self.options.profile_swscale {
            if self.scale_profiler.is_none() {
                self.scale_profiler = Some(ScaleProfiler::new(
                    tapped.as_video(),
                    self.video.input_format(),
                    self.video.output_parameters().dimensions,
                )?);
            }
            let started = Instant::now();
            self.scale_profiler
                .as_mut()
                .expect("initialized above")
                .run(tapped.as_video())?;
            self.times.profiled_swscale_secs += started.elapsed().as_secs_f64();
            self.scale_profile_samples = self.scale_profile_samples.saturating_add(1);
        }
        self.checkpoint()?;
        let started = Instant::now();
        let packets = self
            .video
            .encode_frame(tapped, &self.segmenter)
            .map_err(|error| error.to_string())?;
        self.times.video_scale_encode_secs += started.elapsed().as_secs_f64();
        for packet in packets {
            self.mux.enqueue_video(packet, self.frame_rate)?;
        }
        self.video_frames = self.video_frames.saturating_add(1);
        self.max_source_pts = self.max_source_pts.max(pts_secs);
        self.drain_mux()
    }

    fn push_audio_chunk(&mut self, chunk: ProcessedChunk) -> Result<(), String> {
        self.checkpoint()?;
        let started = Instant::now();
        let packets = self
            .audio_encoder
            .push_chunk(chunk)
            .map_err(|error| error.to_string())?;
        self.times.audio_encode_secs += started.elapsed().as_secs_f64();
        for packet in packets {
            self.mux
                .enqueue_audio(packet, self.audio_encoder.output_sample_rate())?;
        }
        self.audio_frames = self.audio_frames.saturating_add(1);
        self.drain_mux()
    }

    fn drain_mux(&mut self) -> Result<(), String> {
        self.checkpoint()?;
        let started = Instant::now();
        if self.options.include_audio {
            if let Some(sequence) = self.mux.drain_ready(&mut self.segmenter)? {
                self.completed_segments = self.completed_segments.max(sequence.saturating_add(1));
            }
        } else {
            while let Some(packet) = self.mux.video.pop_front() {
                if let Some(sequence) = self
                    .segmenter
                    .push_packet(&packet.packet)
                    .map_err(|error| error.to_string())?
                {
                    self.completed_segments =
                        self.completed_segments.max(sequence.saturating_add(1));
                }
            }
        }
        self.times.mux_secs += started.elapsed().as_secs_f64();
        self.observe_segments();
        Ok(())
    }
}

/// Run a complete, clock-free transcode on the calling worker thread.
pub fn run_clockless_transcode(
    options: &ClocklessTranscodeOptions,
    control: &ClocklessTranscodeControl,
) -> Result<ClocklessTranscodeReport, String> {
    validate_options(options, control)?;
    ffmpeg::init().map_err(|error| format!("FFmpeg initialization failed: {error}"))?;
    let wall_started = Instant::now();
    let open_started = Instant::now();
    let mut input = ffmpeg::format::input(&options.path)
        .map_err(|error| format!("open input {}: {error}", options.path.display()))?;
    let times = ClocklessStageTimes {
        demux_secs: open_started.elapsed().as_secs_f64(),
        ..ClocklessStageTimes::default()
    };

    let video_stream = input
        .streams()
        .best(MediaType::Video)
        .filter(|stream| {
            !stream
                .disposition()
                .contains(ffmpeg::format::stream::Disposition::ATTACHED_PIC)
        })
        .ok_or_else(|| "input has no timed video stream".to_owned())?;
    let video_stream_index = video_stream.index();
    let video_time_base = video_stream.time_base();
    let video_start_secs = stream_start_secs(&video_stream);
    let frame_rate = selected_frame_rate(video_stream.avg_frame_rate(), video_stream.rate())?;
    let video_params = super::decoder::clone_codec_parameters(&video_stream.parameters())?;
    let source_codec = video_params.id().name().to_owned();
    let mut video_decoder = super::decoder::open_aux_video_decoder_with_fallback(
        &video_params,
        video_params.id(),
        options.hw_decode,
        "clockless-transcode",
    )?;
    let source_width = video_decoder.width();
    let source_height = video_decoder.height();
    let decoder_name = video_decoder.decoder_name().to_owned();
    let hardware_decode_active = video_decoder.hw_decode_active();

    let audio_stream = options
        .include_audio
        .then(|| input.streams().best(MediaType::Audio))
        .flatten();
    let audio_stream_index = audio_stream.as_ref().map(|stream| stream.index());
    let audio_start_secs = audio_stream.as_ref().map(stream_start_secs);
    let source_start_secs = audio_start_secs
        .into_iter()
        .chain(std::iter::once(video_start_secs))
        .fold(f64::INFINITY, f64::min);
    let source_start_secs = if source_start_secs.is_finite() {
        source_start_secs
    } else {
        0.0
    };
    let timeline = StreamTimeline::new(source_start_secs).map_err(|error| error.to_string())?;
    let mut audio_path = audio_stream.map(AudioPath::open).transpose()?;
    if options.include_audio && audio_path.is_none() {
        return Err("include_audio was requested but the input has no audio stream".to_owned());
    }
    let audio_codec = audio_path.as_ref().map(|path| path.codec_name.clone());

    let (video_tap_controller, video_tap) = video_tap_channel();
    let (_video_tap_lease, video_rx) = video_tap_controller.attach(1).map_err(str::to_owned)?;
    let video = open_video_stream_encoder(
        EncoderPreference::Auto,
        options.quality.internal(),
        source_width,
        source_height,
        frame_rate,
        SEEK_SERIAL,
        timeline,
    )
    .map_err(|error| error.to_string())?;
    let audio_encoder = open_aac_encoder(
        AUDIO_OUTPUT_RATE,
        options.quality.internal().parameters().audio_bitrate_bps,
        SEEK_SERIAL,
        timeline,
    )
    .map_err(|error| error.to_string())?;
    let segmenter = Fmp4Segmenter::with_capacity(
        video.encoder(),
        &audio_encoder.encoder,
        frame_rate,
        options.segment_capacity,
    )
    .map_err(|error| error.to_string())?;
    let encoder = video.encoder_kind().as_str().to_owned();
    let output = video.output_parameters().dimensions;
    let mut state = DriverState {
        options,
        control,
        frame_rate,
        video,
        video_tap,
        video_rx,
        audio_encoder,
        segmenter,
        mux: ClocklessMux::default(),
        scale_profiler: None,
        times,
        packets: 0,
        video_frames: 0,
        audio_frames: 0,
        scale_profile_samples: 0,
        completed_segments: 0,
        max_source_pts: source_start_secs,
    };

    let limit_at = options
        .max_source_secs
        .map(|seconds| source_start_secs + seconds);
    let video_tb_secs =
        f64::from(video_time_base.numerator()) / f64::from(video_time_base.denominator());
    let mut next_video_pts = source_start_secs;
    let mut reached_limit = false;
    let mut packets = input.packets();
    while !reached_limit {
        state.wait_for_capacity()?;
        state.checkpoint()?;
        let started = Instant::now();
        let item = packets.next();
        state.times.demux_secs += started.elapsed().as_secs_f64();
        let Some(item) = item else {
            break;
        };
        let (stream, packet) = item.map_err(|error| format!("demux packet: {error}"))?;
        state.packets = state.packets.saturating_add(1);
        if stream.index() == video_stream_index {
            state.checkpoint()?;
            let send_started = Instant::now();
            video_decoder
                .decoder_mut()
                .send_packet(&packet)
                .map_err(|error| format!("video send_packet: {error}"))?;
            state.times.video_decode_secs += send_started.elapsed().as_secs_f64();
            loop {
                let mut frame = Video::empty();
                let receive_started = Instant::now();
                let received = video_decoder.decoder_mut().receive_frame(&mut frame);
                state.times.video_decode_secs += receive_started.elapsed().as_secs_f64();
                match received {
                    Ok(()) => {
                        let pts = super::decoder::video_frame_timestamp(&frame)
                            .map(|pts| pts as f64 * video_tb_secs)
                            .unwrap_or(next_video_pts);
                        next_video_pts = pts
                            + f64::from(frame_rate.denominator) / f64::from(frame_rate.numerator);
                        if limit_at.is_some_and(|limit| pts > limit) {
                            reached_limit = true;
                            break;
                        }
                        state.push_video_frame(&frame, pts)?;
                    }
                    Err(ffmpeg::Error::Other { errno }) if errno == FFMPEG_EAGAIN => break,
                    Err(ffmpeg::Error::Eof) => break,
                    Err(error) => return Err(format!("video receive_frame: {error}")),
                }
            }
        } else if Some(stream.index()) == audio_stream_index {
            let path = audio_path
                .as_mut()
                .expect("index exists only with audio path");
            state.checkpoint()?;
            let send_started = Instant::now();
            path.decoder
                .send_packet(&packet)
                .map_err(|error| format!("audio send_packet: {error}"))?;
            state.times.audio_decode_resample_secs += send_started.elapsed().as_secs_f64();
            loop {
                let mut frame = Audio::empty();
                let receive_started = Instant::now();
                let received = path.decoder.receive_frame(&mut frame);
                state.times.audio_decode_resample_secs += receive_started.elapsed().as_secs_f64();
                match received {
                    Ok(()) => {
                        let convert_started = Instant::now();
                        let chunk = path.frame_to_chunk(&mut frame)?;
                        state.times.audio_decode_resample_secs +=
                            convert_started.elapsed().as_secs_f64();
                        if let Some(chunk) = chunk {
                            state.push_audio_chunk(chunk)?;
                        }
                    }
                    Err(ffmpeg::Error::Other { errno }) if errno == FFMPEG_EAGAIN => break,
                    Err(ffmpeg::Error::Eof) => break,
                    Err(error) => return Err(format!("audio receive_frame: {error}")),
                }
            }
        }
    }

    finish_transcode(
        state,
        audio_path.as_mut(),
        source_start_secs,
        wall_started,
        source_codec,
        source_width,
        source_height,
        decoder_name,
        hardware_decode_active,
        encoder,
        output,
        audio_codec,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_transcode(
    mut state: DriverState<'_>,
    _audio_path: Option<&mut AudioPath>,
    source_start_secs: f64,
    wall_started: Instant,
    source_codec: String,
    source_width: u32,
    source_height: u32,
    decoder_name: String,
    hardware_decode_active: bool,
    encoder: String,
    output: OutputDimensions,
    audio_codec: Option<String>,
) -> Result<ClocklessTranscodeReport, String> {
    state.wait_for_capacity()?;
    state.checkpoint()?;
    if state.options.include_audio {
        let started = Instant::now();
        if let Some(sequence) = state.mux.drain_ready(&mut state.segmenter)? {
            state.completed_segments = state.completed_segments.max(sequence.saturating_add(1));
        }
        state.times.mux_secs += started.elapsed().as_secs_f64();
    } else {
        while let Some(packet) = state.mux.video.pop_front() {
            if let Some(sequence) = state
                .segmenter
                .push_packet(&packet.packet)
                .map_err(|error| error.to_string())?
            {
                state.completed_segments = state.completed_segments.max(sequence.saturating_add(1));
            }
        }
    }
    state.observe_segments();

    let wall_secs = wall_started.elapsed().as_secs_f64();
    let source_secs_processed = (state.max_source_pts - source_start_secs).max(0.0);
    Ok(ClocklessTranscodeReport {
        source_path: state.options.path.clone(),
        source_codec,
        source_width,
        source_height,
        frame_rate_num: state.frame_rate.numerator,
        frame_rate_den: state.frame_rate.denominator,
        decoder_name,
        hardware_decode_active,
        encoder,
        output_width: output.width,
        output_height: output.height,
        include_audio: state.options.include_audio,
        audio_codec,
        source_secs_processed,
        wall_secs,
        realtime_multiple: if wall_secs > 0.0 {
            source_secs_processed / wall_secs
        } else {
            0.0
        },
        input_packets: state.packets,
        video_frames: state.video_frames,
        audio_frames: state.audio_frames,
        completed_segments: state.completed_segments,
        scale_profile_samples: state.scale_profile_samples,
        times: state.times,
    })
}

fn validate_options(
    options: &ClocklessTranscodeOptions,
    control: &ClocklessTranscodeControl,
) -> Result<(), String> {
    if !Path::new(&options.path).is_file() {
        return Err(format!("input is not a file: {}", options.path.display()));
    }
    if options.segment_capacity == 0 {
        return Err("segment capacity must be non-zero".to_owned());
    }
    if u64::try_from(options.segment_capacity).ok() != Some(control.inner.capacity) {
        return Err("control capacity does not match transcode options".to_owned());
    }
    if options
        .max_source_secs
        .is_some_and(|seconds| !seconds.is_finite() || seconds <= 0.0)
    {
        return Err("max source seconds must be finite and positive".to_owned());
    }
    Ok(())
}

fn stream_start_secs(stream: &ffmpeg::Stream<'_>) -> f64 {
    let start = stream.start_time();
    if start == i64::MIN {
        return 0.0;
    }
    let time_base = stream.time_base();
    start as f64 * f64::from(time_base.numerator()) / f64::from(time_base.denominator())
}

fn selected_frame_rate(
    avg: ffmpeg::Rational,
    nominal: ffmpeg::Rational,
) -> Result<FrameRate, String> {
    let rate = [avg, nominal]
        .into_iter()
        .find(|rate| rate.numerator() > 0 && rate.denominator() > 0)
        .unwrap_or(ffmpeg::Rational(30, 1));
    FrameRate::new(rate.numerator() as u32, rate.denominator() as u32)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::*;

    #[test]
    fn full_ahead_window_stops_and_release_resumes_driver() {
        let control = ClocklessTranscodeControl::manual(2).unwrap();
        control.record_produced(2);
        let waiter = control.clone();
        let (tx, rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            waiter.wait_for_capacity().unwrap();
            tx.send(()).unwrap();
        });

        thread::sleep(Duration::from_millis(30));
        assert!(rx.try_recv().is_err());
        assert!(control.snapshot().waiting_for_capacity);

        control.release_through(0);
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        thread.join().unwrap();
        assert!(!control.snapshot().waiting_for_capacity);
    }

    #[test]
    fn cancel_is_observed_at_the_next_stage_boundary() {
        let control = ClocklessTranscodeControl::manual(2).unwrap();
        let worker = control.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let first = worker.run_stage(|| {
                entered_tx.send(()).unwrap();
                finish_rx.recv().unwrap();
                7_u32
            });
            let second_ran = AtomicBool::new(false);
            let second = worker.run_stage(|| second_ran.store(true, Ordering::Release));
            result_tx
                .send((first, second, second_ran.load(Ordering::Acquire)))
                .unwrap();
        });

        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        control.cancel();
        finish_tx.send(()).unwrap();
        let (first, second, second_ran) = result_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(first, Err(ClocklessStop::Cancelled));
        assert_eq!(second, Err(ClocklessStop::Cancelled));
        assert!(!second_ran);
        thread.join().unwrap();
    }

    #[test]
    fn future_release_cannot_create_capacity() {
        let control = ClocklessTranscodeControl::manual(3).unwrap();
        control.record_produced(1);
        control.release_through(u64::MAX);
        assert_eq!(
            control.snapshot(),
            ClocklessAheadSnapshot {
                produced_segments: 1,
                released_segments: 1,
                waiting_for_capacity: false,
                cancelled: false,
            }
        );
    }
}
