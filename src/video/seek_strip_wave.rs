//! Window-on-demand waveform analysis for the native video seek strip.
//!
//! The worker owns one lazy-opened audio range decoder for the current video.
//! It first reuses a completed full TimelineAnalysis when the file identity
//! matches; otherwise it decodes only the visible 60-second window plus
//! pre-roll. Rasterization also runs here, and only RGBA crosses to the App and
//! native presenter. The cache is process-memory-only by design.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, bounded};
use music_core::{AnalysisConfig, TimelineAnalysis, WaveformBin};

/// One strip width in waveform mode.
pub(crate) const DEFAULT_WAVEFORM_SPAN_SECS: f64 = 60.0;
/// Filter warm-up decoded before the visible window and discarded afterwards.
pub(crate) const WAVEFORM_PRE_ROLL_SECS: f64 = 0.75;
const MIN_WAVEFORM_BIN_SECS: f64 = 0.010;
const WAVE_RASTER_LRU_MAX_ENTRIES: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct WaveFileIdentity {
    normalized_path: String,
    size: i64,
    mtime: i64,
}

impl WaveFileIdentity {
    pub(crate) fn from_known_meta(path: &Path, mtime: i64, size: i64) -> Self {
        Self {
            normalized_path: crate::adjustment_db::normalize_path(path),
            size,
            mtime,
        }
    }

    fn from_file(path: &Path) -> Self {
        let metadata = std::fs::metadata(path).ok();
        let size = metadata
            .as_ref()
            .and_then(|metadata| i64::try_from(metadata.len()).ok())
            .unwrap_or(0);
        let mtime = metadata
            .as_ref()
            .map(crate::ui_helpers::mtime_secs)
            .unwrap_or(0);
        Self::from_known_meta(path, mtime, size)
    }
}

#[derive(Clone)]
pub(crate) struct CompletedTimelineAnalysis {
    pub(crate) identity: WaveFileIdentity,
    pub(crate) analysis: Arc<TimelineAnalysis>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WaveWindow {
    pub(crate) start_secs: f64,
    pub(crate) end_secs: f64,
    pub(crate) content_start_secs: f64,
    pub(crate) content_end_secs: f64,
}

pub(crate) fn waveform_window(
    center_time_secs: f64,
    duration_secs: f64,
    span_secs: f64,
) -> Option<WaveWindow> {
    if !center_time_secs.is_finite()
        || !duration_secs.is_finite()
        || duration_secs <= 0.0
        || !span_secs.is_finite()
        || span_secs <= 0.0
    {
        return None;
    }
    let start_secs = center_time_secs - span_secs * 0.5;
    let end_secs = start_secs + span_secs;
    Some(WaveWindow {
        start_secs,
        end_secs,
        content_start_secs: start_secs.clamp(0.0, duration_secs),
        content_end_secs: end_secs.clamp(0.0, duration_secs),
    })
}

/// Choose no more than about one analysis bin per physical output pixel.
///
/// Millisecond rounding makes cache keys stable across tiny floating-point
/// changes while preserving the analyzer's 10ms minimum.
pub(crate) fn waveform_bin_secs(span_secs: f64, pixel_width: usize) -> f64 {
    if !span_secs.is_finite() || span_secs <= 0.0 || pixel_width == 0 {
        return MIN_WAVEFORM_BIN_SECS;
    }
    let raw = (span_secs / pixel_width as f64).max(MIN_WAVEFORM_BIN_SECS);
    (raw * 1000.0).ceil() / 1000.0
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct WaveAnalysisRange {
    start_secs: f64,
    end_secs: f64,
}

fn waveform_analysis_range(
    window: WaveWindow,
    duration_secs: f64,
    bin_secs: f64,
) -> Option<WaveAnalysisRange> {
    if window.content_end_secs <= window.content_start_secs
        || !duration_secs.is_finite()
        || duration_secs <= 0.0
        || !bin_secs.is_finite()
        || bin_secs <= 0.0
    {
        return None;
    }
    let raw_start = (window.content_start_secs - WAVEFORM_PRE_ROLL_SECS).max(0.0);
    // bin_secs is millisecond-rounded. Align in integer microseconds so a value
    // that is mathematically on a boundary cannot drift into the next bin due
    // to a floating-point ceil/floor result.
    let bin_micros = (bin_secs * 1_000_000.0).round().max(1.0) as u64;
    let raw_start_micros = (raw_start * 1_000_000.0).floor().max(0.0) as u64;
    let content_end_micros = (window.content_end_secs * 1_000_000.0).ceil().max(0.0) as u64;
    let start_secs = (raw_start_micros / bin_micros * bin_micros) as f64 / 1_000_000.0;
    let aligned_end_micros = content_end_micros
        .div_ceil(bin_micros)
        .saturating_mul(bin_micros);
    let end_secs = (aligned_end_micros as f64 / 1_000_000.0)
        .min(duration_secs)
        .max(window.content_end_secs);
    (end_secs > start_secs).then_some(WaveAnalysisRange {
        start_secs,
        end_secs,
    })
}

/// Half-open bin range overlapping the requested global time window.
pub(crate) fn waveform_bin_range(
    bins: &[WaveformBin],
    start_secs: f64,
    end_secs: f64,
) -> std::ops::Range<usize> {
    if bins.is_empty() || !start_secs.is_finite() || !end_secs.is_finite() || end_secs <= start_secs
    {
        return 0..0;
    }
    let start = bins.partition_point(|bin| bin.start_secs + bin.duration_secs <= start_secs);
    let end = bins.partition_point(|bin| bin.start_secs < end_secs);
    start.min(bins.len())..end.min(bins.len())
}

fn analyze_window_samples(
    stereo_samples: &[f32],
    sample_rate: u32,
    analysis_start_secs: f64,
    visible_start_secs: f64,
    visible_end_secs: f64,
    bin_secs: f64,
    should_abort: &dyn Fn() -> bool,
) -> Option<TimelineAnalysis> {
    let mut analysis = music_core::analyze_stereo_waveform_cancellable(
        stereo_samples,
        sample_rate,
        AnalysisConfig {
            bin_secs,
            row_secs: DEFAULT_WAVEFORM_SPAN_SECS,
            ..AnalysisConfig::default()
        },
        should_abort,
    )?;
    for bin in &mut analysis.bins {
        bin.start_secs += analysis_start_secs;
    }
    let keep = waveform_bin_range(&analysis.bins, visible_start_secs, visible_end_secs);
    analysis.bins = analysis.bins[keep].to_vec();
    analysis.stream.duration_secs = visible_end_secs - visible_start_secs;
    Some(analysis)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct WaveRequestSignature {
    center_bits: u64,
    duration_bits: u64,
    pixel_width: usize,
    pixel_height: usize,
}

impl WaveRequestSignature {
    pub(crate) fn new(
        center_time_secs: f64,
        duration_secs: f64,
        pixel_width: usize,
        pixel_height: usize,
    ) -> Option<Self> {
        if !center_time_secs.is_finite()
            || !duration_secs.is_finite()
            || duration_secs <= 0.0
            || pixel_width == 0
            || pixel_height == 0
        {
            return None;
        }
        Some(Self {
            center_bits: center_time_secs.to_bits(),
            duration_bits: duration_secs.to_bits(),
            pixel_width,
            pixel_height,
        })
    }

    fn center_time_secs(self) -> f64 {
        f64::from_bits(self.center_bits)
    }

    fn duration_secs(self) -> f64 {
        f64::from_bits(self.duration_bits)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct WaveRasterKey {
    identity: WaveFileIdentity,
    window_start_micros: i64,
    window_end_micros: i64,
    bin_micros: u64,
    pixel_width: usize,
    pixel_height: usize,
}

impl WaveRasterKey {
    fn new(
        identity: WaveFileIdentity,
        window: WaveWindow,
        bin_secs: f64,
        pixel_width: usize,
        pixel_height: usize,
    ) -> Self {
        Self {
            identity,
            window_start_micros: (window.start_secs * 1_000_000.0).round() as i64,
            window_end_micros: (window.end_secs * 1_000_000.0).round() as i64,
            bin_micros: (bin_secs * 1_000_000.0).round().max(1.0) as u64,
            pixel_width,
            pixel_height,
        }
    }
}

#[derive(Clone)]
pub(crate) struct WaveRaster {
    pub(crate) window_start_secs: f64,
    pub(crate) window_end_secs: f64,
    pub(crate) bin_secs: f64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Arc<Vec<u8>>,
}

struct WaveRasterLru {
    max_entries: usize,
    entries: VecDeque<(WaveRasterKey, Arc<WaveRaster>)>,
}

impl WaveRasterLru {
    fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            entries: VecDeque::new(),
        }
    }

    fn get(&mut self, key: &WaveRasterKey) -> Option<Arc<WaveRaster>> {
        let position = self
            .entries
            .iter()
            .position(|(candidate, _)| candidate == key)?;
        let entry = self.entries.remove(position)?;
        let raster = Arc::clone(&entry.1);
        self.entries.push_front(entry);
        Some(raster)
    }

    fn insert(&mut self, key: WaveRasterKey, raster: Arc<WaveRaster>) {
        if self.max_entries == 0 {
            return;
        }
        if let Some(position) = self
            .entries
            .iter()
            .position(|(candidate, _)| *candidate == key)
        {
            self.entries.remove(position);
        }
        self.entries.push_front((key, raster));
        self.entries.truncate(self.max_entries);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WaveWorkerStatus {
    Idle,
    Working,
    NoAudioTrack,
    Failed(String),
    Cancelled,
    ThreadSpawnFailed(String),
}

#[derive(Clone)]
pub(crate) struct WaveWorkerSnapshot {
    pub(crate) status: WaveWorkerStatus,
    pub(crate) raster: Option<Arc<WaveRaster>>,
}

struct WaveRequest {
    id: u64,
    signature: WaveRequestSignature,
    completed_analysis: Option<CompletedTimelineAnalysis>,
}

struct WaveSharedState {
    latest_request_id: u64,
    status: WaveWorkerStatus,
    raster: Option<Arc<WaveRaster>>,
}

fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(crate) struct SeekStripWaveWorker {
    pending: Arc<Mutex<Option<WaveRequest>>>,
    wake_tx: Sender<()>,
    state: Arc<Mutex<WaveSharedState>>,
    cancel: Arc<AtomicBool>,
    latest_request_id: Arc<AtomicU64>,
    next_request_id: AtomicU64,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SeekStripWaveWorker {
    pub(crate) fn spawn(path: PathBuf) -> Self {
        let (wake_tx, wake_rx) = bounded::<()>(1);
        let pending = Arc::new(Mutex::new(None));
        let state = Arc::new(Mutex::new(WaveSharedState {
            latest_request_id: 0,
            status: WaveWorkerStatus::Idle,
            raster: None,
        }));
        let cancel = Arc::new(AtomicBool::new(false));
        let latest_request_id = Arc::new(AtomicU64::new(0));
        let worker_pending = Arc::clone(&pending);
        let worker_state = Arc::clone(&state);
        let worker_cancel = Arc::clone(&cancel);
        let worker_latest = Arc::clone(&latest_request_id);
        let thread_result = std::thread::Builder::new()
            .name("video-seek-strip-wave".into())
            .spawn(move || {
                let identity = WaveFileIdentity::from_file(&path);
                let mut runtime = WaveWorkerRuntime::new();
                while !worker_cancel.load(Ordering::Acquire) {
                    let request = lock_recover(&worker_pending).take();
                    if let Some(request) = request {
                        process_wave_request(
                            &path,
                            &identity,
                            request,
                            &worker_state,
                            &worker_cancel,
                            &worker_latest,
                            &mut runtime,
                        );
                        continue;
                    }
                    if wake_rx.recv().is_err() {
                        break;
                    }
                }
                if worker_cancel.load(Ordering::Acquire) {
                    let mut shared = lock_recover(&worker_state);
                    shared.status = WaveWorkerStatus::Cancelled;
                    shared.raster = None;
                }
            });
        let thread = match thread_result {
            Ok(thread) => Some(thread),
            Err(error) => {
                lock_recover(&state).status =
                    WaveWorkerStatus::ThreadSpawnFailed(error.to_string());
                None
            }
        };
        Self {
            pending,
            wake_tx,
            state,
            cancel,
            latest_request_id,
            next_request_id: AtomicU64::new(1),
            thread,
        }
    }

    pub(crate) fn request(
        &self,
        signature: WaveRequestSignature,
        completed_analysis: Option<CompletedTimelineAnalysis>,
    ) -> Option<u64> {
        if self.cancel.load(Ordering::Acquire) {
            return None;
        }
        let id = self.next_request_id.fetch_add(1, Ordering::AcqRel);
        self.latest_request_id.store(id, Ordering::Release);
        {
            let mut shared = lock_recover(&self.state);
            shared.latest_request_id = id;
            shared.status = WaveWorkerStatus::Working;
            shared.raster = None;
        }
        *lock_recover(&self.pending) = Some(WaveRequest {
            id,
            signature,
            completed_analysis,
        });
        let _ = self.wake_tx.try_send(());
        Some(id)
    }

    pub(crate) fn snapshot(&self) -> WaveWorkerSnapshot {
        let shared = lock_recover(&self.state);
        WaveWorkerSnapshot {
            status: shared.status.clone(),
            raster: shared.raster.clone(),
        }
    }

    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        self.latest_request_id.fetch_add(1, Ordering::AcqRel);
        let _ = self.wake_tx.try_send(());
    }
}

impl Drop for SeekStripWaveWorker {
    fn drop(&mut self) {
        self.cancel();
        let _ = self.thread.take();
    }
}

struct WaveWorkerRuntime {
    decoder: Option<crate::audio_decode::AudioRangeDecoder>,
    decoder_open_error: Option<crate::audio_decode::AudioDecodeOpenError>,
    lru: WaveRasterLru,
}

impl WaveWorkerRuntime {
    fn new() -> Self {
        Self {
            decoder: None,
            decoder_open_error: None,
            lru: WaveRasterLru::new(WAVE_RASTER_LRU_MAX_ENTRIES),
        }
    }
}

fn request_is_stale(id: u64, cancel: &AtomicBool, latest_request_id: &AtomicU64) -> bool {
    cancel.load(Ordering::Acquire) || latest_request_id.load(Ordering::Acquire) != id
}

fn process_wave_request(
    path: &Path,
    identity: &WaveFileIdentity,
    request: WaveRequest,
    state: &Mutex<WaveSharedState>,
    cancel: &AtomicBool,
    latest_request_id: &AtomicU64,
    runtime: &mut WaveWorkerRuntime,
) {
    let total_t0 = Instant::now();
    let signature = request.signature;
    let Some(window) = waveform_window(
        signature.center_time_secs(),
        signature.duration_secs(),
        DEFAULT_WAVEFORM_SPAN_SECS,
    ) else {
        publish_wave_failure(
            state,
            request.id,
            latest_request_id,
            "invalid waveform window",
        );
        return;
    };
    let bin_secs = waveform_bin_secs(DEFAULT_WAVEFORM_SPAN_SECS, signature.pixel_width);
    let key = WaveRasterKey::new(
        identity.clone(),
        window,
        bin_secs,
        signature.pixel_width,
        signature.pixel_height,
    );
    if let Some(raster) = runtime.lru.get(&key) {
        publish_wave_raster(state, request.id, latest_request_id, raster);
        emit_wave_perf(
            "memory_lru",
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
            total_t0.elapsed(),
            bin_secs,
            signature.pixel_width,
        );
        return;
    }

    let mut decode_elapsed = Duration::ZERO;
    let mut analyze_elapsed = Duration::ZERO;
    let mut source = "full_analysis";
    let analysis_and_range = if let Some(completed) = request
        .completed_analysis
        .filter(|completed| completed.identity == *identity)
    {
        let range = waveform_bin_range(
            &completed.analysis.bins,
            window.content_start_secs,
            window.content_end_secs,
        );
        Some((completed.analysis, range))
    } else if let Some(analysis_range) =
        waveform_analysis_range(window, signature.duration_secs(), bin_secs)
    {
        source = "range_decode";
        if runtime.decoder.is_none() && runtime.decoder_open_error.is_none() {
            match crate::audio_decode::AudioRangeDecoder::open(path) {
                Ok(decoder) => runtime.decoder = Some(decoder),
                Err(error) => runtime.decoder_open_error = Some(error),
            }
        }
        let Some(decoder) = runtime.decoder.as_mut() else {
            match runtime.decoder_open_error.as_ref() {
                Some(crate::audio_decode::AudioDecodeOpenError::NoAudioTrack) => {
                    publish_wave_no_audio_track(state, request.id, latest_request_id);
                }
                Some(crate::audio_decode::AudioDecodeOpenError::Failed(error)) => {
                    publish_wave_failure(state, request.id, latest_request_id, error);
                }
                None => publish_wave_failure(
                    state,
                    request.id,
                    latest_request_id,
                    "audio decoder unavailable",
                ),
            }
            return;
        };
        let decode_t0 = Instant::now();
        let decoded = match decoder.decode_range_to_stereo_f32(
            analysis_range.start_secs,
            analysis_range.end_secs,
            &|| request_is_stale(request.id, cancel, latest_request_id),
        ) {
            Ok(decoded) => decoded,
            Err(error) if error == "cancelled" => return,
            Err(error) => {
                publish_wave_failure(state, request.id, latest_request_id, &error);
                return;
            }
        };
        decode_elapsed = decode_t0.elapsed();
        let analyze_t0 = Instant::now();
        let analysis = analyze_window_samples(
            &decoded.stereo_samples,
            decoded.info.sample_rate,
            analysis_range.start_secs,
            window.content_start_secs,
            window.content_end_secs,
            bin_secs,
            &|| request_is_stale(request.id, cancel, latest_request_id),
        )
        .map(Arc::new);
        analyze_elapsed = analyze_t0.elapsed();
        let Some(analysis) = analysis else {
            return;
        };
        let range = 0..analysis.bins.len();
        Some((analysis, range))
    } else {
        None
    };

    if request_is_stale(request.id, cancel, latest_request_id) {
        return;
    }
    let bins = analysis_and_range
        .as_ref()
        .map(|(analysis, range)| &analysis.bins[range.clone()])
        .unwrap_or(&[]);
    let raster_t0 = Instant::now();
    let (image, _) = crate::ui_music_timeline::render_timeline_row_image(
        window.start_secs,
        DEFAULT_WAVEFORM_SPAN_SECS,
        bins,
        signature.pixel_width,
        signature.pixel_height,
        0,
        0,
        true,
    );
    let raster_elapsed = raster_t0.elapsed();
    let mut rgba = Vec::with_capacity(image.pixels.len().saturating_mul(4));
    for pixel in image.pixels {
        rgba.extend_from_slice(&pixel.to_srgba_unmultiplied());
    }
    let raster = Arc::new(WaveRaster {
        window_start_secs: window.start_secs,
        window_end_secs: window.end_secs,
        bin_secs,
        width: signature.pixel_width as u32,
        height: signature.pixel_height as u32,
        rgba: Arc::new(rgba),
    });
    runtime.lru.insert(key, Arc::clone(&raster));
    publish_wave_raster(state, request.id, latest_request_id, raster);
    emit_wave_perf(
        source,
        decode_elapsed,
        analyze_elapsed,
        raster_elapsed,
        total_t0.elapsed(),
        bin_secs,
        signature.pixel_width,
    );
}

fn publish_wave_raster(
    state: &Mutex<WaveSharedState>,
    request_id: u64,
    latest_request_id: &AtomicU64,
    raster: Arc<WaveRaster>,
) {
    if latest_request_id.load(Ordering::Acquire) != request_id {
        return;
    }
    let mut shared = lock_recover(state);
    if shared.latest_request_id == request_id {
        shared.status = WaveWorkerStatus::Idle;
        shared.raster = Some(raster);
    }
}

fn publish_wave_failure(
    state: &Mutex<WaveSharedState>,
    request_id: u64,
    latest_request_id: &AtomicU64,
    error: &str,
) {
    if latest_request_id.load(Ordering::Acquire) != request_id {
        return;
    }
    let mut shared = lock_recover(state);
    if shared.latest_request_id == request_id {
        shared.status = WaveWorkerStatus::Failed(error.to_string());
        shared.raster = None;
    }
}

fn publish_wave_no_audio_track(
    state: &Mutex<WaveSharedState>,
    request_id: u64,
    latest_request_id: &AtomicU64,
) {
    if latest_request_id.load(Ordering::Acquire) != request_id {
        return;
    }
    let mut shared = lock_recover(state);
    if shared.latest_request_id == request_id {
        shared.status = WaveWorkerStatus::NoAudioTrack;
        shared.raster = None;
    }
}

fn emit_wave_perf(
    source: &str,
    decode: Duration,
    analyze: Duration,
    raster: Duration,
    total: Duration,
    bin_secs: f64,
    pixel_width: usize,
) {
    if crate::perf::is_enabled() {
        crate::perf::event(
            "video_strip",
            "wave_window",
            None,
            0,
            &[
                ("source", serde_json::Value::from(source)),
                (
                    "decode_ms",
                    serde_json::Value::from(decode.as_secs_f64() * 1000.0),
                ),
                (
                    "analyze_ms",
                    serde_json::Value::from(analyze.as_secs_f64() * 1000.0),
                ),
                (
                    "raster_ms",
                    serde_json::Value::from(raster.as_secs_f64() * 1000.0),
                ),
                (
                    "total_ms",
                    serde_json::Value::from(total.as_secs_f64() * 1000.0),
                ),
                ("bin_secs", serde_json::Value::from(bin_secs)),
                ("pixel_width", serde_json::Value::from(pixel_width as u64)),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin(start: f64, duration: f64) -> WaveformBin {
        WaveformBin {
            start_secs: start,
            duration_secs: duration,
            ..WaveformBin::default()
        }
    }

    fn key(name: &str) -> WaveRasterKey {
        WaveRasterKey {
            identity: WaveFileIdentity {
                normalized_path: name.to_string(),
                size: 1,
                mtime: 1,
            },
            window_start_micros: 0,
            window_end_micros: 1,
            bin_micros: 1,
            pixel_width: 1,
            pixel_height: 1,
        }
    }

    fn raster(value: u8) -> Arc<WaveRaster> {
        Arc::new(WaveRaster {
            window_start_secs: 0.0,
            window_end_secs: 1.0,
            bin_secs: 0.1,
            width: 1,
            height: 1,
            rgba: Arc::new(vec![value, 0, 0, 255]),
        })
    }

    #[test]
    fn bin_window_and_pre_roll_discard_use_half_open_boundaries() {
        let bins = vec![bin(0.0, 0.5), bin(0.5, 0.5), bin(1.0, 0.5), bin(1.5, 0.5)];
        assert_eq!(waveform_bin_range(&bins, 0.5, 1.5), 1..3);
        assert_eq!(waveform_bin_range(&bins, 1.0, 2.0), 2..4);
        assert_eq!(waveform_bin_range(&bins, 2.0, 3.0), 4..4);
    }

    #[test]
    fn analysis_range_adds_only_aligned_pre_roll_before_visible_content() {
        let middle = WaveWindow {
            start_secs: 10.0,
            end_secs: 20.0,
            content_start_secs: 10.0,
            content_end_secs: 20.0,
        };
        assert_eq!(
            waveform_analysis_range(middle, 30.0, 0.050),
            Some(WaveAnalysisRange {
                start_secs: 9.25,
                end_secs: 20.0,
            })
        );

        let beginning = WaveWindow {
            start_secs: -5.0,
            end_secs: 5.0,
            content_start_secs: 0.0,
            content_end_secs: 5.0,
        };
        assert_eq!(
            waveform_analysis_range(beginning, 30.0, 0.050),
            Some(WaveAnalysisRange {
                start_secs: 0.0,
                end_secs: 5.0,
            })
        );
    }

    #[test]
    fn bin_density_is_pixel_bounded_and_stable() {
        assert_eq!(waveform_bin_secs(60.0, 1_000), 0.060);
        assert_eq!(waveform_bin_secs(60.0, 4_000), 0.015);
        assert_eq!(waveform_bin_secs(1.0, 4_000), MIN_WAVEFORM_BIN_SECS);
        assert_eq!(waveform_bin_secs(60.0, 0), MIN_WAVEFORM_BIN_SECS);
    }

    #[test]
    fn raster_lru_evicts_oldest_and_promotes_hits() {
        let mut lru = WaveRasterLru::new(2);
        lru.insert(key("a"), raster(1));
        lru.insert(key("b"), raster(2));
        assert!(lru.get(&key("a")).is_some());
        lru.insert(key("c"), raster(3));
        assert!(lru.get(&key("b")).is_none());
        assert!(lru.get(&key("a")).is_some());
        assert!(lru.get(&key("c")).is_some());
    }

    #[test]
    fn standalone_window_matches_full_analysis_after_pre_roll() {
        const RATE: u32 = 48_000;
        let duration_secs = 12.0;
        let frames = (duration_secs * RATE as f64) as usize;
        let mut samples = Vec::with_capacity(frames * 2);
        for frame in 0..frames {
            let t = frame as f64 / RATE as f64;
            let envelope = 0.25 + 0.18 * (t * 0.7).sin().abs();
            let left = (std::f64::consts::TAU * 173.0 * t).sin() * envelope;
            let right = (std::f64::consts::TAU * 311.0 * t).sin() * envelope * 0.8;
            samples.push(left as f32);
            samples.push(right as f32);
        }
        let config = AnalysisConfig {
            bin_secs: 0.050,
            row_secs: DEFAULT_WAVEFORM_SPAN_SECS,
            ..AnalysisConfig::default()
        };
        let full = music_core::analyze_stereo_timeline(&samples, RATE, config);
        let visible_start = 5.0;
        let visible_end = 9.0;
        let analysis_start = visible_start - WAVEFORM_PRE_ROLL_SECS;
        let start_frame = (analysis_start * RATE as f64).round() as usize;
        let end_frame = (visible_end * RATE as f64).round() as usize;
        let window = analyze_window_samples(
            &samples[start_frame * 2..end_frame * 2],
            RATE,
            analysis_start,
            visible_start,
            visible_end,
            config.bin_secs,
            &|| false,
        )
        .expect("window analysis");
        let full_range = waveform_bin_range(&full.bins, visible_start, visible_end);
        let full_bins = &full.bins[full_range];
        assert_eq!(full_bins.len(), window.bins.len());
        for (full_bin, window_bin) in full_bins.iter().zip(&window.bins) {
            assert!((full_bin.start_secs - window_bin.start_secs).abs() < 1.0e-9);
            assert!((full_bin.peak_l - window_bin.peak_l).abs() < 1.0e-6);
            assert!((full_bin.rms_l - window_bin.rms_l).abs() < 1.0e-6);
            assert!((full_bin.peak_r - window_bin.peak_r).abs() < 1.0e-6);
            assert!((full_bin.rms_r - window_bin.rms_r).abs() < 1.0e-6);
            for band in 0..3 {
                assert!((full_bin.band_energy[band] - window_bin.band_energy[band]).abs() < 0.002);
            }
            assert!((full_bin.transient - window_bin.transient).abs() < 0.002);
        }
        assert!(window.beat_grid.beats.is_empty());
    }
}
