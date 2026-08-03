//! Clock-free remote-video transcode driver.
//!
//! This path owns a separate demuxer/decoder and never consults `VideoPlayer`, the presentation
//! clock, or the audio device. The remote streaming session owns this driver and consumes its
//! bounded output ring; the standalone benchmark uses the same path with automatic consumption.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
use super::audio::SafetyLimiter;
use super::stream::audio_encoder::{OpenedAacEncoder, open_aac_encoder};
use super::stream::encoder::{
    EncoderPreference, FrameRate, H264InputFormat, SEGMENT_DURATION_SECS,
};
use super::stream::playlist::SegmentLookup;
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
    pub(crate) encoder: EncoderPreference,
    /// `None` decodes to EOF. The limit is measured from the selected streams' earliest start.
    pub max_source_secs: Option<f64>,
    pub segment_capacity: usize,
    /// Run one extra swscale operation per video frame for stage attribution. This intentionally
    /// reduces throughput and must be false for the headline real-time multiple.
    pub profile_swscale: bool,
    /// Requested source-timeline origin for this generation.
    pub source_origin_secs: f64,
    /// Runtime generation identifier used only for lifecycle diagnostics.
    pub(crate) diagnostic_generation: Option<u64>,
}

impl ClocklessTranscodeOptions {
    pub fn benchmark(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            include_audio: true,
            hw_decode: true,
            quality: ClocklessQuality::Standard,
            encoder: EncoderPreference::Auto,
            max_source_secs: Some(30.0),
            segment_capacity: 30,
            profile_swscale: false,
            source_origin_secs: 0.0,
            diagnostic_generation: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClocklessOutputInfo {
    pub(crate) encoder: super::stream::encoder::H264EncoderKind,
    pub(crate) output_dimensions: OutputDimensions,
    pub(crate) video_bitrate_bps: u64,
    pub(crate) audio_bitrate_bps: u64,
    pub(crate) codecs: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ClocklessOutputMetrics {
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

struct ClocklessOutputState {
    segmenter: Option<Fmp4Segmenter>,
    info: Option<ClocklessOutputInfo>,
    segment_capacity: usize,
    source_origin_secs: f64,
    generated_duration_secs: f64,
    evicted_duration_secs: f64,
    retained: VecDeque<(u64, f64)>,
    ended: bool,
}

#[derive(Clone)]
pub(crate) struct ClocklessStreamOutput {
    inner: Arc<Mutex<ClocklessOutputState>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ClocklessSegmentBytes {
    Found(Vec<u8>),
    Gone,
    NotFound,
}

#[derive(Clone)]
pub(crate) struct ClocklessAudioProcessing {
    pub(crate) normalize_gain: f64,
}

impl Default for ClocklessAudioProcessing {
    fn default() -> Self {
        Self {
            normalize_gain: 1.0,
        }
    }
}

impl ClocklessStreamOutput {
    pub(crate) fn new(segment_capacity: usize, source_origin_secs: f64) -> Result<Self, String> {
        if segment_capacity == 0 {
            return Err("segment capacity must be non-zero".to_owned());
        }
        if !source_origin_secs.is_finite() || source_origin_secs < 0.0 {
            return Err("source origin must be finite and non-negative".to_owned());
        }
        let retained_capacity = retained_segment_capacity(segment_capacity)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(ClocklessOutputState {
                segmenter: None,
                info: None,
                segment_capacity: retained_capacity,
                source_origin_secs,
                generated_duration_secs: 0.0,
                evicted_duration_secs: 0.0,
                retained: VecDeque::with_capacity(retained_capacity),
                ended: false,
            })),
        })
    }

    pub(crate) fn info(&self) -> Option<ClocklessOutputInfo> {
        self.inner.lock().unwrap().info.clone()
    }

    pub(crate) fn master_playlist(&self) -> Option<String> {
        self.inner
            .lock()
            .unwrap()
            .segmenter
            .as_ref()
            .and_then(Fmp4Segmenter::master_playlist)
    }

    pub(crate) fn media_playlist(&self) -> Option<String> {
        let state = self.inner.lock().unwrap();
        state
            .segmenter
            .as_ref()?
            .media_playlist()
            .map(|body| media_playlist_with_end(body, state.ended))
    }

    pub(crate) fn init_segment(&self) -> Option<Vec<u8>> {
        self.inner
            .lock()
            .unwrap()
            .segmenter
            .as_ref()
            .and_then(Fmp4Segmenter::init_segment)
            .map(<[u8]>::to_vec)
    }

    pub(crate) fn segment(&self, sequence: u64) -> ClocklessSegmentBytes {
        let state = self.inner.lock().unwrap();
        let Some(segmenter) = state.segmenter.as_ref() else {
            return ClocklessSegmentBytes::NotFound;
        };
        match segmenter.segment(sequence) {
            SegmentLookup::Found(segment) => ClocklessSegmentBytes::Found(segment.bytes.clone()),
            SegmentLookup::Gone => ClocklessSegmentBytes::Gone,
            SegmentLookup::NotFound => ClocklessSegmentBytes::NotFound,
        }
    }

    pub(crate) fn metrics(&self) -> ClocklessOutputMetrics {
        let state = self.inner.lock().unwrap();
        let generated_end_secs = state.source_origin_secs + state.generated_duration_secs;
        ClocklessOutputMetrics {
            source_origin_secs: state.source_origin_secs,
            generated_start_secs: state.source_origin_secs,
            generated_end_secs,
            ring_start_secs: state.source_origin_secs + state.evicted_duration_secs,
            ring_end_secs: generated_end_secs,
            earliest_sequence: state.retained.front().map(|(sequence, _)| *sequence),
            latest_sequence: state.retained.back().map(|(sequence, _)| *sequence),
            buffered_secs: state
                .segmenter
                .as_ref()
                .map_or(0.0, Fmp4Segmenter::buffered_duration_secs),
            effective_bitrate_bps: state
                .segmenter
                .as_ref()
                .map_or(0, Fmp4Segmenter::effective_bitrate_bps),
            ended: state.ended,
        }
    }

    fn install(
        &self,
        segmenter: Fmp4Segmenter,
        info: ClocklessOutputInfo,
        source_origin_secs: f64,
    ) {
        let mut state = self.inner.lock().unwrap();
        state.segmenter = Some(segmenter);
        state.info = Some(info);
        state.source_origin_secs = source_origin_secs;
    }

    fn with_segmenter<T>(
        &self,
        action: impl FnOnce(&mut Fmp4Segmenter) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut state = self.inner.lock().unwrap();
        let segmenter = state
            .segmenter
            .as_mut()
            .ok_or_else(|| "clockless output is not initialized".to_owned())?;
        action(segmenter)
    }

    fn record_completed(&self, sequence: u64) -> Result<(), String> {
        let mut state = self.inner.lock().unwrap();
        if state
            .retained
            .back()
            .is_some_and(|(last, _)| *last >= sequence)
        {
            return Ok(());
        }
        let first = state
            .retained
            .back()
            .map_or(0, |(last, _)| last.saturating_add(1));
        for completed in first..=sequence {
            let duration = match state
                .segmenter
                .as_ref()
                .ok_or_else(|| "clockless output is not initialized".to_owned())?
                .segment(completed)
            {
                SegmentLookup::Found(segment) => segment.duration_secs,
                _ => return Err(format!("completed segment {completed} is unavailable")),
            };
            state.generated_duration_secs += duration;
            state.retained.push_back((completed, duration));
        }
        while state.retained.len() > state.segment_capacity {
            if let Some((_, duration)) = state.retained.pop_front() {
                state.evicted_duration_secs += duration;
            }
        }
        Ok(())
    }

    fn finish(&self) -> Result<Option<u64>, String> {
        let mut state = self.inner.lock().unwrap();
        let sequence = state
            .segmenter
            .as_mut()
            .ok_or_else(|| "clockless output is not initialized".to_owned())?
            .finish()
            .map_err(|error| error.to_string())?;
        drop(state);
        if let Some(sequence) = sequence {
            self.record_completed(sequence)?;
        }
        self.inner.lock().unwrap().ended = true;
        Ok(sequence)
    }
}

fn media_playlist_with_end(mut body: String, ended: bool) -> String {
    const START_AT_GENERATION_ORIGIN: &str = "#EXT-X-START:TIME-OFFSET=0,PRECISE=YES\n";
    if !body.contains("#EXT-X-START:") {
        let insert_at = body.find('\n').map_or(0, |index| index + 1);
        body.insert_str(insert_at, START_AT_GENERATION_ORIGIN);
    }
    if ended && !body.contains("#EXT-X-ENDLIST") {
        body.push_str("#EXT-X-ENDLIST\n");
    }
    body
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
    pub audio_process_secs: f64,
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
    phase: AheadPhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AheadPhase {
    Producing,
    Finishing,
}

#[derive(Debug)]
struct ClocklessControlInner {
    capacity: u64,
    auto_release: bool,
    cancel: AtomicBool,
    diagnostic_generation: AtomicU64,
    external_cancel: Mutex<Option<Arc<AtomicBool>>>,
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
                diagnostic_generation: AtomicU64::new(0),
                external_cancel: Mutex::new(None),
                state: Mutex::new(AheadState {
                    produced_segments: 0,
                    released_segments: 0,
                    waiting_for_capacity: false,
                    phase: AheadPhase::Producing,
                }),
                wake: Condvar::new(),
            }),
        })
    }

    pub fn cancel(&self) {
        self.inner.cancel.store(true, Ordering::Release);
        self.inner.wake.notify_all();
    }

    pub(crate) fn bind_cancel_flag(&self, cancel: Arc<AtomicBool>) {
        *self.inner.external_cancel.lock().unwrap() = Some(cancel);
    }

    fn set_diagnostic_generation(&self, generation: Option<u64>) {
        self.inner
            .diagnostic_generation
            .store(generation.unwrap_or(0), Ordering::Release);
    }

    fn diagnostic_generation(&self) -> String {
        match self.inner.diagnostic_generation.load(Ordering::Acquire) {
            0 => "standalone".to_owned(),
            generation => generation.to_string(),
        }
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
        let externally_cancelled = self
            .inner
            .external_cancel
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|cancel| cancel.load(Ordering::Acquire));
        if self.inner.cancel.load(Ordering::Acquire) || externally_cancelled {
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
            if state.phase == AheadPhase::Finishing {
                if state.waiting_for_capacity {
                    crate::logger::log(format!(
                        "remote-stream clockless generation resumed: generation={} reason=finishing produced_segments={} released_segments={}",
                        self.diagnostic_generation(),
                        state.produced_segments,
                        state.released_segments,
                    ));
                }
                state.waiting_for_capacity = false;
                return Ok(());
            }
            let ahead = state
                .produced_segments
                .saturating_sub(state.released_segments);
            // Keep one bounded working fragment beyond the advertised ahead target. Without it,
            // a source ending just after a full segment parks before demux can observe EOF; the
            // terminal fragment then has no URL that a browser could fetch to release capacity.
            if ahead <= self.inner.capacity {
                if state.waiting_for_capacity {
                    crate::logger::log(format!(
                        "remote-stream clockless generation resumed: generation={} produced_segments={} released_segments={} ahead_segments={ahead}",
                        self.diagnostic_generation(),
                        state.produced_segments,
                        state.released_segments,
                    ));
                }
                state.waiting_for_capacity = false;
                return Ok(());
            }
            if !state.waiting_for_capacity {
                state.waiting_for_capacity = true;
                crate::logger::log(format!(
                    "remote-stream clockless generation parked: generation={} produced_segments={} released_segments={} ahead_segments={ahead} target_segments={}",
                    self.diagnostic_generation(),
                    state.produced_segments,
                    state.released_segments,
                    self.inner.capacity,
                ));
            }
            state = self.inner.wake.wait(state).unwrap();
        }
    }

    /// EOF has already been observed, so only bounded decoder/encoder delay and the final
    /// fragment remain. They use the output ring's dedicated terminal slot instead of waiting
    /// for a browser request that cannot name the unpublished final fragment yet.
    fn begin_finishing(&self) {
        let mut state = self.inner.state.lock().unwrap();
        state.phase = AheadPhase::Finishing;
        state.waiting_for_capacity = false;
        self.inner.wake.notify_all();
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

    fn drain_all(&mut self, segmenter: &mut Fmp4Segmenter) -> Result<Option<u64>, String> {
        self.drain_while(segmenter, |_| true)
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
    output_rate: u32,
    next_pts_secs: Option<f64>,
    codec_name: String,
}

impl AudioPath {
    fn open(stream: ffmpeg::Stream<'_>, output_rate: u32) -> Result<Self, String> {
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
            output_rate,
        )
        .map_err(|error| format!("audio resampler open: {error}"))?;
        Ok(Self {
            decoder,
            resampler,
            time_base_secs: f64::from(time_base.numerator()) / f64::from(time_base.denominator()),
            input_rate,
            output_rate,
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
            self.output_rate,
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
            output.set_rate(self.output_rate);
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
        let duration_secs = frames as f64 / f64::from(self.output_rate);
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

    fn flush_resampler(&mut self) -> Result<Option<ProcessedChunk>, String> {
        let pts = self.next_pts_secs.unwrap_or(0.0);
        let delay = self
            .resampler
            .delay()
            .map(|delay| delay.output.max(0) as usize)
            .unwrap_or(0);
        if delay == 0 {
            return Ok(None);
        }
        let mut output = Audio::empty();
        unsafe {
            output.alloc(
                Sample::F32(SampleType::Packed),
                delay.saturating_add(32),
                ffmpeg::ChannelLayoutMask::STEREO,
            );
            output.set_rate(self.output_rate);
        }
        self.resampler
            .flush(&mut output)
            .map_err(|error| format!("audio resampler flush: {error}"))?;
        self.output_to_chunk(output, pts)
    }
}

struct ClocklessAudioProcessor {
    normalize_gain: f32,
    limiter: SafetyLimiter,
}

impl ClocklessAudioProcessor {
    fn new(config: ClocklessAudioProcessing, sample_rate: u32) -> Result<Self, String> {
        if !config.normalize_gain.is_finite() || config.normalize_gain < 0.0 {
            return Err("normalize gain must be finite and non-negative".to_owned());
        }
        Ok(Self {
            normalize_gain: config.normalize_gain as f32,
            limiter: SafetyLimiter::new(sample_rate, 2),
        })
    }

    fn process(&mut self, mut chunk: ProcessedChunk) -> ProcessedChunk {
        if (self.normalize_gain - 1.0).abs() > f32::EPSILON {
            for sample in &mut chunk.samples {
                *sample *= self.normalize_gain;
            }
        }

        let mut latency_secs = 0.0;
        if self.normalize_gain > 1.0 + f32::EPSILON {
            self.limiter.process_block(&mut chunk.samples);
            latency_secs += self.limiter.latency_secs();
        } else {
            self.limiter.reset();
        }
        chunk.audible_pts_secs -= latency_secs;
        chunk.pdc_latency_secs_at_process = latency_secs;
        chunk
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
    output: ClocklessStreamOutput,
    audio_processor: ClocklessAudioProcessor,
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
        self.wait_for_capacity()?;
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
        let packets = self.output.with_segmenter(|segmenter| {
            self.video
                .encode_frame(tapped, segmenter)
                .map_err(|error| error.to_string())
        })?;
        self.times.video_scale_encode_secs += started.elapsed().as_secs_f64();
        for packet in packets {
            self.mux.enqueue_video(packet, self.frame_rate)?;
        }
        self.video_frames = self.video_frames.saturating_add(1);
        self.max_source_pts = self.max_source_pts.max(pts_secs);
        self.drain_mux()
    }

    fn push_audio_chunk(&mut self, chunk: ProcessedChunk) -> Result<(), String> {
        self.wait_for_capacity()?;
        self.checkpoint()?;
        let started = Instant::now();
        let chunk = self.audio_processor.process(chunk);
        self.times.audio_process_secs += started.elapsed().as_secs_f64();
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
            let sequence = self
                .output
                .with_segmenter(|segmenter| self.mux.drain_ready(segmenter))?;
            if let Some(sequence) = sequence {
                self.completed_segments = self.completed_segments.max(sequence.saturating_add(1));
                self.output.record_completed(sequence)?;
            }
        } else {
            while let Some(packet) = self.mux.video.pop_front() {
                if let Some(sequence) = self.output.with_segmenter(|segmenter| {
                    segmenter
                        .push_packet(&packet.packet)
                        .map_err(|error| error.to_string())
                })? {
                    self.completed_segments =
                        self.completed_segments.max(sequence.saturating_add(1));
                    self.output.record_completed(sequence)?;
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
    let output = ClocklessStreamOutput::new(options.segment_capacity, options.source_origin_secs)?;
    run_clockless_stream(
        options,
        control,
        output,
        ClocklessAudioProcessing::default(),
        |_| {},
    )
}

pub(crate) fn run_clockless_stream(
    options: &ClocklessTranscodeOptions,
    control: &ClocklessTranscodeControl,
    stream_output: ClocklessStreamOutput,
    audio_processing: ClocklessAudioProcessing,
    on_ready: impl FnOnce(ClocklessOutputInfo),
) -> Result<ClocklessTranscodeReport, String> {
    control.set_diagnostic_generation(options.diagnostic_generation);
    let generation = control.diagnostic_generation();
    crate::logger::log(format!(
        "remote-stream clockless generation start: generation={generation} origin_secs={:.3} prefetch_target_secs={:.3} segment_capacity={}",
        options.source_origin_secs,
        options.segment_capacity as f64 * f64::from(SEGMENT_DURATION_SECS),
        options.segment_capacity,
    ));
    let result =
        run_clockless_stream_inner(options, control, stream_output, audio_processing, on_ready);
    let reason = match &result {
        Ok(_) => "complete".to_owned(),
        Err(error) if error == "clockless transcode cancelled" => "cancel".to_owned(),
        Err(error) => format!("error error={error}"),
    };
    crate::logger::log(format!(
        "remote-stream clockless generation stopped: generation={generation} reason={reason}"
    ));
    result
}

fn run_clockless_stream_inner(
    options: &ClocklessTranscodeOptions,
    control: &ClocklessTranscodeControl,
    stream_output: ClocklessStreamOutput,
    audio_processing: ClocklessAudioProcessing,
    on_ready: impl FnOnce(ClocklessOutputInfo),
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
    let source_origin_secs = options.source_origin_secs.max(source_start_secs);
    let timeline = StreamTimeline::new(source_origin_secs).map_err(|error| error.to_string())?;
    let processing_sample_rate = AUDIO_OUTPUT_RATE;
    let mut audio_path = audio_stream
        .map(|stream| AudioPath::open(stream, processing_sample_rate))
        .transpose()?;
    if options.include_audio && audio_path.is_none() {
        return Err("include_audio was requested but the input has no audio stream".to_owned());
    }
    let audio_codec = audio_path.as_ref().map(|path| path.codec_name.clone());

    let (video_tap_controller, video_tap) = video_tap_channel();
    let (_video_tap_lease, video_rx) = video_tap_controller.attach(1).map_err(str::to_owned)?;
    let video = open_video_stream_encoder(
        options.encoder,
        options.quality.internal(),
        source_width,
        source_height,
        frame_rate,
        SEEK_SERIAL,
        timeline,
    )
    .map_err(|error| error.to_string())?;
    let audio_encoder = open_aac_encoder(
        processing_sample_rate,
        options.quality.internal().parameters().audio_bitrate_bps,
        SEEK_SERIAL,
        timeline,
    )
    .map_err(|error| error.to_string())?;
    let segmenter = Fmp4Segmenter::with_capacity(
        video.encoder(),
        &audio_encoder.encoder,
        frame_rate,
        retained_segment_capacity(options.segment_capacity)?,
    )
    .map_err(|error| error.to_string())?;
    let encoder = video.encoder_kind().as_str().to_owned();
    let output = video.output_parameters().dimensions;
    let output_info = ClocklessOutputInfo {
        encoder: video.encoder_kind(),
        output_dimensions: output,
        video_bitrate_bps: video.effective_video_bitrate_bps(),
        audio_bitrate_bps: audio_encoder.effective_bitrate_bps(),
        codecs: segmenter.codecs().to_owned(),
    };
    let audio_processor = ClocklessAudioProcessor::new(audio_processing, processing_sample_rate)?;
    stream_output.install(segmenter, output_info.clone(), source_origin_secs);
    on_ready(output_info);
    let mut state = DriverState {
        options,
        control,
        frame_rate,
        video,
        video_tap,
        video_rx,
        audio_encoder,
        output: stream_output,
        audio_processor,
        mux: ClocklessMux::default(),
        scale_profiler: None,
        times,
        packets: 0,
        video_frames: 0,
        audio_frames: 0,
        scale_profile_samples: 0,
        completed_segments: 0,
        max_source_pts: source_origin_secs,
    };

    let limit_at = options
        .max_source_secs
        .map(|seconds| source_origin_secs + seconds);
    let video_tb_secs =
        f64::from(video_time_base.numerator()) / f64::from(video_time_base.denominator());
    let mut next_video_pts = source_origin_secs;
    if source_origin_secs > source_start_secs + f64::EPSILON {
        let target = (source_origin_secs * 1_000_000.0).round() as i64;
        let seek_result = unsafe {
            ffmpeg::ffi::av_seek_frame(
                input.as_mut_ptr(),
                -1,
                target,
                ffmpeg::ffi::AVSEEK_FLAG_BACKWARD as i32,
            )
        };
        if seek_result < 0 {
            return Err(format!(
                "clockless seek to {source_origin_secs:.3}s failed: {seek_result}"
            ));
        }
        video_decoder.decoder_mut().flush();
        if let Some(path) = audio_path.as_mut() {
            path.decoder.flush();
            path.next_pts_secs = None;
        }
    }
    let mut reached_limit = false;
    let mut packets = input.packets();
    while !reached_limit {
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
                        if pts < source_origin_secs {
                            continue;
                        }
                        if state.video_frames == 0 {
                            crate::logger::log(format!(
                                "remote-stream clockless first video frame: generation={} origin_secs={source_origin_secs:.3} frame_pts_secs={pts:.3} delta_secs={:.3}",
                                state.control.diagnostic_generation(),
                                pts - source_origin_secs,
                            ));
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
                            if chunk.audible_pts_secs + chunk.duration_secs >= source_origin_secs {
                                state.push_audio_chunk(chunk)?;
                            }
                        }
                    }
                    Err(ffmpeg::Error::Other { errno }) if errno == FFMPEG_EAGAIN => break,
                    Err(ffmpeg::Error::Eof) => break,
                    Err(error) => return Err(format!("audio receive_frame: {error}")),
                }
            }
        }
    }

    // Capacity belongs at the packet/frame production boundary, not before demux. Otherwise a
    // final full segment can fill the live window and park the worker before it observes EOF.
    // Once EOF is known, the remaining codec delay and final fragment are bounded and use the
    // ring's reserved terminal slot after the bounded working fragment.
    let ahead = state.control.snapshot();
    crate::logger::log(format!(
        "remote-stream clockless {} reached: generation={} transition=Producing->Finishing produced_segments={} released_segments={}",
        if reached_limit { "source limit" } else { "EOF" },
        state.control.diagnostic_generation(),
        ahead.produced_segments,
        ahead.released_segments,
    ));
    state.control.begin_finishing();

    if !reached_limit {
        state.checkpoint()?;
        video_decoder
            .decoder_mut()
            .send_eof()
            .map_err(|error| format!("video send_eof: {error}"))?;
        loop {
            let mut frame = Video::empty();
            match video_decoder.decoder_mut().receive_frame(&mut frame) {
                Ok(()) => {
                    let pts = super::decoder::video_frame_timestamp(&frame)
                        .map(|pts| pts as f64 * video_tb_secs)
                        .unwrap_or(next_video_pts);
                    next_video_pts =
                        pts + f64::from(frame_rate.denominator) / f64::from(frame_rate.numerator);
                    if pts >= source_origin_secs {
                        state.push_video_frame(&frame, pts)?;
                    }
                }
                Err(ffmpeg::Error::Eof) => break,
                Err(ffmpeg::Error::Other { errno }) if errno == FFMPEG_EAGAIN => break,
                Err(error) => return Err(format!("video drain receive_frame: {error}")),
            }
        }
        if let Some(path) = audio_path.as_mut() {
            path.decoder
                .send_eof()
                .map_err(|error| format!("audio send_eof: {error}"))?;
            loop {
                let mut frame = Audio::empty();
                match path.decoder.receive_frame(&mut frame) {
                    Ok(()) => {
                        if let Some(chunk) = path.frame_to_chunk(&mut frame)?
                            && chunk.audible_pts_secs + chunk.duration_secs >= source_origin_secs
                        {
                            state.push_audio_chunk(chunk)?;
                        }
                    }
                    Err(ffmpeg::Error::Eof) => break,
                    Err(ffmpeg::Error::Other { errno }) if errno == FFMPEG_EAGAIN => break,
                    Err(error) => return Err(format!("audio drain receive_frame: {error}")),
                }
            }
            if let Some(chunk) = path.flush_resampler()? {
                state.push_audio_chunk(chunk)?;
            }
        }
    }

    finish_transcode(
        state,
        audio_path.as_mut(),
        source_origin_secs,
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
    let started = Instant::now();
    for packet in state.video.finish().map_err(|error| error.to_string())? {
        state.mux.enqueue_video(packet, state.frame_rate)?;
    }
    state.times.video_scale_encode_secs += started.elapsed().as_secs_f64();
    if state.options.include_audio {
        let started = Instant::now();
        for packet in state
            .audio_encoder
            .finish()
            .map_err(|error| error.to_string())?
        {
            state
                .mux
                .enqueue_audio(packet, state.audio_encoder.output_sample_rate())?;
        }
        state.times.audio_encode_secs += started.elapsed().as_secs_f64();
    }

    let started = Instant::now();
    let completed = state
        .output
        .with_segmenter(|segmenter| state.mux.drain_all(segmenter))?;
    state.times.mux_secs += started.elapsed().as_secs_f64();
    if let Some(sequence) = completed {
        state.completed_segments = state.completed_segments.max(sequence.saturating_add(1));
        state.output.record_completed(sequence)?;
    }
    state.wait_for_capacity()?;
    let terminal_sequence = state.output.finish()?;
    if let Some(sequence) = terminal_sequence {
        state.completed_segments = state.completed_segments.max(sequence.saturating_add(1));
    }
    state.observe_segments();
    let final_metrics = state.output.metrics();
    crate::logger::log(format!(
        "remote-stream clockless final fragment published: generation={} terminal_sequence={} generated_end_secs={:.3} ended={}",
        state.control.diagnostic_generation(),
        terminal_sequence.map_or_else(|| "none".to_owned(), |sequence| sequence.to_string()),
        final_metrics.generated_end_secs,
        final_metrics.ended,
    ));

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
    retained_segment_capacity(options.segment_capacity)?;
    if u64::try_from(options.segment_capacity).ok() != Some(control.inner.capacity) {
        return Err("control capacity does not match transcode options".to_owned());
    }
    if options
        .max_source_secs
        .is_some_and(|seconds| !seconds.is_finite() || seconds <= 0.0)
    {
        return Err("max source seconds must be finite and positive".to_owned());
    }
    if !options.source_origin_secs.is_finite() || options.source_origin_secs < 0.0 {
        return Err("source origin must be finite and non-negative".to_owned());
    }
    Ok(())
}

fn retained_segment_capacity(segment_capacity: usize) -> Result<usize, String> {
    segment_capacity.checked_add(2).ok_or_else(|| {
        "segment capacity leaves no room for the working and terminal fragments".to_owned()
    })
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
    use std::path::Path;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::*;

    fn write_finite_av_fixture(path: &Path, video_frames: i64) {
        use ffmpeg::codec::packet::Flags;

        const WIDTH: usize = 320;
        const HEIGHT: usize = 180;
        const AUDIO_SAMPLES_PER_PACKET: usize = 4_000;
        const AUDIO_RATE: i32 = 8_000;

        ffmpeg::init().unwrap();
        let mut output = ffmpeg::format::output_as(path, "avi").unwrap();
        let video_codec = ffmpeg::codec::encoder::find(ffmpeg::codec::Id::RAWVIDEO).unwrap();
        let audio_codec = ffmpeg::codec::encoder::find(ffmpeg::codec::Id::PCM_S16LE).unwrap();
        let video_index;
        {
            let mut stream = output.add_stream(video_codec).unwrap();
            video_index = stream.index();
            stream.set_time_base((1, 2));
            stream.set_rate((2, 1));
            stream.set_avg_frame_rate((2, 1));
            let mut parameters = stream.parameters_mut();
            unsafe {
                let parameters = &mut *parameters.as_mut_ptr();
                parameters.codec_type = ffmpeg::ffi::AVMediaType::AVMEDIA_TYPE_VIDEO;
                parameters.codec_id = ffmpeg::ffi::AVCodecID::AV_CODEC_ID_RAWVIDEO;
                parameters.format = ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_BGR24 as i32;
                parameters.width = WIDTH as i32;
                parameters.height = HEIGHT as i32;
                parameters.framerate = ffmpeg::ffi::AVRational { num: 2, den: 1 };
                parameters.bits_per_coded_sample = 24;
            }
        }
        let audio_index;
        {
            let mut stream = output.add_stream(audio_codec).unwrap();
            audio_index = stream.index();
            stream.set_time_base((1, AUDIO_RATE));
            let mut parameters = stream.parameters_mut();
            unsafe {
                let parameters = &mut *parameters.as_mut_ptr();
                parameters.codec_type = ffmpeg::ffi::AVMediaType::AVMEDIA_TYPE_AUDIO;
                parameters.codec_id = ffmpeg::ffi::AVCodecID::AV_CODEC_ID_PCM_S16LE;
                parameters.format = ffmpeg::ffi::AVSampleFormat::AV_SAMPLE_FMT_S16 as i32;
                parameters.sample_rate = AUDIO_RATE;
                parameters.bits_per_coded_sample = 16;
                parameters.bits_per_raw_sample = 16;
                parameters.block_align = 4;
                parameters.bit_rate = i64::from(AUDIO_RATE) * 16 * 2;
                ffmpeg::ffi::av_channel_layout_default(&mut parameters.ch_layout, 2);
            }
        }
        output.write_header().unwrap();

        for index in 0..video_frames {
            let mut pixels = vec![0_u8; WIDTH * HEIGHT * 3];
            for pixel in pixels.chunks_exact_mut(3) {
                pixel[0] = (index * 31) as u8;
                pixel[1] = 64;
                pixel[2] = 192;
            }
            let mut video = ffmpeg::Packet::copy(&pixels);
            video.set_stream(video_index);
            video.set_pts(Some(index));
            video.set_dts(Some(index));
            video.set_duration(1);
            video.set_flags(Flags::KEY);
            video.write_interleaved(&mut output).unwrap();

            let samples = vec![0_u8; AUDIO_SAMPLES_PER_PACKET * 2 * std::mem::size_of::<i16>()];
            let mut audio = ffmpeg::Packet::copy(&samples);
            audio.set_stream(audio_index);
            audio.set_pts(Some(index * AUDIO_SAMPLES_PER_PACKET as i64));
            audio.set_dts(Some(index * AUDIO_SAMPLES_PER_PACKET as i64));
            audio.set_duration(AUDIO_SAMPLES_PER_PACKET as i64);
            audio.set_flags(Flags::KEY);
            audio.write_interleaved(&mut output).unwrap();
        }
        output.write_trailer().unwrap();
    }

    #[test]
    fn full_ahead_window_stops_and_release_resumes_driver() {
        let control = ClocklessTranscodeControl::manual(2).unwrap();
        // The advertised two-segment target has one bounded working fragment. The producer
        // parks before accepting another frame only after all three are occupied.
        control.record_produced(3);
        let waiter = control.clone();
        let (tx, rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            waiter.wait_for_capacity().unwrap();
            tx.send(()).unwrap();
        });

        // 「少し眠ればもう待機に入っているはず」は仮定であって観測ではない。lib test 全件
        // (4800 超) と並列に走ると、この thread が 30ms 以内に scheduler へ回る保証は無く、
        // 実際に単独では通るのに全件実行でだけ落ちた。状態そのものを待つ。
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !control.snapshot().waiting_for_capacity {
            assert!(
                std::time::Instant::now() < deadline,
                "driver never parked on a full ahead window"
            );
            thread::yield_now();
        }
        assert!(
            rx.try_recv().is_err(),
            "a parked driver must not have produced past the window"
        );

        control.release_through(0);
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        thread.join().unwrap();
        assert!(!control.snapshot().waiting_for_capacity);
    }

    #[test]
    fn eof_finishing_uses_terminal_slot_without_waiting_for_an_unpublished_fragment() {
        let control = ClocklessTranscodeControl::manual(1).unwrap();
        control.record_produced(2);
        let waiter = control.clone();
        let (tx, rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            waiter.wait_for_capacity().unwrap();
            tx.send(()).unwrap();
        });

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !control.snapshot().waiting_for_capacity {
            assert!(
                std::time::Instant::now() < deadline,
                "driver never parked on the full live window"
            );
            thread::yield_now();
        }
        control.begin_finishing();

        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        thread.join().unwrap();
        assert_eq!(control.snapshot().produced_segments, 2);
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

    #[test]
    fn ended_output_adds_endlist_once_without_changing_live_playlists() {
        let live = "#EXTM3U\n#EXT-X-TARGETDURATION:2\n".to_owned();
        let started = media_playlist_with_end(live.clone(), false);
        assert!(started.contains("#EXT-X-START:TIME-OFFSET=0,PRECISE=YES\n"));
        assert!(!started.contains("#EXT-X-ENDLIST"));
        let ended = media_playlist_with_end(live, true);
        assert!(ended.contains("#EXT-X-START:TIME-OFFSET=0,PRECISE=YES\n"));
        assert!(ended.ends_with("#EXT-X-ENDLIST\n"));
        assert_eq!(media_playlist_with_end(ended.clone(), true), ended);
    }

    #[test]
    fn seek_cancels_old_worker_at_boundary_while_new_generation_runs_independently() {
        let old = ClocklessTranscodeControl::manual(1).unwrap();
        let replacement = ClocklessTranscodeControl::manual(1).unwrap();
        let old_worker = old.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (leave_tx, leave_rx) = mpsc::channel();
        let (old_result_tx, old_result_rx) = mpsc::channel();
        let old_thread = thread::spawn(move || {
            let result = old_worker.run_stage(|| {
                entered_tx.send(()).unwrap();
                leave_rx.recv().unwrap();
            });
            old_result_tx.send(result).unwrap();
        });

        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(replacement.run_stage(|| 17_u32), Ok(17));
        old.cancel();
        leave_tx.send(()).unwrap();

        assert_eq!(
            old_result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err(ClocklessStop::Cancelled)
        );
        assert_eq!(replacement.checkpoint(), Ok(()));
        old_thread.join().unwrap();
    }

    #[test]
    fn normalize_gain_is_applied_before_remote_audio_encoding() {
        let mut processor = ClocklessAudioProcessor::new(
            ClocklessAudioProcessing {
                normalize_gain: 0.579,
            },
            AUDIO_OUTPUT_RATE,
        )
        .unwrap();
        let chunk = processor.process(ProcessedChunk {
            samples: vec![1.0, -0.5],
            audible_pts_secs: 10.0,
            duration_secs: 1.0 / f64::from(AUDIO_OUTPUT_RATE),
            source_secs_per_output_sec: 1.0,
            seek_serial: SEEK_SERIAL,
            pdc_latency_secs_at_process: 0.0,
        });

        assert!((chunk.samples[0] - 0.579).abs() < 1.0e-6);
        assert!((chunk.samples[1] + 0.2895).abs() < 1.0e-6);
        assert_eq!(chunk.audible_pts_secs, 10.0);
    }

    #[test]
    fn finite_fixture_flushes_final_fragment_and_endlist() {
        let temp = tempfile::tempdir().unwrap();
        let generated_path = temp.path().join("finite-av.avi");
        let external_path = std::env::var_os("MIV_CLOCKLESS_EOF_FIXTURE").map(PathBuf::from);
        let path = external_path.clone().unwrap_or_else(|| {
            write_finite_av_fixture(&generated_path, 5);
            generated_path
        });
        let mut options = ClocklessTranscodeOptions::benchmark(path);
        options.hw_decode = false;
        options.quality = ClocklessQuality::Minimum;
        options.max_source_secs = None;
        options.segment_capacity = 1;
        let control = ClocklessTranscodeControl::manual(options.segment_capacity).unwrap();
        let output = ClocklessStreamOutput::new(options.segment_capacity, 0.0).unwrap();
        let worker_control = control.clone();
        let worker_output = output.clone();
        let (result_tx, result_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let result = run_clockless_stream(
                &options,
                &worker_control,
                worker_output,
                ClocklessAudioProcessing::default(),
                |_| {},
            );
            result_tx.send(result).unwrap();
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let result = loop {
            match result_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(result) => break result,
                Err(mpsc::RecvTimeoutError::Timeout) if std::time::Instant::now() < deadline => {
                    // A real, longer fixture needs an HLS-like consumer to advance its one-slot
                    // live window. The generated 2.5 s fixture deliberately leaves the slot full
                    // so the terminal-reserve regression remains exact.
                    if external_path.is_some()
                        && let Some(sequence) = output.metrics().latest_sequence
                    {
                        control.release_through(sequence);
                    }
                }
                Err(error) => {
                    control.cancel();
                    worker.join().unwrap();
                    panic!(
                        "finite transcode did not reach EOF while the terminal slot was full: {error}"
                    );
                }
            }
        };
        result.unwrap();
        worker.join().unwrap();

        let metrics = output.metrics();
        assert!(metrics.ended);
        let latest = metrics.latest_sequence.expect("final fragment sequence");
        assert!(matches!(
            output.segment(latest),
            ClocklessSegmentBytes::Found(_)
        ));
        assert!(
            output
                .media_playlist()
                .expect("final media playlist")
                .ends_with("#EXT-X-ENDLIST\n")
        );
    }

    #[test]
    fn finite_source_reaches_end_after_filling_the_live_window_without_a_next_fetch() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("full-window-finite-av.avi");
        // 2 fps for 4.5 seconds: two complete 2-second fragments plus a finite tail. With a
        // one-segment live target this reproduces a browser that fetched the visible live edge
        // but does not ask for an unpublished terminal fragment.
        write_finite_av_fixture(&path, 9);
        let mut options = ClocklessTranscodeOptions::benchmark(path);
        options.hw_decode = false;
        options.quality = ClocklessQuality::Minimum;
        options.max_source_secs = None;
        options.segment_capacity = 1;
        let control = ClocklessTranscodeControl::manual(options.segment_capacity).unwrap();
        let output = ClocklessStreamOutput::new(options.segment_capacity, 0.0).unwrap();
        let worker_control = control.clone();
        let worker_output = output.clone();
        let (result_tx, result_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let result = run_clockless_stream(
                &options,
                &worker_control,
                worker_output,
                ClocklessAudioProcessing::default(),
                |_| {},
            );
            result_tx.send(result).unwrap();
        });

        let result = result_rx.recv_timeout(Duration::from_secs(5));
        if result.is_err() {
            control.cancel();
        }
        let result = result.expect(
            "finite producer parked at the live edge before it could observe EOF and publish ENDLIST",
        );
        result.unwrap();
        worker.join().unwrap();
        assert!(output.metrics().ended);
        assert!(
            output
                .media_playlist()
                .expect("final media playlist")
                .ends_with("#EXT-X-ENDLIST\n")
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "set MIV_VST_FAST_PROFILE_DIR to an isolated settings.db copy"]
    fn configured_vst_chain_accepts_sixty_seconds_of_clockless_feed() {
        let profile_dir = std::env::var_os("MIV_VST_FAST_PROFILE_DIR")
            .map(PathBuf::from)
            .expect("MIV_VST_FAST_PROFILE_DIR");
        let sample_rate = std::env::var("MIV_VST_FAST_SAMPLE_RATE")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(44_100);
        const BLOCK_FRAMES: u32 = 480;
        const SOURCE_SECS: u32 = 60;

        let _data_dir_serial = crate::data_dir::test_override_lock();
        crate::data_dir::set_test_override(Some(profile_dir));
        crate::settings_db::reset_global_for_test();
        crate::settings_db::set_save_suppressed(false);
        let settings = crate::settings::Settings::load();
        assert!(settings.vst3_enabled, "configured VST3 chain is disabled");
        assert!(
            !settings.vst3_plugins.is_empty(),
            "configured VST3 chain is empty"
        );

        let bridge = super::super::dsp::DspBridge::new();
        bridge.enable().unwrap();
        for plugin in &settings.vst3_plugins {
            bridge
                .add_plugin(
                    &plugin.path,
                    sample_rate,
                    BLOCK_FRAMES,
                    plugin.bypass,
                    plugin.user_hidden,
                    plugin.state.as_deref(),
                    None,
                    None,
                )
                .unwrap_or_else(|error| panic!("load {}: {error}", plugin.path));
        }
        assert!(
            bridge.active_slot_count() > 0,
            "configured chain has no active VST3 slots"
        );
        bridge.reset_plugins_sync();

        let block_samples = BLOCK_FRAMES as usize * 2;
        let source = vec![0.0_f32; block_samples];
        let mut destination = vec![0.0_f32; block_samples];
        let blocks = u64::from(sample_rate)
            .saturating_mul(u64::from(SOURCE_SECS))
            .div_ceil(u64::from(BLOCK_FRAMES));
        let started = Instant::now();
        for _ in 0..blocks {
            bridge
                .process_block(&source, &mut destination)
                .expect("VST3 fast feed");
        }
        let elapsed_secs = started.elapsed().as_secs_f64();
        let realtime_multiple = f64::from(SOURCE_SECS) / elapsed_secs;
        eprintln!(
            "MIV_VST_FAST_FEED sample_rate={sample_rate} active_slots={} \
             source_secs={SOURCE_SECS} wall_secs={elapsed_secs:.6} realtime_x={realtime_multiple:.3}",
            bridge.active_slot_count()
        );

        bridge.disable();
        crate::settings_db::reset_global_for_test();
        crate::data_dir::set_test_override(None);
    }
}
