//! Window-on-demand waveform analysis for the native video seek strip.
//!
//! The worker owns one lazy-opened audio range decoder for the current video.
//! It first reuses a completed full TimelineAnalysis when the file identity
//! matches; otherwise it decodes the requested span plus pre-roll. For spans from ten minutes it
//! also builds a worker-local 100ms full-length column in center-prioritized 60-second chunks; spans
//! over thirty minutes are served progressively from that column. The first request is one visible
//! screen, followed by a same-scale wider upgrade where window decode still applies. Rasterization
//! also runs here, and only versioned RGBA plus its time coverage crosses to the App and native
//! presenter. All caches are process-memory-only by design.

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
pub(crate) const COARSE_BIN_SECS: f64 = 0.100;
pub(crate) const COARSE_CHUNK_SECS: f64 = 60.0;
const COARSE_BINS_PER_CHUNK: usize = 600;
const WINDOW_DECODE_MAX_SPAN_SECS: f64 = 1800.0;
const COARSE_BUILD_MIN_SPAN_SECS: f64 = 600.0;

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

/// 実際に焼ける可視幅。GPU 上限を超える窓では、届く raster は可視ストリップより狭い。
///
/// 「可視範囲はもう描けているか」を問う側が生の可視幅と比べると、上限で頭打ちになった
/// raster は**永久に「まだ足りない」と判定される**。再生中は中心時刻が動き続けるので、
/// 同じ要求を毎フレーム出し直し、復号を連続キャンセルして波形が追従しなくなる。
/// 要求側と判定側は必ずこの 1 つの答えを使う。
pub(crate) fn effective_visible_pixel_width(visible_pixel_width: usize) -> usize {
    visible_pixel_width.min(MAX_WAVEFORM_TEXTURE_WIDTH)
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
    /// (bin が粗くなるだけ)。描画は `waveform_texture_slice` が時刻から UV を出し、
    /// `painter.image` が宛先矩形へ伸縮するので、**テクスチャの画素幅は位置に関与しない**。
    ///
    /// 先読みぶんを削る順序では可視幅を優先するが、**可視幅そのものが上限を超えるときは
    /// 上限が勝つ**。3 面のマルチモニターにまたがる窓など、可視幅が 8192px を超える構成は
    /// 実在する。そこで「可視部分の解像度は下げない」を通すと、選ぶのは「少しぼやける」対
    /// 「動画機能が再起動まで死ぬ」であって、比べるまでもない。
    pub(crate) fn pixel_width(self, visible_pixel_width: usize, visible_span_secs: f64) -> usize {
        if visible_pixel_width == 0 || !visible_span_secs.is_finite() || visible_span_secs <= 0.0 {
            return 0;
        }
        let effective = effective_visible_pixel_width(visible_pixel_width);
        let ratio = (self.span.duration_secs() / visible_span_secs).max(1.0);
        ((effective as f64 * ratio).round() as usize)
            .max(effective)
            .min(MAX_WAVEFORM_TEXTURE_WIDTH)
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

/// Where the strip stops being the video, as fractions of its visible width.
///
/// `before` covers `0.0..before` and `after` covers `after..1.0`; `None` means the media
/// reaches that edge and nothing should be shaded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WaveOutOfTrack {
    pub(crate) before: Option<f32>,
    pub(crate) after: Option<f32>,
}

/// The parts of the visible strip that are outside the video, from its **duration**.
///
/// Deliberately not from how much waveform has been rastered — and not from the range the
/// raster names either, which is the window that was asked for rather than the part of it
/// that exists: only `content_start_secs` / `content_end_secs` are clamped to the track, so
/// the image is drawn black past the end. Those two regions look the
/// same today — both are simply black — and during a fast drag most of the strip has no
/// raster yet, so the end of the track is invisible exactly when the user is moving fast
/// enough to overshoot it. Shading what is merely unrendered would make "still loading"
/// and "past the end" indistinguishable, which is the thing this shading exists to tell
/// apart (2026-08-28 利用者報告: 終端が見えず次の曲へ行ってしまう)。
pub(crate) fn waveform_out_of_track(
    center_time_secs: f64,
    visible_span_secs: f64,
    duration_secs: f64,
) -> WaveOutOfTrack {
    let empty = WaveOutOfTrack {
        before: None,
        after: None,
    };
    if !center_time_secs.is_finite()
        || !visible_span_secs.is_finite()
        || visible_span_secs <= 0.0
        || !duration_secs.is_finite()
        || duration_secs <= 0.0
    {
        return empty;
    }
    let visible_start = center_time_secs - visible_span_secs * 0.5;
    let fraction = |secs: f64| ((secs - visible_start) / visible_span_secs) as f32;
    WaveOutOfTrack {
        before: (visible_start < 0.0).then(|| fraction(0.0).clamp(0.0, 1.0)),
        after: (visible_start + visible_span_secs > duration_secs)
            .then(|| fraction(duration_secs).clamp(0.0, 1.0)),
    }
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

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct QuantizedWaveformBin([u8; 7]);

fn quantize_unit(value: f32) -> u8 {
    if value.is_finite() {
        (value.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        0
    }
}

fn quantize_waveform_bin(bin: &WaveformBin) -> QuantizedWaveformBin {
    QuantizedWaveformBin([
        quantize_unit(bin.peak_l),
        quantize_unit(bin.rms_l),
        quantize_unit(bin.peak_r),
        quantize_unit(bin.rms_r),
        quantize_unit(bin.band_energy[0]),
        quantize_unit(bin.band_energy[1]),
        quantize_unit(bin.band_energy[2]),
    ])
}

fn dequantize_waveform_bin(
    bin: QuantizedWaveformBin,
    start_secs: f64,
    duration_secs: f64,
) -> WaveformBin {
    let [peak_l, rms_l, peak_r, rms_r, low, mid, high] = bin.0.map(|value| value as f32 / 255.0);
    WaveformBin {
        start_secs,
        duration_secs,
        peak: peak_l.max(peak_r),
        rms: rms_l.max(rms_r),
        peak_l,
        rms_l,
        peak_r,
        rms_r,
        band_energy: [low, mid, high],
        ..WaveformBin::default()
    }
}

fn coarse_chunk_count(duration_secs: f64) -> usize {
    if !duration_secs.is_finite() || duration_secs <= 0.0 {
        0
    } else {
        (duration_secs / COARSE_CHUNK_SECS).ceil() as usize
    }
}

fn coarse_bin_count(duration_secs: f64) -> usize {
    if !duration_secs.is_finite() || duration_secs <= 0.0 {
        0
    } else {
        (duration_secs / COARSE_BIN_SECS).ceil() as usize
    }
}

/// Largest coarse column we are willing to hold, in bytes.
///
/// The column is one [`QuantizedWaveformBin`] per [`COARSE_BIN_SECS`] for the whole
/// file, so its size is set by the duration the container reports — a number mIV
/// does not control and already knows can be wrong: backlog §1.13 is about MPEG-PS
/// files whose duration is missing or nonsense. At 100ms and 7 bytes a bin, a year
/// asks for 2.2 GB, and a duration large enough to saturate the `as usize` cast asks
/// for `usize::MAX` bins, which aborts the process before anything can refuse it.
///
/// 32 MiB is about 5.5 days of audio, past any real recording. Refusing past that
/// costs speed and nothing else: the column is an optimization, so a wide span
/// decodes its own window instead of reading one that was already there.
const MAX_COARSE_WAVEFORM_BYTES: usize = 32 * 1024 * 1024;

/// Whether a coarse column for `duration_secs` fits [`MAX_COARSE_WAVEFORM_BYTES`].
fn coarse_column_fits_budget(duration_secs: f64) -> bool {
    coarse_bin_count(duration_secs)
        .checked_mul(std::mem::size_of::<QuantizedWaveformBin>())
        .is_some_and(|bytes| bytes <= MAX_COARSE_WAVEFORM_BYTES)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CoarseChunkCoverage {
    bits: Vec<u64>,
    chunk_count: usize,
}

impl CoarseChunkCoverage {
    fn new(chunk_count: usize) -> Self {
        Self {
            bits: vec![0; chunk_count.div_ceil(64)],
            chunk_count,
        }
    }

    fn contains(&self, chunk_index: usize) -> bool {
        chunk_index < self.chunk_count
            && self.bits[chunk_index / 64] & (1_u64 << (chunk_index % 64)) != 0
    }

    fn insert(&mut self, chunk_index: usize) -> bool {
        if chunk_index >= self.chunk_count {
            return false;
        }
        let bit = 1_u64 << (chunk_index % 64);
        let word = &mut self.bits[chunk_index / 64];
        let changed = *word & bit == 0;
        *word |= bit;
        changed
    }

    fn marked_count(&self) -> usize {
        self.bits
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum::<usize>()
            .min(self.chunk_count)
    }

    fn covers_time_span(&self, start_secs: f64, end_secs: f64, duration_secs: f64) -> bool {
        let start_secs = start_secs.clamp(0.0, duration_secs);
        let end_secs = end_secs.clamp(0.0, duration_secs);
        if end_secs <= start_secs {
            return true;
        }
        let first = (start_secs / COARSE_CHUNK_SECS).floor() as usize;
        let end = (end_secs / COARSE_CHUNK_SECS).ceil() as usize;
        (first.min(self.chunk_count)..end.min(self.chunk_count))
            .all(|chunk_index| self.contains(chunk_index))
    }

    fn analyzed_spans(
        &self,
        window_start_secs: f64,
        window_end_secs: f64,
        duration_secs: f64,
    ) -> Vec<(f64, f64)> {
        let mut spans = Vec::new();
        let mut chunk_index = 0;
        while chunk_index < self.chunk_count {
            if !self.contains(chunk_index) {
                chunk_index += 1;
                continue;
            }
            let run_start = chunk_index;
            while chunk_index < self.chunk_count && self.contains(chunk_index) {
                chunk_index += 1;
            }
            let start_secs = (run_start as f64 * COARSE_CHUNK_SECS)
                .max(window_start_secs)
                .max(0.0);
            let end_secs = (chunk_index as f64 * COARSE_CHUNK_SECS)
                .min(window_end_secs)
                .min(duration_secs);
            if end_secs > start_secs {
                spans.push((start_secs, end_secs));
            }
        }
        spans
    }
}

/// Pick the unattempted 60-second chunk whose midpoint is nearest the current view center.
fn next_coarse_chunk(
    coverage: &CoarseChunkCoverage,
    failed: &CoarseChunkCoverage,
    center_secs: f64,
) -> Option<usize> {
    debug_assert_eq!(coverage.chunk_count, failed.chunk_count);
    let center_secs = if center_secs.is_finite() {
        center_secs.max(0.0)
    } else {
        0.0
    };
    (0..coverage.chunk_count)
        .filter(|&chunk_index| !coverage.contains(chunk_index) && !failed.contains(chunk_index))
        .min_by(|&left, &right| {
            let left_center = (left as f64 + 0.5) * COARSE_CHUNK_SECS;
            let right_center = (right as f64 + 0.5) * COARSE_CHUNK_SECS;
            (left_center - center_secs)
                .abs()
                .total_cmp(&(right_center - center_secs).abs())
                .then_with(|| left.cmp(&right))
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CoarseChunkUpdate {
    bin_start: usize,
    bins: Vec<QuantizedWaveformBin>,
}

/// Convert one analyzed chunk into the exact slots it owns in the full-length coarse column.
fn compose_coarse_chunk(
    duration_secs: f64,
    chunk_index: usize,
    analyzed_bins: &[WaveformBin],
) -> Option<CoarseChunkUpdate> {
    let total_bins = coarse_bin_count(duration_secs);
    let bin_start = chunk_index.checked_mul(COARSE_BINS_PER_CHUNK)?;
    let bin_end = bin_start
        .saturating_add(COARSE_BINS_PER_CHUNK)
        .min(total_bins);
    if chunk_index >= coarse_chunk_count(duration_secs) || bin_end <= bin_start {
        return None;
    }
    let mut bins = vec![None; bin_end - bin_start];
    for bin in analyzed_bins {
        let global_index = (bin.start_secs / COARSE_BIN_SECS).round() as usize;
        if global_index < bin_start || global_index >= bin_end {
            continue;
        }
        let expected_start = global_index as f64 * COARSE_BIN_SECS;
        if (bin.start_secs - expected_start).abs() > 1.0e-6 {
            return None;
        }
        bins[global_index - bin_start] = Some(quantize_waveform_bin(bin));
    }
    Some(CoarseChunkUpdate {
        bin_start,
        bins: bins.into_iter().collect::<Option<Vec<_>>>()?,
    })
}

struct CoarseWaveform {
    duration_secs: f64,
    bins: Vec<QuantizedWaveformBin>,
    coverage: CoarseChunkCoverage,
    failed: CoarseChunkCoverage,
}

impl CoarseWaveform {
    fn new(duration_secs: f64) -> Self {
        let chunk_count = coarse_chunk_count(duration_secs);
        Self {
            duration_secs,
            bins: vec![QuantizedWaveformBin::default(); coarse_bin_count(duration_secs)],
            coverage: CoarseChunkCoverage::new(chunk_count),
            failed: CoarseChunkCoverage::new(chunk_count),
        }
    }

    fn apply_chunk(&mut self, chunk_index: usize, analyzed_bins: &[WaveformBin]) -> bool {
        let Some(update) = compose_coarse_chunk(self.duration_secs, chunk_index, analyzed_bins)
        else {
            return false;
        };
        let bin_end = update.bin_start + update.bins.len();
        self.bins[update.bin_start..bin_end].copy_from_slice(&update.bins);
        self.coverage.insert(chunk_index)
    }

    fn mark_failed(&mut self, chunk_index: usize) -> bool {
        !self.coverage.contains(chunk_index) && self.failed.insert(chunk_index)
    }

    fn is_complete(&self) -> bool {
        (0..self.coverage.chunk_count).all(|chunk_index| {
            self.coverage.contains(chunk_index) || self.failed.contains(chunk_index)
        })
    }

    fn covers_span(&self, start_secs: f64, end_secs: f64) -> bool {
        self.coverage
            .covers_time_span(start_secs, end_secs, self.duration_secs)
    }

    fn analyzed_spans(&self, start_secs: f64, end_secs: f64) -> Vec<(f64, f64)> {
        self.coverage
            .analyzed_spans(start_secs, end_secs, self.duration_secs)
    }

    fn waveform_bins(&self, start_secs: f64, end_secs: f64) -> Vec<WaveformBin> {
        let start_secs = start_secs.clamp(0.0, self.duration_secs);
        let end_secs = end_secs.clamp(0.0, self.duration_secs);
        if end_secs <= start_secs {
            return Vec::new();
        }
        let first = (start_secs / COARSE_BIN_SECS).floor() as usize;
        let end = (end_secs / COARSE_BIN_SECS).ceil() as usize;
        (first.min(self.bins.len())..end.min(self.bins.len()))
            .filter(|&bin_index| self.coverage.contains(bin_index / COARSE_BINS_PER_CHUNK))
            .map(|bin_index| {
                let bin_start_secs = bin_index as f64 * COARSE_BIN_SECS;
                dequantize_waveform_bin(
                    self.bins[bin_index],
                    bin_start_secs,
                    COARSE_BIN_SECS.min(self.duration_secs - bin_start_secs),
                )
            })
            .collect()
    }

    fn coverage_ratio(&self) -> f64 {
        if self.coverage.chunk_count == 0 {
            1.0
        } else {
            self.coverage.marked_count() as f64 / self.coverage.chunk_count as f64
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaveRenderRoute {
    /// No column will be built and the span is past what one decode may hold in memory.
    /// See [`decide_wave_render_route`].
    TooWideWithoutColumn,
    Coarse,
    WindowDecode,
    CoarseProgressive,
}

/// D27's ordered three-way routing decision.
/// What the coarse column can offer this request.
///
/// `CoarseProgressive` waits for a column to fill in. If none will ever exist the wait
/// never ends, so "not covered yet" and "never" have to be different answers -- telling
/// them apart is what keeps a span past thirty minutes from failing outright.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CoarseAvailability {
    /// A column exists and already spans the visible range.
    CoversVisible,
    /// A column exists or is still being built, but does not span it yet.
    NotYet,
    /// No column will be built for this file, so nothing arrives by waiting.
    Never,
}

fn decide_wave_render_route(
    coarse: CoarseAvailability,
    requested_bin_secs: f64,
    visible_span_secs: f64,
) -> WaveRenderRoute {
    match coarse {
        CoarseAvailability::CoversVisible if requested_bin_secs >= COARSE_BIN_SECS => {
            WaveRenderRoute::Coarse
        }
        // With no column to progress against, the progressive route only refuses the
        // request, so a span past thirty minutes would show nothing at all. Decode the
        // window instead -- but only while the window is one a decode may hold.
        //
        // `WINDOW_DECODE_MAX_SPAN_SECS` is not a preference. A range decode allocates the
        // whole requested span as 48kHz stereo f32, sized from the span rather than from
        // the audio actually found, and holds the aligned copy beside it: three hours is
        // 3.86 GiB twice over. Sending the widest spans here to avoid one OOM would only
        // have bought a larger one.
        CoarseAvailability::Never if visible_span_secs <= WINDOW_DECODE_MAX_SPAN_SECS => {
            WaveRenderRoute::WindowDecode
        }
        CoarseAvailability::Never => WaveRenderRoute::TooWideWithoutColumn,
        _ if visible_span_secs <= WINDOW_DECODE_MAX_SPAN_SECS => WaveRenderRoute::WindowDecode,
        _ => WaveRenderRoute::CoarseProgressive,
    }
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
    pub(crate) revision: u64,
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
    coarse_center_bits: Arc<AtomicU64>,
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
        let coarse_center_bits = Arc::new(AtomicU64::new(0.0_f64.to_bits()));
        let worker_pending = Arc::clone(&pending);
        let worker_state = Arc::clone(&state);
        let worker_cancel = Arc::clone(&cancel);
        let worker_latest = Arc::clone(&latest_request_id);
        let worker_coarse_center_bits = Arc::clone(&coarse_center_bits);
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
                            &worker_coarse_center_bits,
                            &mut runtime,
                        );
                        continue;
                    }
                    if process_next_coarse_chunk(
                        &path,
                        &worker_state,
                        &worker_cancel,
                        &worker_latest,
                        &worker_coarse_center_bits,
                        &mut runtime,
                    ) {
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
            coarse_center_bits,
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

    pub(crate) fn prioritize_coarse_center(&self, center_time_secs: f64) {
        if center_time_secs.is_finite() {
            self.coarse_center_bits
                .store(center_time_secs.to_bits(), Ordering::Release);
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

enum CoarseBuildState {
    Dormant,
    Building(CoarseWaveform),
    Unavailable(crate::audio_decode::AudioDecodeOpenError),
    /// The reported duration asks for a column past [`MAX_COARSE_WAVEFORM_BYTES`], so
    /// none was built. Distinct from `Unavailable`, which means the audio itself cannot
    /// be read: here the waveform still works, it just decodes each window it shows.
    OverBudget,
}

#[derive(Clone, Copy)]
struct ActiveWaveRequest {
    id: u64,
    signature: WaveRequestSignature,
}

struct WaveWorkerRuntime {
    decoder: Option<crate::audio_decode::AudioRangeDecoder>,
    decoder_open_error: Option<crate::audio_decode::AudioDecodeOpenError>,
    coarse: CoarseBuildState,
    active_request: Option<ActiveWaveRequest>,
    next_raster_revision: u64,
    lru: WaveRasterLru,
}

impl WaveWorkerRuntime {
    fn new() -> Self {
        Self {
            decoder: None,
            decoder_open_error: None,
            coarse: CoarseBuildState::Dormant,
            active_request: None,
            next_raster_revision: 1,
            lru: WaveRasterLru::new(WAVE_RASTER_LRU_MAX_ENTRIES),
        }
    }

    fn start_coarse_if_needed(&mut self, duration_secs: f64, visible_span_secs: f64) {
        if visible_span_secs >= COARSE_BUILD_MIN_SPAN_SECS
            && matches!(self.coarse, CoarseBuildState::Dormant)
        {
            self.coarse = if coarse_column_fits_budget(duration_secs) {
                CoarseBuildState::Building(CoarseWaveform::new(duration_secs))
            } else {
                crate::logger::log(format!(
                    "[wave] no coarse column: duration {duration_secs:.0}s wants {} bins,                      over the {} MiB budget; wide spans will decode their own window",
                    coarse_bin_count(duration_secs),
                    MAX_COARSE_WAVEFORM_BYTES / (1024 * 1024)
                ));
                CoarseBuildState::OverBudget
            };
        }
    }

    /// What the column can offer, asked of the state rather than rebuilt by the caller.
    /// `OverBudget` is not "not yet": waiting on it never resolves.
    fn coarse_availability(
        &self,
        signature: WaveRequestSignature,
        visible_center_secs: f64,
    ) -> CoarseAvailability {
        match &self.coarse {
            CoarseBuildState::OverBudget => CoarseAvailability::Never,
            CoarseBuildState::Building(coarse)
                if coarse_covers_visible(coarse, signature, visible_center_secs) =>
            {
                CoarseAvailability::CoversVisible
            }
            CoarseBuildState::Building(_)
            | CoarseBuildState::Dormant
            | CoarseBuildState::Unavailable(_) => CoarseAvailability::NotYet,
        }
    }

    fn coarse(&self) -> Option<&CoarseWaveform> {
        match &self.coarse {
            CoarseBuildState::Building(coarse) => Some(coarse),
            CoarseBuildState::Dormant
            | CoarseBuildState::Unavailable(_)
            | CoarseBuildState::OverBudget => None,
        }
    }

    fn coarse_mut(&mut self) -> Option<&mut CoarseWaveform> {
        match &mut self.coarse {
            CoarseBuildState::Building(coarse) => Some(coarse),
            CoarseBuildState::Dormant
            | CoarseBuildState::Unavailable(_)
            | CoarseBuildState::OverBudget => None,
        }
    }

    fn next_raster_revision(&mut self) -> u64 {
        let revision = self.next_raster_revision;
        self.next_raster_revision = self.next_raster_revision.wrapping_add(1).max(1);
        revision
    }
}

enum CoarseChunkError {
    Unavailable(crate::audio_decode::AudioDecodeOpenError),
    Failed(String),
    Cancelled,
}

struct CoarseChunkAnalysis {
    bins: Vec<WaveformBin>,
    decode_elapsed: Duration,
    analyze_elapsed: Duration,
    discarded_non_audio_streams: usize,
}

fn analyze_coarse_chunk(
    path: &Path,
    chunk_index: usize,
    duration_secs: f64,
    cancel: &AtomicBool,
    runtime: &mut WaveWorkerRuntime,
) -> Result<CoarseChunkAnalysis, CoarseChunkError> {
    let chunk_start_secs = chunk_index as f64 * COARSE_CHUNK_SECS;
    let chunk_end_secs = (chunk_start_secs + COARSE_CHUNK_SECS).min(duration_secs);
    let window = WaveWindow {
        start_secs: chunk_start_secs,
        end_secs: chunk_end_secs,
        content_start_secs: chunk_start_secs,
        content_end_secs: chunk_end_secs,
    };
    let analysis_range = waveform_analysis_range(window, duration_secs, COARSE_BIN_SECS)
        .ok_or_else(|| CoarseChunkError::Failed("invalid coarse chunk range".into()))?;
    if runtime.decoder.is_none() && runtime.decoder_open_error.is_none() {
        match crate::audio_decode::AudioRangeDecoder::open(path) {
            Ok(decoder) => runtime.decoder = Some(decoder),
            Err(error) => runtime.decoder_open_error = Some(error),
        }
    }
    if let Some(error) = runtime.decoder_open_error.as_ref() {
        return Err(CoarseChunkError::Unavailable(error.clone()));
    }
    let decoder = runtime
        .decoder
        .as_mut()
        .ok_or_else(|| CoarseChunkError::Failed("audio decoder unavailable".into()))?;
    let discarded_non_audio_streams = decoder.discarded_non_audio_streams();
    let decode_t0 = Instant::now();
    let decoded = decoder
        .decode_range_to_stereo_f32(analysis_range.start_secs, analysis_range.end_secs, &|| {
            cancel.load(Ordering::Acquire)
        })
        .map_err(|error| {
            if error == "cancelled" {
                CoarseChunkError::Cancelled
            } else {
                CoarseChunkError::Failed(error)
            }
        })?;
    let decode_elapsed = decode_t0.elapsed();
    let analyze_t0 = Instant::now();
    let analysis = analyze_window_samples(
        &decoded.stereo_samples,
        decoded.info.sample_rate,
        analysis_range.start_secs,
        chunk_start_secs,
        chunk_end_secs,
        COARSE_BIN_SECS,
        COARSE_CHUNK_SECS,
        &|| cancel.load(Ordering::Acquire),
    )
    .ok_or(CoarseChunkError::Cancelled)?;
    Ok(CoarseChunkAnalysis {
        bins: analysis.bins,
        decode_elapsed,
        analyze_elapsed: analyze_t0.elapsed(),
        discarded_non_audio_streams,
    })
}

fn record_coarse_chunk_failure(
    chunk_index: usize,
    reason: &str,
    state: &Mutex<WaveSharedState>,
    latest_request_id: &AtomicU64,
    coarse_center_bits: &AtomicU64,
    runtime: &mut WaveWorkerRuntime,
) {
    let coarse = runtime
        .coarse_mut()
        .expect("selected coarse chunk requires an active build");
    assert!(
        coarse.mark_failed(chunk_index),
        "selected coarse chunk must be unattempted"
    );
    let covered_chunks = coarse.coverage.marked_count();
    let failed_chunks = coarse.failed.marked_count();
    let total_chunks = coarse.coverage.chunk_count;
    emit_wave_coarse_chunk_failure(
        chunk_index,
        reason,
        covered_chunks,
        failed_chunks,
        total_chunks,
    );
    publish_active_coarse_raster(state, latest_request_id, coarse_center_bits, runtime);
}

fn process_next_coarse_chunk(
    path: &Path,
    state: &Mutex<WaveSharedState>,
    cancel: &AtomicBool,
    latest_request_id: &AtomicU64,
    coarse_center_bits: &AtomicU64,
    runtime: &mut WaveWorkerRuntime,
) -> bool {
    if cancel.load(Ordering::Acquire) {
        return false;
    }
    let center_secs = f64::from_bits(coarse_center_bits.load(Ordering::Acquire));
    let Some(chunk_index) = runtime
        .coarse()
        .and_then(|coarse| next_coarse_chunk(&coarse.coverage, &coarse.failed, center_secs))
    else {
        return false;
    };
    let analysis = match analyze_coarse_chunk(
        path,
        chunk_index,
        runtime.coarse().map_or(0.0, |coarse| coarse.duration_secs),
        cancel,
        runtime,
    ) {
        Ok(analysis) => analysis,
        Err(CoarseChunkError::Cancelled) => return true,
        Err(CoarseChunkError::Unavailable(error)) => {
            runtime.coarse = CoarseBuildState::Unavailable(error.clone());
            publish_active_decoder_error(runtime.active_request, state, latest_request_id, &error);
            return false;
        }
        Err(CoarseChunkError::Failed(error)) => {
            record_coarse_chunk_failure(
                chunk_index,
                &error,
                state,
                latest_request_id,
                coarse_center_bits,
                runtime,
            );
            return true;
        }
    };
    let decode_elapsed = analysis.decode_elapsed;
    let analyze_elapsed = analysis.analyze_elapsed;
    let discarded_non_audio_streams = analysis.discarded_non_audio_streams;
    let Some(coarse) = runtime.coarse_mut() else {
        return false;
    };
    if !coarse.apply_chunk(chunk_index, &analysis.bins) {
        record_coarse_chunk_failure(
            chunk_index,
            "coarse chunk composition failed",
            state,
            latest_request_id,
            coarse_center_bits,
            runtime,
        );
        return true;
    }
    let covered_chunks = coarse.coverage.marked_count();
    let failed_chunks = coarse.failed.marked_count();
    let total_chunks = coarse.coverage.chunk_count;
    emit_wave_coarse_chunk(
        chunk_index,
        decode_elapsed,
        analyze_elapsed,
        discarded_non_audio_streams,
        covered_chunks,
        failed_chunks,
        total_chunks,
    );
    publish_active_coarse_raster(state, latest_request_id, coarse_center_bits, runtime);
    true
}

fn publish_active_decoder_error(
    active: Option<ActiveWaveRequest>,
    state: &Mutex<WaveSharedState>,
    latest_request_id: &AtomicU64,
    error: &crate::audio_decode::AudioDecodeOpenError,
) {
    if let Some(active) = active {
        publish_decoder_open_error(state, active.id, latest_request_id, error);
    }
}

fn publish_active_coarse_raster(
    state: &Mutex<WaveSharedState>,
    latest_request_id: &AtomicU64,
    coarse_center_bits: &AtomicU64,
    runtime: &mut WaveWorkerRuntime,
) {
    let Some(active) = runtime.active_request else {
        return;
    };
    if latest_request_id.load(Ordering::Acquire) != active.id {
        return;
    }
    let visible_center_secs = f64::from_bits(coarse_center_bits.load(Ordering::Acquire));
    let Some(coarse) = runtime.coarse() else {
        return;
    };
    let visible_covered = coarse_covers_visible(coarse, active.signature, visible_center_secs);
    let route = decide_wave_render_route(
        if visible_covered {
            CoarseAvailability::CoversVisible
        } else {
            CoarseAvailability::NotYet
        },
        waveform_bin_secs(active.signature.span_secs(), active.signature.pixel_width),
        active.signature.visible_span_secs(),
    );
    if route == WaveRenderRoute::WindowDecode {
        return;
    }
    let Some(window) = waveform_window(
        active.signature.center_time_secs(),
        active.signature.duration_secs(),
        active.signature.span_secs(),
    ) else {
        return;
    };
    process_coarse_request(
        active.id,
        active.signature,
        window,
        visible_covered,
        state,
        latest_request_id,
        runtime,
    );
}

fn request_is_stale(id: u64, cancel: &AtomicBool, latest_request_id: &AtomicU64) -> bool {
    cancel.load(Ordering::Acquire) || latest_request_id.load(Ordering::Acquire) != id
}

fn coarse_covers_visible(
    coarse: &CoarseWaveform,
    signature: WaveRequestSignature,
    visible_center_secs: f64,
) -> bool {
    waveform_window(
        visible_center_secs,
        signature.duration_secs(),
        signature.visible_span_secs(),
    )
    .is_some_and(|window| coarse.covers_span(window.content_start_secs, window.content_end_secs))
}

fn process_coarse_request(
    request_id: u64,
    signature: WaveRequestSignature,
    window: WaveWindow,
    visible_covered: bool,
    state: &Mutex<WaveSharedState>,
    latest_request_id: &AtomicU64,
    runtime: &mut WaveWorkerRuntime,
) {
    let revision = runtime.next_raster_revision();
    if let CoarseBuildState::Unavailable(error) = &runtime.coarse {
        publish_decoder_open_error(state, request_id, latest_request_id, error);
        return;
    }
    let CoarseBuildState::Building(coarse) = &runtime.coarse else {
        publish_wave_failure(
            state,
            request_id,
            latest_request_id,
            "coarse waveform unavailable",
        );
        return;
    };
    let raster = render_coarse_raster(coarse, signature, window, revision);
    let status = if visible_covered || coarse.is_complete() {
        WaveWorkerStatus::Idle
    } else {
        WaveWorkerStatus::Working
    };
    publish_wave_raster(state, request_id, latest_request_id, raster, status);
}

fn render_coarse_raster(
    coarse: &CoarseWaveform,
    signature: WaveRequestSignature,
    window: WaveWindow,
    revision: u64,
) -> Arc<WaveRaster> {
    let bins = coarse.waveform_bins(window.content_start_secs, window.content_end_secs);
    let analyzed_spans = coarse.analyzed_spans(window.start_secs, window.end_secs);
    let strip = crate::ui_music_timeline::SeekStripRowStyle {
        analyzed_spans: &analyzed_spans,
        duration_secs: signature.duration_secs(),
    };
    let (image, _) = crate::ui_music_timeline::render_timeline_row_image(
        window.start_secs,
        signature.span_secs(),
        &bins,
        Some(&strip),
        signature.pixel_width,
        signature.pixel_height,
        0,
        0,
        true,
    );
    let mut rgba = Vec::with_capacity(image.pixels.len().saturating_mul(4));
    for pixel in image.pixels {
        rgba.extend_from_slice(&pixel.to_srgba_unmultiplied());
    }
    emit_wave_coarse_serve(coarse, signature);
    Arc::new(WaveRaster {
        revision,
        window_start_secs: window.start_secs,
        window_end_secs: window.end_secs,
        visible_span_secs: signature.visible_span_secs(),
        bin_secs: COARSE_BIN_SECS,
        width: signature.pixel_width as u32,
        height: signature.pixel_height as u32,
        rgba: Arc::new(rgba),
    })
}

fn process_wave_request(
    path: &Path,
    identity: &WaveFileIdentity,
    request: WaveRequest,
    state: &Mutex<WaveSharedState>,
    cancel: &AtomicBool,
    latest_request_id: &AtomicU64,
    coarse_center_bits: &AtomicU64,
    runtime: &mut WaveWorkerRuntime,
) {
    let total_t0 = Instant::now();
    let signature = request.signature;
    runtime.active_request = Some(ActiveWaveRequest {
        id: request.id,
        signature,
    });
    runtime.start_coarse_if_needed(signature.duration_secs(), signature.visible_span_secs());
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
    let availability = runtime.coarse_availability(
        signature,
        f64::from_bits(coarse_center_bits.load(Ordering::Acquire)),
    );
    let coarse_covers_visible = availability == CoarseAvailability::CoversVisible;
    let route = decide_wave_render_route(availability, bin_secs, signature.visible_span_secs());
    if route == WaveRenderRoute::CoarseProgressive && runtime.decoder.is_none() {
        if runtime.decoder_open_error.is_none() {
            match crate::audio_decode::AudioRangeDecoder::open(path) {
                Ok(decoder) => runtime.decoder = Some(decoder),
                Err(error) => runtime.decoder_open_error = Some(error),
            }
        }
        if let Some(error) = runtime.decoder_open_error.clone() {
            runtime.coarse = CoarseBuildState::Unavailable(error.clone());
            publish_decoder_open_error(state, request.id, latest_request_id, &error);
            return;
        }
    }
    if route == WaveRenderRoute::TooWideWithoutColumn {
        // Say so rather than falling into the coarse path, which would report the same
        // "coarse waveform unavailable" it reports for a column that is merely still
        // filling in. Narrower spans on this file still work.
        publish_wave_failure(
            state,
            request.id,
            latest_request_id,
            "this file has no coarse column, and the span is wider than one decode may hold",
        );
        return;
    }
    if route != WaveRenderRoute::WindowDecode {
        process_coarse_request(
            request.id,
            signature,
            window,
            coarse_covers_visible,
            state,
            latest_request_id,
            runtime,
        );
        return;
    }
    let key = WaveRasterKey::new(
        identity.clone(),
        window,
        bin_secs,
        signature.visible_span_secs(),
        signature.pixel_width,
        signature.pixel_height,
    );
    if let Some(raster) = runtime.lru.get(&key) {
        publish_wave_raster(
            state,
            request.id,
            latest_request_id,
            raster,
            WaveWorkerStatus::Idle,
        );
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
    // 窓復号は窓全体が解析済み。ここで `None` を渡すと音楽ビュー扱いになり、同じ帯なのに
    // 粗い列の経路と中心線の明るさが変わってしまう。
    let window_analyzed = [(window.content_start_secs, window.content_end_secs)];
    let strip = crate::ui_music_timeline::SeekStripRowStyle {
        analyzed_spans: &window_analyzed,
        duration_secs: signature.duration_secs(),
    };
    let (image, _) = crate::ui_music_timeline::render_timeline_row_image(
        window.start_secs,
        signature.span_secs(),
        bins,
        Some(&strip),
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
        revision: runtime.next_raster_revision(),
        window_start_secs: window.start_secs,
        window_end_secs: window.end_secs,
        visible_span_secs: signature.visible_span_secs(),
        bin_secs,
        width: signature.pixel_width as u32,
        height: signature.pixel_height as u32,
        rgba: Arc::new(rgba),
    });
    runtime.lru.insert(key, Arc::clone(&raster));
    publish_wave_raster(
        state,
        request.id,
        latest_request_id,
        raster,
        WaveWorkerStatus::Idle,
    );
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
    status: WaveWorkerStatus,
) {
    if latest_request_id.load(Ordering::Acquire) != request_id {
        return;
    }
    let mut shared = lock_recover(state);
    if shared.latest_request_id == request_id {
        shared.status = status;
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

fn publish_decoder_open_error(
    state: &Mutex<WaveSharedState>,
    request_id: u64,
    latest_request_id: &AtomicU64,
    error: &crate::audio_decode::AudioDecodeOpenError,
) {
    match error {
        crate::audio_decode::AudioDecodeOpenError::NoAudioTrack => {
            publish_wave_no_audio_track(state, request_id, latest_request_id);
        }
        crate::audio_decode::AudioDecodeOpenError::Failed(error) => {
            publish_wave_failure(state, request_id, latest_request_id, error);
        }
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

fn emit_wave_coarse_chunk(
    chunk_index: usize,
    decode: Duration,
    analyze: Duration,
    discarded_non_audio_streams: usize,
    covered_chunks: usize,
    failed_chunks: usize,
    total_chunks: usize,
) {
    if crate::perf::is_enabled() {
        crate::perf::event(
            stringify!(video_strip),
            stringify!(wave_coarse_chunk),
            None,
            0,
            &[
                (stringify!(outcome), serde_json::Value::from("success")),
                (
                    stringify!(chunk_index),
                    serde_json::Value::from(chunk_index as u64),
                ),
                (
                    stringify!(decode_ms),
                    serde_json::Value::from(decode.as_secs_f64() * 1000.0),
                ),
                (
                    stringify!(analyze_ms),
                    serde_json::Value::from(analyze.as_secs_f64() * 1000.0),
                ),
                (
                    stringify!(discarded_non_audio_streams),
                    serde_json::Value::from(discarded_non_audio_streams as u64),
                ),
                (
                    stringify!(covered_chunks),
                    serde_json::Value::from(covered_chunks as u64),
                ),
                (
                    stringify!(failed_chunks),
                    serde_json::Value::from(failed_chunks as u64),
                ),
                (
                    stringify!(total_chunks),
                    serde_json::Value::from(total_chunks as u64),
                ),
            ],
        );
    }
}

fn emit_wave_coarse_chunk_failure(
    chunk_index: usize,
    reason: &str,
    covered_chunks: usize,
    failed_chunks: usize,
    total_chunks: usize,
) {
    if crate::perf::is_enabled() {
        crate::perf::event(
            stringify!(video_strip),
            stringify!(wave_coarse_chunk),
            None,
            0,
            &[
                (stringify!(outcome), serde_json::Value::from("failed")),
                (
                    stringify!(chunk_index),
                    serde_json::Value::from(chunk_index as u64),
                ),
                (stringify!(reason), serde_json::Value::from(reason)),
                (
                    stringify!(covered_chunks),
                    serde_json::Value::from(covered_chunks as u64),
                ),
                (
                    stringify!(failed_chunks),
                    serde_json::Value::from(failed_chunks as u64),
                ),
                (
                    stringify!(total_chunks),
                    serde_json::Value::from(total_chunks as u64),
                ),
            ],
        );
    }
}

fn emit_wave_coarse_serve(coarse: &CoarseWaveform, signature: WaveRequestSignature) {
    if crate::perf::is_enabled() {
        crate::perf::event(
            stringify!(video_strip),
            stringify!(wave_coarse_serve),
            None,
            0,
            &[
                (
                    stringify!(coverage_ratio),
                    serde_json::Value::from(coarse.coverage_ratio()),
                ),
                (
                    stringify!(covered_chunks),
                    serde_json::Value::from(coarse.coverage.marked_count() as u64),
                ),
                (
                    stringify!(failed_chunks),
                    serde_json::Value::from(coarse.failed.marked_count() as u64),
                ),
                (
                    stringify!(total_chunks),
                    serde_json::Value::from(coarse.coverage.chunk_count as u64),
                ),
                (
                    stringify!(complete),
                    serde_json::Value::from(coarse.is_complete()),
                ),
                (
                    stringify!(raster_span_secs),
                    serde_json::Value::from(signature.span_secs()),
                ),
                (
                    stringify!(visible_span_secs),
                    serde_json::Value::from(signature.visible_span_secs()),
                ),
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

    #[test]
    fn coarse_quantization_round_trip_stays_within_one_u8_step() {
        assert_eq!(std::mem::size_of::<QuantizedWaveformBin>(), 7);
        let source = WaveformBin {
            peak_l: 0.013,
            rms_l: 0.177,
            peak_r: 0.501,
            rms_r: 0.749,
            band_energy: [0.111, 0.333, 0.901],
            ..WaveformBin::default()
        };
        let restored = dequantize_waveform_bin(quantize_waveform_bin(&source), 12.3, 0.1);
        let original = [
            source.peak_l,
            source.rms_l,
            source.peak_r,
            source.rms_r,
            source.band_energy[0],
            source.band_energy[1],
            source.band_energy[2],
        ];
        let round_trip = [
            restored.peak_l,
            restored.rms_l,
            restored.peak_r,
            restored.rms_r,
            restored.band_energy[0],
            restored.band_energy[1],
            restored.band_energy[2],
        ];
        for (original, restored) in original.into_iter().zip(round_trip) {
            assert!((original - restored).abs() <= 1.0 / 255.0);
        }
    }

    #[test]
    fn coarse_chunk_selection_tracks_center_and_coverage_islands() {
        let mut coverage = CoarseChunkCoverage::new(7);
        let mut failed = CoarseChunkCoverage::new(7);
        assert_eq!(next_coarse_chunk(&coverage, &failed, 0.0), Some(0));
        assert_eq!(next_coarse_chunk(&coverage, &failed, 10_000.0), Some(6));
        assert_eq!(next_coarse_chunk(&coverage, &failed, 210.0), Some(3));
        coverage.insert(3);
        assert_eq!(next_coarse_chunk(&coverage, &failed, 210.0), Some(2));
        failed.insert(2);
        assert_eq!(next_coarse_chunk(&coverage, &failed, 210.0), Some(4));
        for chunk_index in 0..7 {
            if chunk_index % 2 == 0 {
                failed.insert(chunk_index);
            } else {
                coverage.insert(chunk_index);
            }
        }
        assert_eq!(next_coarse_chunk(&coverage, &failed, 210.0), None);
    }

    #[test]
    fn d27_route_order_preserves_window_decode_through_thirty_minutes() {
        use CoarseAvailability::{CoversVisible, Never, NotYet};

        assert_eq!(
            decide_wave_render_route(CoversVisible, COARSE_BIN_SECS, 600.0),
            WaveRenderRoute::Coarse
        );
        assert_eq!(
            decide_wave_render_route(CoversVisible, COARSE_BIN_SECS - 0.001, 600.0),
            WaveRenderRoute::WindowDecode
        );
        assert_eq!(
            decide_wave_render_route(NotYet, 0.5, WINDOW_DECODE_MAX_SPAN_SECS),
            WaveRenderRoute::WindowDecode
        );
        assert_eq!(
            decide_wave_render_route(NotYet, 0.5, WINDOW_DECODE_MAX_SPAN_SECS + 0.001),
            WaveRenderRoute::CoarseProgressive
        );

        // 列が来ないと分かっているなら、**待つ経路へは入れない**。入れると
        // `process_coarse_request` が毎回 Failed を返し、30 分より広い段が全滅する。
        // ただし一括復号にも上限があるので、そこを超えたら型で断る (P1-1)。
        for visible_span_secs in [600.0, WINDOW_DECODE_MAX_SPAN_SECS] {
            assert_eq!(
                decide_wave_render_route(Never, 0.5, visible_span_secs),
                WaveRenderRoute::WindowDecode,
                "{visible_span_secs}s の段が待ちに入ってしまう"
            );
        }
        for visible_span_secs in [
            WINDOW_DECODE_MAX_SPAN_SECS + 0.001,
            3600.0,
            7200.0,
            10_800.0,
        ] {
            assert_eq!(
                decide_wave_render_route(Never, 0.5, visible_span_secs),
                WaveRenderRoute::TooWideWithoutColumn,
                "{visible_span_secs}s を一括復号へ送ってはいけない"
            );
        }
    }

    /// 要求が作る raster は、必ず「表示済み」判定を満たす幅になる。
    ///
    /// 判定側 ([native_video.rs] の `displayed`) が生の可視幅と比べていたため、8192 で
    /// 頭打ちになった raster は可視幅 8193px 以上で**永久に「まだ足りない」**と読まれた。
    /// 再生中は中心時刻が 100ms ごとに動くので毎フレーム要求し直し、復号を連続キャンセル
    /// して波形が追従しなくなる (2026-08-27 Codex 再レビュー P2-1)。
    #[test]
    fn a_finished_raster_always_satisfies_the_displayed_width_check() {
        for visible in [
            400_usize,
            1_920,
            3_840,
            MAX_WAVEFORM_TEXTURE_WIDTH - 1,
            MAX_WAVEFORM_TEXTURE_WIDTH,
            MAX_WAVEFORM_TEXTURE_WIDTH + 1,
            11_520,
            30_000,
        ] {
            for visible_span_secs in [30.0, 600.0, 3_600.0] {
                let request = WaveSpanRequest {
                    span: WaveSpan::centered(1_000.0, visible_span_secs).expect("span"),
                    stage: WaveRequestStage::FirstPaint,
                };
                let produced = request.pixel_width(visible, visible_span_secs);
                assert!(
                    produced >= effective_visible_pixel_width(visible),
                    "visible={visible} span={visible_span_secs}: 作った {produced}px が                      表示済み判定の下限 {}px に届かない",
                    effective_visible_pixel_width(visible)
                );
                assert!(produced <= MAX_WAVEFORM_TEXTURE_WIDTH);
            }
        }
    }

    /// 予算超過で列を作らないと決めた動画でも、1 時間以上の段が失敗しない。
    ///
    /// 状態だけのテストでは足りない: `decide_wave_render_route` が `CoarseProgressive` を
    /// 返すと `process_coarse_request` が `Building` 以外を `"coarse waveform unavailable"`
    /// として `Failed` にするため、**列を作らない判断がそのまま波形の全滅になる**
    /// (2026-08-27 Codex 再レビュー P1-2)。
    #[test]
    fn an_over_budget_file_still_routes_its_widest_spans_to_a_window_decode() {
        let mut runtime = WaveWorkerRuntime::new();
        runtime.start_coarse_if_needed(1e300, COARSE_BUILD_MIN_SPAN_SECS);
        assert!(matches!(&runtime.coarse, CoarseBuildState::OverBudget));

        for visible_span_secs in [1800.0, 3600.0, 7200.0, 10_800.0] {
            let signature = WaveRequestSignature::new(
                5_000.0,
                1e300,
                visible_span_secs,
                visible_span_secs,
                1_920,
                131,
            )
            .expect("a valid signature");
            let availability = runtime.coarse_availability(signature, 5_000.0);
            assert_eq!(
                availability,
                CoarseAvailability::Never,
                "予算超過は「まだ」ではなく「来ない」"
            );
            let route = decide_wave_render_route(
                availability,
                waveform_bin_secs(signature.span_secs(), signature.pixel_width),
                visible_span_secs,
            );
            if visible_span_secs <= WINDOW_DECODE_MAX_SPAN_SECS {
                assert_eq!(
                    route,
                    WaveRenderRoute::WindowDecode,
                    "{visible_span_secs}s の段が復号へ回らない"
                );
            } else {
                assert_eq!(
                    route,
                    WaveRenderRoute::TooWideWithoutColumn,
                    "{visible_span_secs}s を一括復号へ送ると PCM が数 GiB になる"
                );
            }
        }
    }

    /// 曲の外側は duration から決まり、**波形が焼けているかには左右されない**。
    ///
    /// 素早くドラッグしている間は大半がまだ未描画なので、未描画を根拠に陰をつけると
    /// 「読込中」と「曲の外」が同じ見え方になる。区別が要るのはまさにその瞬間で、
    /// 終端が見えないまま行き過ぎて次の曲へ移ってしまう (2026-08-28 利用者報告)。
    #[test]
    fn the_outside_of_a_track_is_decided_by_its_duration_not_by_what_is_rendered() {
        // 中央付近: 端はどちらも見えない。
        let middle = waveform_out_of_track(300.0, 180.0, 600.0);
        assert_eq!(middle.before, None);
        assert_eq!(middle.after, None);

        // 終端が画面の中央に来る位置。右半分が曲の外。
        let at_end = waveform_out_of_track(600.0, 180.0, 600.0);
        assert_eq!(at_end.before, None);
        assert_eq!(at_end.after, Some(0.5));

        // 先頭が画面の中央。左半分が曲の外。
        let at_start = waveform_out_of_track(0.0, 180.0, 600.0);
        assert_eq!(at_start.before, Some(0.5));
        assert_eq!(at_start.after, None);

        // 表示範囲が曲より広ければ、両側が出る。
        let wider_than_track = waveform_out_of_track(30.0, 600.0, 60.0);
        assert_eq!(wider_than_track.before, Some(0.45));
        assert_eq!(wider_than_track.after, Some(0.55));

        // 終端の直前: わずかに見えるだけでも位置は正しい。
        let nearly_at_end = waveform_out_of_track(599.0, 180.0, 600.0);
        let after = nearly_at_end.after.expect("終端は画面内にある");
        assert!((after - (91.0 / 180.0)).abs() < 1e-5, "{after}");

        // 値が壊れていれば何も塗らない。塗る根拠が無いときに塗る方が悪い。
        for (center, span, duration) in [
            (f64::NAN, 180.0, 600.0),
            (300.0, 0.0, 600.0),
            (300.0, 180.0, 0.0),
            (300.0, 180.0, f64::INFINITY),
        ] {
            let broken = waveform_out_of_track(center, span, duration);
            assert_eq!(
                broken,
                WaveOutOfTrack {
                    before: None,
                    after: None
                }
            );
        }
    }

    /// 一括 PCM 復号へ回る範囲は、必ず固定上限の内側に収まる。
    ///
    /// `decode_range_to_stereo_f32` は要求範囲ぶんの 48kHz stereo f32 を確保する。長さは
    /// **実際に見つかった音声ではなく要求範囲**で決まり、時刻合わせ後のコピーが並存するので、
    /// 3 時間なら 3.86 GiB が 2 つになる。列を作れない動画を無条件に一括復号へ送ると、
    /// 32 MiB の OOM を避けた代わりにその 200 倍を確保することになる
    /// (2026-08-28 Codex 第 4 回レビュー P1-1)。
    ///
    /// route の列挙ではなく、**そこから決まる確保量**を縛る。
    #[test]
    fn nothing_reaches_a_range_decode_with_more_span_than_it_may_hold() {
        /// 48kHz stereo f32、時刻合わせ後のコピーと並存する分を含む。
        fn peak_pcm_bytes(span_secs: f64) -> f64 {
            span_secs * 48_000.0 * 2.0 * 4.0 * 2.0
        }
        let ceiling = peak_pcm_bytes(WINDOW_DECODE_MAX_SPAN_SECS);

        for availability in [
            CoarseAvailability::CoversVisible,
            CoarseAvailability::NotYet,
            CoarseAvailability::Never,
        ] {
            for visible_span_secs in crate::video::seek_strip::WAVEFORM_RANGE_STEPS_SECS
                .iter()
                .copied()
            {
                for requested_bin_secs in [MIN_WAVEFORM_BIN_SECS, 0.5, COARSE_BIN_SECS, 5.0] {
                    let route = decide_wave_render_route(
                        availability,
                        requested_bin_secs,
                        visible_span_secs,
                    );
                    if route == WaveRenderRoute::WindowDecode {
                        assert!(
                            peak_pcm_bytes(visible_span_secs) <= ceiling,
                            "{availability:?} / {visible_span_secs}s / bin {requested_bin_secs} が                              一括復号へ回り、PCM は {:.1} GiB になる",
                            peak_pcm_bytes(visible_span_secs) / (1024.0 * 1024.0 * 1024.0)
                        );
                    }
                }
            }
        }

        // 上限そのものは 3 時間の 1/6。実測でこの値が動いたら気づけるように書いておく。
        assert!((ceiling / (1024.0 * 1024.0 * 1024.0) - 1.288).abs() < 0.01);
    }

    #[test]
    fn coarse_build_starts_at_the_visible_span_threshold_only() {
        let mut runtime = WaveWorkerRuntime::new();
        runtime.start_coarse_if_needed(3600.0, COARSE_BUILD_MIN_SPAN_SECS - 0.001);
        assert!(matches!(&runtime.coarse, CoarseBuildState::Dormant));
        runtime.start_coarse_if_needed(3600.0, COARSE_BUILD_MIN_SPAN_SECS);
        assert!(matches!(&runtime.coarse, CoarseBuildState::Building(_)));
    }

    /// The coarse column is sized from the duration the container reports, which
    /// backlog §1.13 already records as sometimes missing or nonsense. A dense
    /// `Vec` proportional to that number reaches 2.2 GB for a year, and a value
    /// large enough to saturate the cast aborts on the allocation itself.
    #[test]
    fn a_coarse_column_is_refused_when_the_duration_asks_for_more_than_the_budget() {
        // Real recordings stay well inside it: a day is 864,000 bins, about 6 MiB.
        assert!(coarse_column_fits_budget(60.0 * 60.0 * 24.0));
        // The edge of the budget itself.
        let max_bins = MAX_COARSE_WAVEFORM_BYTES / std::mem::size_of::<QuantizedWaveformBin>();
        assert!(coarse_column_fits_budget(max_bins as f64 * COARSE_BIN_SECS));
        // A year, which is what a broken duration reads like.
        assert!(!coarse_column_fits_budget(60.0 * 60.0 * 24.0 * 365.0));
        // Large enough that `as usize` saturates, so the multiply must be the guard.
        assert!(!coarse_column_fits_budget(1e300));

        let mut runtime = WaveWorkerRuntime::new();
        runtime.start_coarse_if_needed(1e300, COARSE_BUILD_MIN_SPAN_SECS);
        assert!(
            matches!(&runtime.coarse, CoarseBuildState::OverBudget),
            "an impossible duration must not allocate a column"
        );
        // It is not an audio failure: the waveform still works, window by window.
        assert!(runtime.coarse().is_none());
        assert!(!matches!(&runtime.coarse, CoarseBuildState::Unavailable(_)));
    }

    #[test]
    fn coarse_coverage_composes_adjacent_chunks_and_keeps_islands_separate() {
        let mut coverage = CoarseChunkCoverage::new(6);
        coverage.insert(1);
        coverage.insert(2);
        coverage.insert(4);
        assert_eq!(
            coverage.analyzed_spans(30.0, 330.0, 360.0),
            vec![(60.0, 180.0), (240.0, 300.0)]
        );
        assert!(coverage.covers_time_span(65.0, 175.0, 360.0));
        assert!(!coverage.covers_time_span(175.0, 245.0, 360.0));
    }

    #[test]
    fn failed_coarse_chunks_finish_selection_without_covering_their_span() {
        let mut coarse = CoarseWaveform::new(180.0);
        coarse.coverage.insert(0);
        assert!(coarse.mark_failed(1));
        coarse.coverage.insert(2);

        assert!(coarse.is_complete());
        assert_eq!(
            next_coarse_chunk(&coarse.coverage, &coarse.failed, 90.0),
            None
        );
        assert!(!coarse.covers_span(60.0, 120.0));
        assert_eq!(
            coarse.analyzed_spans(0.0, 180.0),
            vec![(0.0, 60.0), (120.0, 180.0)]
        );
        assert!(coarse.waveform_bins(60.0, 120.0).is_empty());
    }

    #[test]
    fn coarse_decoder_unavailable_remains_a_file_global_terminal_state() {
        let request_id = 1;
        let signature =
            WaveRequestSignature::new(90.0, 180.0, 180.0, 180.0, 320, 48).expect("valid signature");
        let window = waveform_window(90.0, 180.0, 180.0).expect("valid window");
        let state = Mutex::new(WaveSharedState {
            latest_request_id: request_id,
            status: WaveWorkerStatus::Working,
            raster: None,
        });
        let latest_request_id = AtomicU64::new(request_id);
        let mut runtime = WaveWorkerRuntime::new();
        runtime.coarse =
            CoarseBuildState::Unavailable(crate::audio_decode::AudioDecodeOpenError::NoAudioTrack);

        process_coarse_request(
            request_id,
            signature,
            window,
            false,
            &state,
            &latest_request_id,
            &mut runtime,
        );

        assert!(matches!(
            &runtime.coarse,
            CoarseBuildState::Unavailable(crate::audio_decode::AudioDecodeOpenError::NoAudioTrack)
        ));
        assert!(runtime.coarse().is_none());
        assert_eq!(lock_recover(&state).status, WaveWorkerStatus::NoAudioTrack);
    }

    #[test]
    fn coarse_chunk_composition_places_all_bins_in_its_owned_slice() {
        let mut analyzed = Vec::with_capacity(COARSE_BINS_PER_CHUNK);
        for offset in 0..COARSE_BINS_PER_CHUNK {
            analyzed.push(WaveformBin {
                start_secs: COARSE_CHUNK_SECS + offset as f64 * COARSE_BIN_SECS,
                duration_secs: COARSE_BIN_SECS,
                peak_l: 0.25,
                rms_l: 0.125,
                peak_r: 0.75,
                rms_r: 0.5,
                band_energy: [0.2, 0.3, 0.5],
                ..WaveformBin::default()
            });
        }
        let mut coarse = CoarseWaveform::new(180.0);
        assert!(coarse.apply_chunk(1, &analyzed));
        assert_eq!(coarse.coverage.marked_count(), 1);
        assert_eq!(coarse.analyzed_spans(0.0, 180.0), vec![(60.0, 120.0)]);
        let restored = coarse.waveform_bins(0.0, 180.0);
        assert_eq!(restored.len(), COARSE_BINS_PER_CHUNK);
        assert!((restored[0].start_secs - 60.0).abs() < 1.0e-9);
        assert!((restored[0].peak_l - 0.25).abs() <= 1.0 / 255.0);
    }

    /// The last chunk of a file is short whenever the duration is not a whole number of chunks,
    /// and the last bin is short whenever it is not a whole number of bins. `apply_chunk` returning
    /// false marks that chunk as unavailable, so a short tail must still compose successfully.
    #[test]
    fn coarse_chunk_composition_survives_a_short_final_chunk() {
        for duration_secs in [301.0_f64, 300.05, 299.999, 180.0, 61.0] {
            let chunk_index = coarse_chunk_count(duration_secs) - 1;
            let chunk_start_secs = chunk_index as f64 * COARSE_CHUNK_SECS;
            let chunk_end_secs = (chunk_start_secs + COARSE_CHUNK_SECS).min(duration_secs);
            let window = WaveWindow {
                start_secs: chunk_start_secs,
                end_secs: chunk_end_secs,
                content_start_secs: chunk_start_secs,
                content_end_secs: chunk_end_secs,
            };
            let range = waveform_analysis_range(window, duration_secs, COARSE_BIN_SECS)
                .unwrap_or_else(|| panic!("no analysis range for duration {duration_secs}"));
            let frames = ((range.end_secs - range.start_secs) * 48_000.0).ceil() as usize;
            let samples = vec![0.05_f32; frames * 2];
            let analysis = analyze_window_samples(
                &samples,
                48_000,
                range.start_secs,
                chunk_start_secs,
                chunk_end_secs,
                COARSE_BIN_SECS,
                COARSE_CHUNK_SECS,
                &|| false,
            )
            .unwrap_or_else(|| panic!("no analysis for duration {duration_secs}"));
            let mut coarse = CoarseWaveform::new(duration_secs);
            assert!(
                coarse.apply_chunk(chunk_index, &analysis.bins),
                "final chunk {chunk_index} of duration {duration_secs} failed to compose \
                 ({} bins produced)",
                analysis.bins.len()
            );
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
            revision: value as u64,
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

    /// 可視幅そのものが上限を超える環境 (3 面にまたがる超広幅ウィンドウ) では、上限が勝つ。
    ///
    /// **以前はここで丸めず「テクスチャ生成側の責務として残す」としていたが、生成側は
    /// 分割も縮小もしないまま残り、到達すれば必ずレンダースレッドがパニックしていた**
    /// (2026-08-27 Codex レビュー)。要求した幅がそのまま焼かれる以上、上限を知っている
    /// のは要求側しかない。可視部分は伸縮でわずかにぼやけるが、位置は時刻由来の UV で
    /// 決まるためずれない。分割して鮮鋭さを取り戻す案は backlog §1.140。
    #[test]
    fn an_oversized_visible_width_is_capped_at_the_texture_ceiling() {
        let visible_span_secs = 30.0;
        let first = WaveSpanRequest {
            span: WaveSpan::centered(100.0, visible_span_secs).expect("span"),
            stage: WaveRequestStage::FirstPaint,
        };
        for huge in [
            MAX_WAVEFORM_TEXTURE_WIDTH + 1,
            MAX_WAVEFORM_TEXTURE_WIDTH + 2_000,
            11_520,
            30_000,
        ] {
            assert_eq!(
                first.pixel_width(huge, visible_span_secs),
                MAX_WAVEFORM_TEXTURE_WIDTH,
                "a {huge}px window must not ask for a texture the GPU refuses"
            );
        }
        // 上限内の可視幅はこれまでどおり下げない。
        assert_eq!(
            first.pixel_width(MAX_WAVEFORM_TEXTURE_WIDTH, visible_span_secs),
            MAX_WAVEFORM_TEXTURE_WIDTH
        );
        assert_eq!(first.pixel_width(1_920, visible_span_secs), 1_920);
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
