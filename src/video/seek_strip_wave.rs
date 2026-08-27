//! Window-on-demand waveform analysis for the native video seek strip.
//!
//! The worker owns one lazy-opened audio range decoder for the current video.
//! It first reuses a completed full TimelineAnalysis when the file identity
//! matches; otherwise it decodes the requested span plus pre-roll. The first request is one visible
//! screen, followed by a same-scale wider upgrade. Rasterization also runs here, and only RGBA plus
//! its time coverage crosses to the App and native presenter. The cache is process-memory-only by
//! design.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, bounded};
use music_core::{AnalysisConfig, TimelineAnalysis, WaveformBin};

/// One strip width in waveform mode.
#[cfg(test)]
pub(crate) const DEFAULT_WAVEFORM_SPAN_SECS: f64 =
    crate::settings::VIDEO_SEEK_STRIP_WAVEFORM_SPAN_DEFAULT_SECS;
/// Keep three visible screens while that remains modest, but never add background coverage beyond
/// one hour. A visible span of one hour or more is decoded at exactly its visible width, without a
/// redundant second-stage request.
const WAVEFORM_RETAINED_SPAN_MULTIPLIER: f64 = 3.0;
/// 波形テクスチャ 1 枚の最大幅 (物理ピクセル)。`crate::app::MAX_TEXTURE_DIM` と同じ
/// wgpu の既定上限。これを超えると `Device::create_texture` がパニックする。
const MAX_WAVEFORM_TEXTURE_WIDTH: usize = 8192;
pub(crate) const MAX_WAVEFORM_RETAINED_SPAN_SECS: f64 = 3600.0;
/// Start a replacement while one quarter of a visible screen remains beyond the visible edge.
const WAVEFORM_REQUEST_MARGIN_RATIO: f64 = 0.25;
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WaveSpan {
    pub(crate) start_secs: f64,
    pub(crate) end_secs: f64,
}

impl WaveSpan {
    pub(crate) fn centered(center_time_secs: f64, span_secs: f64) -> Option<Self> {
        if !center_time_secs.is_finite() || !span_secs.is_finite() || span_secs <= 0.0 {
            return None;
        }
        let start_secs = center_time_secs - span_secs * 0.5;
        Some(Self {
            start_secs,
            end_secs: start_secs + span_secs,
        })
    }

    fn contains_center_with_margin(
        self,
        center_time_secs: f64,
        visible_span_secs: f64,
        margin_secs: f64,
    ) -> bool {
        let inset = visible_span_secs * 0.5 + margin_secs;
        center_time_secs >= self.start_secs + inset && center_time_secs <= self.end_secs - inset
    }

    fn contains_visible_center(self, center_time_secs: f64, visible_span_secs: f64) -> bool {
        self.contains_center_with_margin(center_time_secs, visible_span_secs, 0.0)
    }

    fn overlaps(self, other: Self) -> bool {
        self.start_secs < other.end_secs && other.start_secs < self.end_secs
    }

    pub(crate) fn duration_secs(self) -> f64 {
        self.end_secs - self.start_secs
    }

    pub(crate) fn center_secs(self) -> f64 {
        (self.start_secs + self.end_secs) * 0.5
    }

    pub(crate) fn matches_window(self, window_start_secs: f64, window_end_secs: f64) -> bool {
        (self.start_secs - window_start_secs).abs() <= 1.0e-6
            && (self.end_secs - window_end_secs).abs() <= 1.0e-6
    }

    fn has_duration(self, expected_secs: f64) -> bool {
        (self.duration_secs() - expected_secs).abs() <= 1.0e-6
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaveRequestStage {
    FirstPaint,
    Wide,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WaveSpanRequest {
    pub(crate) span: WaveSpan,
    pub(crate) stage: WaveRequestStage,
}

impl WaveSpanRequest {
    fn first_paint(center_time_secs: f64, visible_span_secs: f64) -> Option<Self> {
        Some(Self {
            span: WaveSpan::centered(center_time_secs, visible_span_secs)?,
            stage: WaveRequestStage::FirstPaint,
        })
    }

    fn wide(center_time_secs: f64, visible_span_secs: f64) -> Option<Self> {
        Some(Self {
            span: WaveSpan::centered(
                center_time_secs,
                waveform_retained_span_secs(visible_span_secs)?,
            )?,
            stage: WaveRequestStage::Wide,
        })
    }

    /// 1 枚のテクスチャに焼く波形の幅 (物理ピクセル)。
    ///
    /// ⚠ **GPU のテクスチャ上限で頭打ちにする。** 先読みぶんを掛けた幅をそのまま
    /// 要求すると、4K 幅のストリップ (3840px) × `WAVEFORM_RETAINED_SPAN_MULTIPLIER`
    /// = 11520px となり、wgpu の 8192 制限を超えて `Device::create_texture` が
    /// パニックする。**パニックしたレンダースレッドは戻らないので、以後どの動画も
    /// 再生できなくなる** (2026-08-27 利用者報告。同じ形は 2026-08-10 / 08-14 の
    /// panic.log にも 8320 / 9000 で残っている)。
    ///
    /// 幅を丸めても時間 ↔ ピクセルの対応は比率で決まるため、位置はずれない
    /// (bin が粗くなるだけ)。**可視幅より狭くはしない**: 可視部分の解像度を
    /// 下げてまで先読みを優先する理由はない。
    pub(crate) fn pixel_width(self, visible_pixel_width: usize, visible_span_secs: f64) -> usize {
        if visible_pixel_width == 0 || !visible_span_secs.is_finite() || visible_span_secs <= 0.0 {
            return 0;
        }
        let ratio = (self.span.duration_secs() / visible_span_secs).max(1.0);
        ((visible_pixel_width as f64 * ratio).round() as usize)
            .max(visible_pixel_width)
            .min(MAX_WAVEFORM_TEXTURE_WIDTH.max(visible_pixel_width))
    }
}

pub(crate) fn waveform_retained_span_secs(visible_span_secs: f64) -> Option<f64> {
    if !visible_span_secs.is_finite() || visible_span_secs <= 0.0 {
        return None;
    }
    Some(
        (visible_span_secs * WAVEFORM_RETAINED_SPAN_MULTIPLIER)
            .min(MAX_WAVEFORM_RETAINED_SPAN_SECS)
            .max(visible_span_secs),
    )
}

/// Choose the visible-first request, its same-centred wide upgrade, and later edge replacements.
///
/// A first-paint request is not chased for small playback/drag movements while it is in flight;
/// it is replaced only after the current visible range no longer overlaps it. Once a wide request
/// exists, its full-coverage band is the hysteresis latch used by steady scrolling.
pub(crate) fn decide_waveform_span_request(
    center_time_secs: f64,
    visible_span_secs: f64,
    displayed: Option<WaveSpan>,
    pending: Option<WaveSpan>,
) -> Option<WaveSpanRequest> {
    if !center_time_secs.is_finite() || !visible_span_secs.is_finite() || visible_span_secs <= 0.0 {
        return None;
    }
    let visible = WaveSpan::centered(center_time_secs, visible_span_secs)?;
    let retained_span_secs = waveform_retained_span_secs(visible_span_secs)?;
    let has_wide_upgrade = retained_span_secs > visible_span_secs + 1.0e-6;
    if let Some(pending) = pending {
        if pending.has_duration(visible_span_secs) {
            return if pending.overlaps(visible) {
                None
            } else {
                WaveSpanRequest::first_paint(center_time_secs, visible_span_secs)
            };
        }
        if !pending.has_duration(retained_span_secs) {
            return WaveSpanRequest::first_paint(center_time_secs, visible_span_secs);
        }
        if pending.contains_visible_center(center_time_secs, visible_span_secs) {
            return None;
        }
        return if displayed.is_some_and(|span| {
            (span.has_duration(visible_span_secs) || span.has_duration(retained_span_secs))
                && span.overlaps(visible)
        }) {
            WaveSpanRequest::wide(center_time_secs, visible_span_secs)
        } else {
            WaveSpanRequest::first_paint(center_time_secs, visible_span_secs)
        };
    }

    let Some(displayed) = displayed else {
        return WaveSpanRequest::first_paint(center_time_secs, visible_span_secs);
    };
    if displayed.has_duration(visible_span_secs) {
        return if displayed.overlaps(visible) && has_wide_upgrade {
            WaveSpanRequest::wide(displayed.center_secs(), visible_span_secs)
        } else if !has_wide_upgrade
            && (displayed.center_secs() - center_time_secs).abs()
                <= visible_span_secs * WAVEFORM_REQUEST_MARGIN_RATIO
        {
            None
        } else {
            WaveSpanRequest::first_paint(center_time_secs, visible_span_secs)
        };
    }
    if !displayed.has_duration(retained_span_secs) {
        // A preference change can leave the previous image on screen as a holdover. It is not
        // request coverage for the new scale, so rebuild the new first paint immediately.
        return WaveSpanRequest::first_paint(center_time_secs, visible_span_secs);
    }
    if displayed.contains_center_with_margin(
        center_time_secs,
        visible_span_secs,
        visible_span_secs * WAVEFORM_REQUEST_MARGIN_RATIO,
    ) {
        return None;
    }
    WaveSpanRequest::wide(center_time_secs, visible_span_secs)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WaveTextureSlice {
    pub(crate) destination_start: f32,
    pub(crate) destination_end: f32,
    pub(crate) texture_start: f32,
    pub(crate) texture_end: f32,
}

/// Map the overlap between the visible one-screen range and a retained wider raster to normalized
/// destination/texture coordinates. A partial overlap draws the old raster only where it is valid;
/// `None` is the unavoidable full gap after a jump beyond all analyzed coverage.
pub(crate) fn waveform_texture_slice(
    center_time_secs: f64,
    visible_span_secs: f64,
    raster_start_secs: f64,
    raster_end_secs: f64,
) -> Option<WaveTextureSlice> {
    if !center_time_secs.is_finite()
        || !visible_span_secs.is_finite()
        || visible_span_secs <= 0.0
        || !raster_start_secs.is_finite()
        || !raster_end_secs.is_finite()
        || raster_end_secs <= raster_start_secs
    {
        return None;
    }
    let visible_start = center_time_secs - visible_span_secs * 0.5;
    let visible_end = visible_start + visible_span_secs;
    let overlap_start = visible_start.max(raster_start_secs);
    let overlap_end = visible_end.min(raster_end_secs);
    if overlap_end <= overlap_start {
        return None;
    }
    Some(WaveTextureSlice {
        destination_start: ((overlap_start - visible_start) / visible_span_secs) as f32,
        destination_end: ((overlap_end - visible_start) / visible_span_secs) as f32,
        texture_start: ((overlap_start - raster_start_secs) / (raster_end_secs - raster_start_secs))
            as f32,
        texture_end: ((overlap_end - raster_start_secs) / (raster_end_secs - raster_start_secs))
            as f32,
    })
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
    row_secs: f64,
    should_abort: &dyn Fn() -> bool,
) -> Option<TimelineAnalysis> {
    let mut analysis = music_core::analyze_stereo_waveform_cancellable(
        stereo_samples,
        sample_rate,
        AnalysisConfig {
            bin_secs,
            row_secs,
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
    span_bits: u64,
    visible_span_bits: u64,
    pixel_width: usize,
    pixel_height: usize,
}

impl WaveRequestSignature {
    pub(crate) fn new(
        center_time_secs: f64,
        duration_secs: f64,
        span_secs: f64,
        visible_span_secs: f64,
        pixel_width: usize,
        pixel_height: usize,
    ) -> Option<Self> {
        if !center_time_secs.is_finite()
            || !duration_secs.is_finite()
            || duration_secs <= 0.0
            || !span_secs.is_finite()
            || span_secs <= 0.0
            || !visible_span_secs.is_finite()
            || visible_span_secs <= 0.0
            || pixel_width == 0
            || pixel_height == 0
        {
            return None;
        }
        Some(Self {
            center_bits: center_time_secs.to_bits(),
            duration_bits: duration_secs.to_bits(),
            span_bits: span_secs.to_bits(),
            visible_span_bits: visible_span_secs.to_bits(),
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

    fn span_secs(self) -> f64 {
        f64::from_bits(self.span_bits)
    }

    fn visible_span_secs(self) -> f64 {
        f64::from_bits(self.visible_span_bits)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct WaveRasterKey {
    identity: WaveFileIdentity,
    window_start_micros: i64,
    window_end_micros: i64,
    bin_micros: u64,
    visible_span_micros: u64,
    pixel_width: usize,
    pixel_height: usize,
}

impl WaveRasterKey {
    fn new(
        identity: WaveFileIdentity,
        window: WaveWindow,
        bin_secs: f64,
        visible_span_secs: f64,
        pixel_width: usize,
        pixel_height: usize,
    ) -> Self {
        Self {
            identity,
            window_start_micros: (window.start_secs * 1_000_000.0).round() as i64,
            window_end_micros: (window.end_secs * 1_000_000.0).round() as i64,
            bin_micros: (bin_secs * 1_000_000.0).round().max(1.0) as u64,
            visible_span_micros: (visible_span_secs * 1_000_000.0).round().max(1.0) as u64,
            pixel_width,
            pixel_height,
        }
    }
}

#[derive(Clone)]
pub(crate) struct WaveRaster {
    pub(crate) window_start_secs: f64,
    pub(crate) window_end_secs: f64,
    pub(crate) visible_span_secs: f64,
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

fn mark_wave_request_working(shared: &mut WaveSharedState, request_id: u64) {
    shared.latest_request_id = request_id;
    shared.status = WaveWorkerStatus::Working;
    // The displayed raster is deliberately retained until the replacement is published.
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
            mark_wave_request_working(&mut shared, id);
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
        signature.span_secs(),
    ) else {
        publish_wave_failure(
            state,
            request.id,
            latest_request_id,
            "invalid waveform window",
        );
        return;
    };
    let bin_secs = waveform_bin_secs(signature.span_secs(), signature.pixel_width);
    let key = WaveRasterKey::new(
        identity.clone(),
        window,
        bin_secs,
        signature.visible_span_secs(),
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
            signature.span_secs(),
            signature.visible_span_secs(),
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
            signature.span_secs(),
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
        signature.span_secs(),
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
        visible_span_secs: signature.visible_span_secs(),
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
        signature.span_secs(),
        signature.visible_span_secs(),
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
    }
}

fn emit_wave_perf(
    source: &str,
    decode: Duration,
    analyze: Duration,
    raster: Duration,
    total: Duration,
    bin_secs: f64,
    raster_span_secs: f64,
    visible_span_secs: f64,
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
                (
                    "raster_span_secs",
                    serde_json::Value::from(raster_span_secs),
                ),
                (
                    "visible_span_secs",
                    serde_json::Value::from(visible_span_secs),
                ),
                ("pixel_width", serde_json::Value::from(pixel_width as u64)),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "manual real-media first-paint measurement"]
    fn measure_app_worker_first_paint_from_env() {
        let path = std::env::var_os("MIV_WAVE_BENCH_PATH")
            .map(PathBuf::from)
            .expect("set MIV_WAVE_BENCH_PATH to a real video");
        let duration_secs: f64 = std::env::var("MIV_WAVE_BENCH_DURATION_SECS")
            .expect("set MIV_WAVE_BENCH_DURATION_SECS")
            .parse()
            .expect("duration must be seconds");
        let log_path = std::env::var_os("MIV_WAVE_BENCH_LOG")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("miv-wave-first-paint.jsonl"));
        crate::perf::init_with_path(true, None, Some(log_path));

        const PIXEL_WIDTH: usize = 1920;
        const PIXEL_HEIGHT: usize = 94;
        let center_time_secs = duration_secs * 0.5;
        for visible_span_secs in [60.0, 600.0, 1800.0, 3600.0, 7200.0, 10_800.0] {
            let worker = SeekStripWaveWorker::spawn(path.clone());
            let signature = WaveRequestSignature::new(
                center_time_secs,
                duration_secs,
                visible_span_secs,
                visible_span_secs,
                PIXEL_WIDTH,
                PIXEL_HEIGHT,
            )
            .expect("valid request");
            let started = Instant::now();
            worker.request(signature, None).expect("worker request");
            loop {
                let snapshot = worker.snapshot();
                match snapshot.status {
                    WaveWorkerStatus::Idle if snapshot.raster.is_some() => {
                        let raster = snapshot.raster.unwrap();
                        println!(
                            "first-paint span={visible_span_secs:.0}s total_ms={:.1} bin_secs={:.3} bins_at_most={}",
                            started.elapsed().as_secs_f64() * 1000.0,
                            raster.bin_secs,
                            (visible_span_secs / raster.bin_secs).ceil() as usize,
                        );
                        break;
                    }
                    WaveWorkerStatus::NoAudioTrack => panic!("no audio track"),
                    WaveWorkerStatus::Failed(error)
                    | WaveWorkerStatus::ThreadSpawnFailed(error) => panic!("{error}"),
                    WaveWorkerStatus::Cancelled => panic!("worker cancelled"),
                    WaveWorkerStatus::Idle | WaveWorkerStatus::Working => {}
                }
                assert!(
                    started.elapsed() < Duration::from_secs(180),
                    "first paint timed out"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        crate::perf::flush();
    }

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
            visible_span_micros: 1,
            pixel_width: 1,
            pixel_height: 1,
        }
    }

    fn raster(value: u8) -> Arc<WaveRaster> {
        Arc::new(WaveRaster {
            window_start_secs: 0.0,
            window_end_secs: 1.0,
            visible_span_secs: DEFAULT_WAVEFORM_SPAN_SECS,
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

    /// 先読みぶんを掛けた幅が GPU のテクスチャ上限を超えてはいけない。超えると
    /// `Device::create_texture` がパニックし、**レンダースレッドが死んで以後どの動画も
    /// 再生できなくなる** (2026-08-27 利用者報告。4K 幅 3840 × 3 = 11520 で発生)。
    #[test]
    fn a_wide_upgrade_never_asks_for_a_texture_past_the_gpu_limit() {
        let visible_span_secs = 30.0;
        // 4K のストリップ全幅。報告された 11520 はこの値から出ている。
        let visible_width = 3_840;
        let upgrade = WaveSpanRequest {
            span: WaveSpan::centered(
                100.0,
                waveform_retained_span_secs(visible_span_secs).expect("retained span"),
            )
            .expect("span"),
            stage: WaveRequestStage::Wide,
        };
        let width = upgrade.pixel_width(visible_width, visible_span_secs);
        assert!(
            width <= MAX_WAVEFORM_TEXTURE_WIDTH,
            "requested {width}px exceeds the {MAX_WAVEFORM_TEXTURE_WIDTH}px GPU limit"
        );
        assert!(
            width >= visible_width,
            "the visible part must not lose resolution to make room for the look-ahead"
        );
    }

    /// 上限に収まる範囲では従来どおり先読みぶんを掛ける (丸めが常時効いてしまわないこと)。
    #[test]
    fn a_wide_upgrade_still_pre_renders_when_it_fits() {
        let visible_span_secs = 30.0;
        let upgrade = WaveSpanRequest {
            span: WaveSpan::centered(
                100.0,
                waveform_retained_span_secs(visible_span_secs).expect("retained span"),
            )
            .expect("span"),
            stage: WaveRequestStage::Wide,
        };
        assert_eq!(upgrade.pixel_width(1_000, visible_span_secs), 3_000);
    }

    /// 可視幅そのものが上限を超える環境 (超広幅ウィンドウ) でも、可視解像度は落とさない。
    /// ここで丸めると波形が可視部分でぼやける。テクスチャ生成側の責務として残す。
    #[test]
    fn an_oversized_visible_width_is_not_reduced_below_itself() {
        let visible_span_secs = 30.0;
        let huge = MAX_WAVEFORM_TEXTURE_WIDTH + 2_000;
        let first = WaveSpanRequest {
            span: WaveSpan::centered(100.0, visible_span_secs).expect("span"),
            stage: WaveRequestStage::FirstPaint,
        };
        assert_eq!(first.pixel_width(huge, visible_span_secs), huge);
    }

    #[test]
    fn first_paint_is_visible_then_upgrades_wide_at_the_same_center_and_scale() {
        let visible_span_secs = DEFAULT_WAVEFORM_SPAN_SECS;
        assert_eq!(
            waveform_retained_span_secs(visible_span_secs),
            Some(visible_span_secs * 3.0)
        );
        let first = decide_waveform_span_request(100.0, visible_span_secs, None, None).unwrap();
        assert_eq!(first.stage, WaveRequestStage::FirstPaint);
        assert_eq!(
            first.span,
            WaveSpan::centered(100.0, visible_span_secs).unwrap()
        );
        assert_eq!(first.pixel_width(1_000, visible_span_secs), 1_000);

        // Small follow-playhead movement must not cancel and restart the fast first paint.
        assert_eq!(
            decide_waveform_span_request(100.1, visible_span_secs, None, Some(first.span)),
            None
        );

        let upgrade =
            decide_waveform_span_request(100.1, visible_span_secs, Some(first.span), None).unwrap();
        assert_eq!(upgrade.stage, WaveRequestStage::Wide);
        assert_eq!(upgrade.span.center_secs(), first.span.center_secs());
        assert_eq!(upgrade.span.duration_secs(), visible_span_secs * 3.0);
        assert_eq!(upgrade.pixel_width(1_000, visible_span_secs), 3_000);
        assert_eq!(
            waveform_bin_secs(
                first.span.duration_secs(),
                first.pixel_width(1_000, visible_span_secs)
            ),
            waveform_bin_secs(
                upgrade.span.duration_secs(),
                upgrade.pixel_width(1_000, visible_span_secs)
            )
        );
    }

    #[test]
    fn wide_raster_requests_at_hysteresis_boundary() {
        let visible_span_secs = 60.0;
        let displayed = WaveSpan::centered(
            100.0,
            waveform_retained_span_secs(visible_span_secs).unwrap(),
        )
        .unwrap();
        assert_eq!(
            decide_waveform_span_request(55.0, visible_span_secs, Some(displayed), None),
            None
        );
        let replacement =
            decide_waveform_span_request(54.999, visible_span_secs, Some(displayed), None).unwrap();
        assert_eq!(replacement.stage, WaveRequestStage::Wide);
        assert!((replacement.span.duration_secs() - 180.0).abs() < 1.0e-9);
        assert!(replacement.span.matches_window(
            replacement.span.start_secs + 0.5e-6,
            replacement.span.end_secs - 0.5e-6,
        ));

        // The in-flight replacement is the latch: returning across the old trigger does not
        // replace the request while its full visible range would cover the new centre.
        assert_eq!(
            decide_waveform_span_request(
                60.0,
                visible_span_secs,
                Some(displayed),
                Some(replacement.span)
            ),
            None
        );
        assert!(
            decide_waveform_span_request(
                145.0,
                visible_span_secs,
                Some(displayed),
                Some(replacement.span)
            )
            .is_some()
        );
    }

    #[test]
    fn thirty_minute_view_caps_retained_decode_at_one_hour_with_hysteresis() {
        let visible_span_secs = 1800.0;
        assert_eq!(
            waveform_retained_span_secs(visible_span_secs),
            Some(MAX_WAVEFORM_RETAINED_SPAN_SECS)
        );
        let displayed = WaveSpan::centered(4_000.0, MAX_WAVEFORM_RETAINED_SPAN_SECS).unwrap();
        assert_eq!(
            decide_waveform_span_request(4_450.0, visible_span_secs, Some(displayed), None),
            None
        );
        let replacement =
            decide_waveform_span_request(4_450.001, visible_span_secs, Some(displayed), None)
                .unwrap();
        assert_eq!(replacement.stage, WaveRequestStage::Wide);
        assert_eq!(replacement.span.duration_secs(), 3600.0);
        assert_eq!(replacement.pixel_width(1_920, visible_span_secs), 3_840);
    }

    #[test]
    fn hour_and_longer_views_decode_once_and_replace_after_quarter_span_motion() {
        for visible_span_secs in [3600.0, 7200.0, 10_800.0] {
            assert_eq!(
                waveform_retained_span_secs(visible_span_secs),
                Some(visible_span_secs)
            );
            let first =
                decide_waveform_span_request(20_000.0, visible_span_secs, None, None).unwrap();
            assert_eq!(first.stage, WaveRequestStage::FirstPaint);
            assert_eq!(first.span.duration_secs(), visible_span_secs);
            assert_eq!(
                decide_waveform_span_request(20_000.0, visible_span_secs, Some(first.span), None,),
                None,
                "a completed full-width raster must not request the same span again"
            );
            assert_eq!(
                decide_waveform_span_request(
                    20_000.0 + visible_span_secs * 0.25,
                    visible_span_secs,
                    Some(first.span),
                    None,
                ),
                None
            );
            let replacement = decide_waveform_span_request(
                20_000.0 + visible_span_secs * 0.25 + 0.001,
                visible_span_secs,
                Some(first.span),
                None,
            )
            .unwrap();
            assert_eq!(replacement.stage, WaveRequestStage::FirstPaint);
            assert_eq!(replacement.span.duration_secs(), visible_span_secs);
        }
    }

    #[test]
    fn changed_visible_span_treats_old_raster_only_as_a_display_holdover() {
        let old_retained = WaveSpan::centered(1000.0, 180.0).unwrap();
        let request =
            decide_waveform_span_request(1000.0, 600.0, Some(old_retained), None).unwrap();
        assert_eq!(request.stage, WaveRequestStage::FirstPaint);
        assert_eq!(request.span.duration_secs(), 600.0);
    }

    #[test]
    fn starting_replacement_keeps_the_displayed_raster() {
        let displayed = raster(7);
        let mut shared = WaveSharedState {
            latest_request_id: 1,
            status: WaveWorkerStatus::Idle,
            raster: Some(Arc::clone(&displayed)),
        };
        mark_wave_request_working(&mut shared, 2);
        assert_eq!(shared.latest_request_id, 2);
        assert_eq!(shared.status, WaveWorkerStatus::Working);
        assert!(
            shared
                .raster
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &displayed))
        );
    }

    #[test]
    fn retained_wave_texture_scrolls_by_uv_and_only_gaps_after_a_jump() {
        let centered = waveform_texture_slice(100.0, 60.0, 10.0, 190.0).unwrap();
        assert!((centered.destination_start - 0.0).abs() < f32::EPSILON);
        assert!((centered.destination_end - 1.0).abs() < f32::EPSILON);
        assert!((centered.texture_start - (1.0 / 3.0)).abs() < 1.0e-6);
        assert!((centered.texture_end - (2.0 / 3.0)).abs() < 1.0e-6);

        let scrolled = waveform_texture_slice(130.0, 60.0, 10.0, 190.0).unwrap();
        assert!((scrolled.texture_start - 0.5).abs() < 1.0e-6);
        assert!((scrolled.texture_end - (5.0 / 6.0)).abs() < 1.0e-6);

        let partial = waveform_texture_slice(200.0, 60.0, 10.0, 190.0).unwrap();
        assert!((partial.destination_start - 0.0).abs() < f32::EPSILON);
        assert!((partial.destination_end - (1.0 / 3.0)).abs() < 1.0e-6);
        assert_eq!(waveform_texture_slice(221.0, 60.0, 10.0, 190.0), None);
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
        for visible_span_secs in [60.0, 600.0, 1800.0, 3600.0, 7200.0, 10_800.0] {
            let visible_width = 1_920;
            let retained_span_secs = waveform_retained_span_secs(visible_span_secs).unwrap();
            let retained = WaveSpanRequest::wide(0.0, visible_span_secs).unwrap();
            let retained_width = retained.pixel_width(visible_width, visible_span_secs);
            let visible_bin_secs = waveform_bin_secs(visible_span_secs, visible_width);
            let retained_bin_secs = waveform_bin_secs(retained_span_secs, retained_width);
            assert_eq!(visible_bin_secs, retained_bin_secs);
            assert!((visible_span_secs / visible_bin_secs).ceil() as usize <= visible_width);
        }
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
            config.row_secs,
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
