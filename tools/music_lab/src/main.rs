use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use music_core::{
    AnalysisConfig, AudioStreamInfo, DecodedAudio, MusicBookmark, PlaybackSnapshot,
    SPECTRUM_NOTE_MAX_MIDI, SPECTRUM_NOTE_MIN_MIDI, SpectrumAnalysis, SpectrumAnalyzer,
    TimelineAnalysis, WaveformBin, analyze_stereo_timeline, resample_linear_stereo,
};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::conv::IntoSample;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

const TIMELINE_WAVEFORM_H: f32 = 68.0;
const TIMELINE_METRICS_H: f32 = 44.0;
const TIMELINE_METRIC_LANE_COUNT: usize = 2;
const TIMELINE_LOUDNESS_ROOT_LANE_FRACTION: f32 = 0.81;
const TIMELINE_ROOT_MIN_SEGMENT_SECS: f64 = 0.050;
const TIMELINE_ROOT_MAX_SEGMENT_SECS: f64 = 0.100;
const TIMELINE_ROOT_TRANSIENT_THRESHOLD: f32 = 0.16;
const TIMELINE_KEY_WINDOW_SECS: f64 = 6.0;
const TIMELINE_KEY_TRANSIENT_PENALTY_START: f32 = 0.22;
const TIMELINE_KEY_TRANSIENT_PENALTY_END: f32 = 0.90;
const TIMELINE_KEY_DENSITY_PENALTY_START: f32 = 0.24;
const TIMELINE_KEY_DENSITY_PENALTY_END: f32 = 0.86;
const TIMELINE_KEY_TRANSIENT_REDUCTION: f32 = 0.56;
const TIMELINE_KEY_DENSITY_REDUCTION: f32 = 0.30;
const TIMELINE_KEY_DISPLAY_FLOOR: f32 = 0.18;
const TIMELINE_KEY_KRUMHANSL_WEIGHT: f32 = 0.64;
const TIMELINE_KEY_TEMPERLEY_WEIGHT: f32 = 0.36;
const TIMELINE_KEY_KRUMHANSL_MAJOR_PROFILE: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const TIMELINE_KEY_KRUMHANSL_MINOR_PROFILE: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];
const TIMELINE_KEY_TEMPERLEY_MAJOR_PROFILE: [f32; 12] =
    [5.0, 2.0, 3.5, 2.0, 4.5, 4.0, 2.0, 4.5, 2.0, 3.5, 1.5, 4.0];
const TIMELINE_KEY_TEMPERLEY_MINOR_PROFILE: [f32; 12] =
    [5.0, 2.0, 3.5, 4.5, 2.0, 4.0, 2.0, 4.5, 3.5, 2.0, 1.5, 4.0];
const TIMELINE_INNER_GAP: f32 = 4.0;
const TIMELINE_ROW_GAP: f32 = 12.0;
const TIMELINE_TEXTURE_MAX_WIDTH: usize = 4096;
const TIMELINE_ROW_TEXTURE_UPLOAD_BUDGET_PER_FRAME: usize = 1;
const TIMELINE_PARTIAL_CHUNK_SECS: f64 = 5.0;
const LOAD_MESSAGES_PER_FRAME: usize = 16;
const TIMELINE_ROW_SECS_CHOICES: [f64; 8] = [5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0];
const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "alac"];
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm"];
const MEDIA_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "alac", "mp4", "m4v", "mov", "mkv", "webm",
];
const SPECTRUM_BANDS: usize = 108;
const SPECTRUM_TRAIL_DECAY: f32 = 0.994;
const SPECTRUM_REFRESH_INTERVAL: Duration = Duration::from_millis(5);
const SPECTRUM_KEYBOARD_H: f32 = 34.0;
const SPECTRUM_PANEL_GAP: f32 = 8.0;
const KEY_HIGHLIGHT_DECAY: f32 = 0.925;
const KEY_HIGHLIGHT_MIN_PEAK: f32 = 0.035;
const KEY_SUSTAIN_ATTACK: f32 = 0.18;
const KEY_SUSTAIN_RELEASE: f32 = 0.965;
const SPECTRUM_ANALYSIS_MIN_HZ: f32 = 20.0;
const SPECTRUM_AXIS_MIN_HZ: f32 = SPECTRUM_ANALYSIS_MIN_HZ;
const SPECTRUM_VIEW_MAX_HZ: f32 = 18_000.0;
const KEYBOARD_DISPLAY_MIN_MIDI: u8 = 12; // C0
const KEYBOARD_DISPLAY_MAX_MIDI: u8 = 143; // B10, clipped by the 18 kHz axis.
const PERF_LOG_INTERVAL: Duration = Duration::from_secs(2);
const BEAT_GRID_MIN_CONFIDENCE: f32 = 0.55;
const TRANSIENT_ACCENT_MIN: f32 = 0.42;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native(
        "mIV music lab",
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            Ok(Box::<MusicLabApp>::default())
        }),
    )
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let font_paths = [
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
    ];
    for path in font_paths {
        if let Ok(data) = std::fs::read(path) {
            fonts.font_data.insert(
                "japanese".to_owned(),
                Arc::new(egui::FontData::from_owned(data)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "japanese".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "japanese".to_owned());
            break;
        }
    }
    ctx.set_fonts(fonts);
    apply_dark_visuals(ctx);
}

fn apply_dark_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = app_panel_bg();
    visuals.window_fill = app_panel_bg();
    visuals.window_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 82, 94));
    visuals.extreme_bg_color = app_bg();
    visuals.faint_bg_color = egui::Color32::from_rgb(13, 16, 19);
    visuals.code_bg_color = egui::Color32::from_rgb(14, 18, 22);
    visuals.text_edit_bg_color = Some(egui::Color32::from_rgb(13, 16, 19));
    visuals.widgets.noninteractive.bg_fill = app_panel_bg();
    visuals.widgets.noninteractive.weak_bg_fill = app_soft_bg();
    visuals.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(68, 80, 92));
    visuals.widgets.noninteractive.fg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(222, 228, 234));
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(24, 29, 34);
    visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(24, 29, 34);
    visuals.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(86, 98, 110));
    visuals.widgets.inactive.fg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(232, 237, 242));
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(38, 47, 56);
    visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(38, 47, 56);
    visuals.widgets.hovered.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(112, 132, 150));
    visuals.widgets.hovered.fg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(245, 248, 250));
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(52, 65, 76);
    visuals.widgets.active.weak_bg_fill = egui::Color32::from_rgb(52, 65, 76);
    visuals.widgets.active.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(132, 154, 174));
    visuals.widgets.active.fg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 255, 255));
    visuals.widgets.open = visuals.widgets.active;
    visuals.selection.bg_fill = egui::Color32::from_rgb(44, 106, 170);
    visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(250, 252, 255));
    visuals.override_text_color = Some(egui::Color32::from_rgb(238, 243, 248));
    visuals.weak_text_color = Some(egui::Color32::from_rgb(204, 214, 224));
    visuals.disabled_alpha = 0.88;
    ctx.set_visuals(visuals);
}

fn app_bg() -> egui::Color32 {
    egui::Color32::BLACK
}

fn app_panel_bg() -> egui::Color32 {
    egui::Color32::from_rgb(6, 8, 10)
}

fn app_soft_bg() -> egui::Color32 {
    egui::Color32::from_rgb(10, 13, 16)
}

fn app_panel_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(app_panel_bg())
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(76, 92, 108, 95),
        ))
}

struct LoadedTrack {
    path: PathBuf,
    decoded: Arc<DecodedAudio>,
    analysis: Arc<TimelineAnalysis>,
}

#[derive(Clone, Debug)]
struct LoadingTrack {
    path: PathBuf,
    info: AudioStreamInfo,
}

enum LoadMsg {
    Probed {
        path: PathBuf,
        info: AudioStreamInfo,
    },
    PartialTimeline {
        bins: Vec<WaveformBin>,
        decoded_duration_secs: f64,
    },
    Status(String),
    Loaded(Box<LoadedTrack>),
    Failed(String),
}

struct SpectrumMsg {
    analysis: SpectrumAnalysis,
    compute_ms: f32,
}

struct SpectrumRequest {
    position_secs: f64,
}

#[derive(Default)]
struct FrameStats {
    last_frame: Option<Instant>,
    last_log: Option<Instant>,
    perf_log: Option<PerfLogSink>,
    fps: f32,
    frame_ms: f32,
    update_ms: f32,
    poll_ms: f32,
    top_ms: f32,
    left_ms: f32,
    right_ms: f32,
    bottom_ms: f32,
    central_ms: f32,
    spectrum_compute_ms: f32,
    timeline_rows: usize,
    timeline_bins: usize,
    timeline_cache_misses: usize,
}

struct PerfLogSink {
    tx: mpsc::Sender<String>,
    path: PathBuf,
}

impl FrameStats {
    fn record_frame(&mut self) {
        let now = Instant::now();
        let Some(last) = self.last_frame.replace(now) else {
            return;
        };
        let dt = now.saturating_duration_since(last).as_secs_f32();
        if dt <= 0.0 {
            return;
        }
        let frame_ms = dt * 1000.0;
        let fps = 1.0 / dt;
        if self.fps <= 0.0 {
            self.fps = fps;
            self.frame_ms = frame_ms;
        } else {
            self.fps = self.fps * 0.90 + fps * 0.10;
            self.frame_ms = self.frame_ms * 0.90 + frame_ms * 0.10;
        }
    }

    fn record_update(&mut self, duration: Duration) {
        Self::smooth_ms(&mut self.update_ms, duration);
    }

    fn record_poll(&mut self, duration: Duration) {
        Self::smooth_ms(&mut self.poll_ms, duration);
    }

    fn record_top(&mut self, duration: Duration) {
        Self::smooth_ms(&mut self.top_ms, duration);
    }

    fn record_left(&mut self, duration: Duration) {
        Self::smooth_ms(&mut self.left_ms, duration);
    }

    fn record_right(&mut self, duration: Duration) {
        Self::smooth_ms(&mut self.right_ms, duration);
    }

    fn record_bottom(&mut self, duration: Duration) {
        Self::smooth_ms(&mut self.bottom_ms, duration);
    }

    fn record_central(&mut self, duration: Duration, timeline: TimelineDrawStats) {
        Self::smooth_ms(&mut self.central_ms, duration);
        self.timeline_rows = timeline.visible_rows;
        self.timeline_bins = timeline.drawn_bins;
        self.timeline_cache_misses = timeline.cache_misses;
    }

    fn record_spectrum_compute(&mut self, compute_ms: f32) {
        Self::smooth_value(&mut self.spectrum_compute_ms, compute_ms.max(0.0));
    }

    fn log_path(&self) -> PathBuf {
        self.perf_log
            .as_ref()
            .map(|sink| sink.path.clone())
            .unwrap_or_else(perf_log_path)
    }

    fn maybe_log(&mut self, playing: bool, spectrum_pending: bool) {
        let now = Instant::now();
        if self
            .last_log
            .is_some_and(|last| now.saturating_duration_since(last) < PERF_LOG_INTERVAL)
        {
            return;
        }
        self.last_log = Some(now);

        if self.perf_log.is_none() {
            self.perf_log = PerfLogSink::spawn();
        }
        let Some(sink) = &self.perf_log else {
            return;
        };
        let ts_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        let line = format!(
            concat!(
                "ts_ms={ts_ms} fps={fps:.1} frame_ms={frame_ms:.1} ",
                "update_ms={update_ms:.1} poll_ms={poll_ms:.1} top_ms={top_ms:.1} ",
                "left_ms={left_ms:.1} right_ms={right_ms:.1} bottom_ms={bottom_ms:.1} ",
                "central_ms={central_ms:.1} spectrum_compute_ms={spectrum_compute_ms:.1} ",
                "timeline_rows={timeline_rows} timeline_bins={timeline_bins} ",
                "timeline_cache_misses={timeline_cache_misses} ",
                "playing={playing} spectrum_pending={spectrum_pending}"
            ),
            ts_ms = ts_ms,
            fps = self.fps,
            frame_ms = self.frame_ms,
            update_ms = self.update_ms,
            poll_ms = self.poll_ms,
            top_ms = self.top_ms,
            left_ms = self.left_ms,
            right_ms = self.right_ms,
            bottom_ms = self.bottom_ms,
            central_ms = self.central_ms,
            spectrum_compute_ms = self.spectrum_compute_ms,
            timeline_rows = self.timeline_rows,
            timeline_bins = self.timeline_bins,
            timeline_cache_misses = self.timeline_cache_misses,
            playing = playing,
            spectrum_pending = spectrum_pending,
        );
        let _ = sink.tx.send(line);
    }

    fn smooth_ms(slot: &mut f32, duration: Duration) {
        Self::smooth_value(slot, duration.as_secs_f32() * 1000.0);
    }

    fn smooth_value(slot: &mut f32, value: f32) {
        if *slot <= 0.0 {
            *slot = value;
        } else {
            *slot = *slot * 0.85 + value * 0.15;
        }
    }
}

impl PerfLogSink {
    fn spawn() -> Option<Self> {
        let path = perf_log_path();
        let thread_path = path.clone();
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::Builder::new()
            .name("music-lab-perf-log".into())
            .spawn(move || {
                if let Ok(mut file) = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&thread_path)
                {
                    let _ = writeln!(file, "# mIV music lab perf log");
                    while let Ok(line) = rx.recv() {
                        let _ = writeln!(file, "{line}");
                        let _ = file.flush();
                    }
                }
            })
            .ok()?;
        Some(Self { tx, path })
    }
}

#[derive(Default)]
struct TimelineTextureCache {
    key: Option<TimelineTextureCacheKey>,
    rows: Vec<Option<TimelineRowTexture>>,
    pending: Vec<Option<TimelinePendingRow>>,
    row_versions: Vec<u64>,
    generation: u64,
    raster_tx: Option<mpsc::Sender<TimelineRasterRequest>>,
    raster_rx: Option<mpsc::Receiver<TimelineRasterResult>>,
    raster_cancel: Option<Arc<AtomicBool>>,
}

struct TimelineRowTexture {
    key: TimelineTextureCacheKey,
    row_version: u64,
    texture: egui::TextureHandle,
    represented_bins: usize,
}

#[derive(Clone, Copy, Debug)]
struct TimelinePendingRow {
    key: TimelineTextureCacheKey,
    generation: u64,
    row_version: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TimelineTextureCacheKey {
    width_px: usize,
    waveform_h_px: usize,
    gap_px: usize,
    metrics_h_px: usize,
    row_secs_millis: u32,
    rows: usize,
    dark: bool,
}

struct TimelineRasterRequest {
    generation: u64,
    row_version: u64,
    row: usize,
    row_secs: f64,
    key: TimelineTextureCacheKey,
    bins: Vec<WaveformBin>,
}

struct TimelineRasterResult {
    generation: u64,
    row_version: u64,
    row: usize,
    key: TimelineTextureCacheKey,
    image: egui::ColorImage,
    represented_bins: usize,
}

impl TimelineTextureCache {
    fn clear(&mut self) {
        self.cancel_worker();
        self.key = None;
        self.rows.clear();
        self.pending.clear();
        self.row_versions.clear();
    }

    fn ensure(&mut self, key: TimelineTextureCacheKey) {
        if self.key == Some(key) {
            return;
        }
        self.cancel_worker();
        self.generation = self.generation.wrapping_add(1);
        self.key = Some(key);
        self.rows.clear();
        self.rows.resize_with(key.rows, || None);
        self.pending.clear();
        self.pending.resize_with(key.rows, || None);
        self.row_versions.clear();
        self.row_versions.resize(key.rows, 0);
        self.spawn_worker();
    }

    fn cancel_worker(&mut self) {
        if let Some(cancel) = self.raster_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.raster_tx = None;
        self.raster_rx = None;
    }

    fn spawn_worker(&mut self) {
        let (request_tx, request_rx) = mpsc::channel::<TimelineRasterRequest>();
        let (result_tx, result_rx) = mpsc::channel::<TimelineRasterResult>();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let spawned = std::thread::Builder::new()
            .name("music-lab-timeline-raster".into())
            .spawn(move || run_timeline_raster_worker(request_rx, result_tx, worker_cancel));
        if spawned.is_ok() {
            self.raster_tx = Some(request_tx);
            self.raster_rx = Some(result_rx);
            self.raster_cancel = Some(cancel);
        }
    }

    fn invalidate_all_rows(&mut self) {
        for version in &mut self.row_versions {
            *version = version.wrapping_add(1);
        }
    }

    fn invalidate_time_range(&mut self, start_secs: f64, end_secs: f64, row_secs: f64) {
        if self.rows.is_empty()
            || !start_secs.is_finite()
            || !end_secs.is_finite()
            || row_secs <= 0.0
        {
            return;
        }
        let start_row = (start_secs.max(0.0) / row_secs).floor().max(0.0) as usize;
        let end_row = (end_secs.max(start_secs).max(0.0) / row_secs)
            .floor()
            .max(start_row as f64) as usize;
        let last_row = self.rows.len().saturating_sub(1);
        for row in start_row.min(last_row)..=end_row.min(last_row) {
            if let Some(version) = self.row_versions.get_mut(row) {
                *version = version.wrapping_add(1);
            }
        }
    }

    fn poll_finished_rows(&mut self, ctx: &egui::Context, upload_budget: usize) -> (usize, usize) {
        let Some(rx) = self.raster_rx.as_ref() else {
            return (0, 0);
        };
        let mut uploaded = 0;
        let mut represented_bins = 0;
        while uploaded < upload_budget {
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.raster_tx = None;
                    self.raster_rx = None;
                    self.raster_cancel = None;
                    break;
                }
            };
            if Some(result.key) != self.key || result.generation != self.generation {
                continue;
            }
            let Some(slot) = self.rows.get_mut(result.row) else {
                continue;
            };
            if let Some(pending_slot) = self.pending.get_mut(result.row) {
                if pending_slot.as_ref().is_some_and(|pending| {
                    pending.generation == result.generation
                        && pending.row_version <= result.row_version
                }) {
                    *pending_slot = None;
                }
            }
            if slot.as_ref().is_some_and(|row_texture| {
                row_texture.key == result.key && row_texture.row_version > result.row_version
            }) {
                continue;
            }
            let texture = ctx.load_texture(
                format!(
                    "music_timeline_row_{}_{}_{}_{}x{}_{}",
                    result.generation,
                    result.row_version,
                    result.row,
                    result.key.width_px,
                    result.key.height_px(),
                    result.key.dark as u8
                ),
                result.image,
                egui::TextureOptions::LINEAR,
            );
            *slot = Some(TimelineRowTexture {
                key: result.key,
                row_version: result.row_version,
                texture,
                represented_bins: result.represented_bins,
            });
            uploaded += 1;
            represented_bins += result.represented_bins;
        }
        (uploaded, represented_bins)
    }

    fn row_texture(
        &mut self,
        bins: &[WaveformBin],
        row: usize,
        row_secs: f64,
        key: TimelineTextureCacheKey,
    ) -> (Option<(&egui::TextureHandle, usize)>, bool) {
        self.ensure(key);
        if row >= self.rows.len() {
            return (None, false);
        }
        let stale = self.rows[row]
            .as_ref()
            .is_some_and(|row_texture| row_texture.key != key);
        let missing = self.rows[row].is_none() || stale;
        let row_version = self.row_versions.get(row).copied().unwrap_or(0);
        let mut request_sent = false;
        let needs_newer_texture = self.rows[row].as_ref().is_none_or(|row_texture| {
            row_texture.key != key || row_texture.row_version < row_version
        });
        if missing || needs_newer_texture {
            let pending_current = self.pending[row]
                .is_some_and(|pending| pending.key == key && pending.generation == self.generation);
            if !pending_current && let Some(tx) = self.raster_tx.as_ref() {
                let row_start = row as f64 * row_secs;
                let row_bins = timeline_bins_for_raster(bins, row_start, row_secs);
                let request = TimelineRasterRequest {
                    generation: self.generation,
                    row_version,
                    row,
                    row_secs,
                    key,
                    bins: row_bins,
                };
                if tx.send(request).is_ok() {
                    self.pending[row] = Some(TimelinePendingRow {
                        key,
                        generation: self.generation,
                        row_version,
                    });
                    request_sent = true;
                }
            }
        }
        let Some(row) = self.rows[row].as_ref() else {
            return (None, request_sent);
        };
        if row.key == key {
            (Some((&row.texture, row.represented_bins)), request_sent)
        } else {
            (None, request_sent)
        }
    }

    fn row_is_fresh(&self, row: usize, key: TimelineTextureCacheKey) -> bool {
        let row_version = self.row_versions.get(row).copied().unwrap_or(0);
        self.rows
            .get(row)
            .and_then(Option::as_ref)
            .is_some_and(|row_texture| {
                row_texture.key == key && row_texture.row_version >= row_version
            })
    }
}

impl Drop for TimelineTextureCache {
    fn drop(&mut self) {
        self.cancel_worker();
    }
}

fn run_timeline_raster_worker(
    request_rx: mpsc::Receiver<TimelineRasterRequest>,
    result_tx: mpsc::Sender<TimelineRasterResult>,
    cancel: Arc<AtomicBool>,
) {
    while !cancel.load(Ordering::Relaxed) {
        let request = match request_rx.recv() {
            Ok(request) => request,
            Err(_) => break,
        };
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let row_start = request.row as f64 * request.row_secs;
        let (image, represented_bins) = render_timeline_row_image(
            row_start,
            request.row_secs,
            &request.bins,
            request.key.width_px,
            request.key.waveform_h_px,
            request.key.gap_px,
            request.key.metrics_h_px,
            request.key.dark,
        );
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if result_tx
            .send(TimelineRasterResult {
                generation: request.generation,
                row_version: request.row_version,
                row: request.row,
                key: request.key,
                image,
                represented_bins,
            })
            .is_err()
        {
            break;
        }
    }
}

impl TimelineTextureCacheKey {
    fn height_px(self) -> usize {
        self.waveform_h_px + self.gap_px + self.metrics_h_px
    }
}

#[derive(Default)]
struct MusicLabApp {
    active_path: Option<PathBuf>,
    track: Option<LoadedTrack>,
    loading_track: Option<LoadingTrack>,
    partial_analysis: Option<TimelineAnalysis>,
    load_rx: Option<mpsc::Receiver<LoadMsg>>,
    load_cancel: Option<Arc<AtomicBool>>,
    load_status: String,
    player: Option<LabPlayer>,
    bookmarks: Vec<MusicBookmark>,
    next_bookmark_id: u64,
    selected_bookmark: Option<u64>,
    spectrum_bands: Vec<f32>,
    spectrum_trail: Vec<f32>,
    spectrum_prev_bands: Vec<f32>,
    spectrum_onsets: Vec<f32>,
    spectrum_notes: Vec<f32>,
    spectrum_note_sustain: Vec<f32>,
    spectrum_note_trail: Vec<f32>,
    spectrum_tx: Option<mpsc::Sender<SpectrumRequest>>,
    spectrum_rx: Option<mpsc::Receiver<SpectrumMsg>>,
    spectrum_pending: bool,
    last_spectrum_request: Option<Instant>,
    timeline_cache: TimelineTextureCache,
    timeline_row_secs: f64,
    timeline_follow_playhead: bool,
    frame_stats: FrameStats,
}

impl eframe::App for MusicLabApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        apply_dark_visuals(ctx);
        let update_start = Instant::now();
        self.frame_stats.record_frame();

        let stage_start = Instant::now();
        self.handle_dropped_files(ctx);
        self.poll_loader(ctx);
        self.poll_spectrum_analyzer(ctx);
        self.frame_stats.record_poll(stage_start.elapsed());

        let stage_start = Instant::now();
        self.draw_top_bar(ctx);
        self.frame_stats.record_top(stage_start.elapsed());

        let stage_start = Instant::now();
        self.draw_left_panel(ctx);
        self.frame_stats.record_left(stage_start.elapsed());

        let stage_start = Instant::now();
        self.draw_right_panel(ctx);
        self.frame_stats.record_right(stage_start.elapsed());

        let stage_start = Instant::now();
        self.draw_bottom_bar(ctx);
        self.frame_stats.record_bottom(stage_start.elapsed());

        let stage_start = Instant::now();
        let mut timeline_stats = TimelineDrawStats::default();
        let track = self.track.as_ref();
        let loading_track = self.loading_track.as_ref();
        let partial_analysis = self.partial_analysis.as_ref();
        let player = self.player.as_ref();
        let snap = self.player_snapshot();
        let row_secs = self.timeline_row_secs();
        let load_status = self.load_status.clone();
        let timeline_cache = &mut self.timeline_cache;
        let timeline_follow_playhead = &mut self.timeline_follow_playhead;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(app_bg()))
            .show(ctx, |ui| {
                if let Some(track) = track {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            timeline_stats = draw_timeline(
                                ui,
                                &track.analysis,
                                track.decoded.info.duration_secs,
                                player,
                                snap,
                                timeline_cache,
                                row_secs,
                                timeline_follow_playhead,
                            );
                        });
                } else if let (Some(loading_track), Some(partial_analysis)) =
                    (loading_track, partial_analysis)
                {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            timeline_stats = draw_timeline(
                                ui,
                                partial_analysis,
                                loading_track
                                    .info
                                    .duration_secs
                                    .max(partial_analysis.stream.duration_secs),
                                player,
                                snap,
                                timeline_cache,
                                row_secs,
                                timeline_follow_playhead,
                            );
                        });
                } else if let Some(loading_track) = loading_track {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            timeline_stats =
                                draw_pending_timeline(ui, loading_track, row_secs, &load_status);
                        });
                } else {
                    draw_empty_state(ui, &load_status);
                }
            });
        self.frame_stats
            .record_central(stage_start.elapsed(), timeline_stats);
        self.frame_stats.record_update(update_start.elapsed());

        let playing = self.player.as_ref().is_some_and(|p| p.snapshot().playing);
        self.frame_stats.maybe_log(playing, self.spectrum_pending);

        if playing {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}

impl MusicLabApp {
    fn draw_top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("music_lab_top")
            .exact_height(48.0)
            .frame(app_panel_frame())
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    if ui.button("Open").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Media", MEDIA_EXTENSIONS)
                            .add_filter("Audio", AUDIO_EXTENSIONS)
                            .add_filter("Video", VIDEO_EXTENSIONS)
                            .pick_file()
                        {
                            self.start_load(path, ctx);
                        }
                    }

                    let can_play = self.player.is_some();
                    let playing = self.player.as_ref().is_some_and(|p| p.snapshot().playing);
                    if ui
                        .add_enabled(
                            can_play,
                            egui::Button::new(if playing { "Pause" } else { "Play" }),
                        )
                        .clicked()
                        && let Some(player) = &self.player
                    {
                        player.set_playing(!playing);
                    }

                    if ui
                        .add_enabled(can_play, egui::Button::new("Stop"))
                        .clicked()
                        && let Some(player) = &self.player
                    {
                        player.seek_secs(0.0);
                        player.set_playing(false);
                    }

                    ui.separator();
                    let snap = self.player_snapshot();
                    let duration_secs = self.display_duration_secs();
                    ui.label(format!(
                        "{} / {}",
                        format_time(snap.position_secs),
                        format_time(duration_secs)
                    ));
                    if snap.effect_chain_active {
                        ui.label(format!("FX {} samples", snap.effect_latency_samples));
                    } else {
                        ui.label("FX: lab no-op boundary");
                    }
                    ui.separator();
                    let mut row_secs = self.timeline_row_secs();
                    egui::ComboBox::from_id_salt("music_lab_row_secs")
                        .selected_text(format!("Row {}", format_row_secs(row_secs)))
                        .show_ui(ui, |ui| {
                            for choice in TIMELINE_ROW_SECS_CHOICES {
                                ui.selectable_value(&mut row_secs, choice, format_row_secs(choice));
                            }
                        });
                    if (row_secs - self.timeline_row_secs()).abs() > f64::EPSILON {
                        self.timeline_row_secs = row_secs;
                        self.timeline_cache.clear();
                    }
                    ui.separator();
                    ui.label(format!(
                        "FPS {:.1}  {:.1} ms  UI {:.1} C {:.1} B {:.1} raster {} miss {}",
                        self.frame_stats.fps,
                        self.frame_stats.frame_ms,
                        self.frame_stats.update_ms,
                        self.frame_stats.central_ms,
                        self.frame_stats.bottom_ms,
                        self.frame_stats.timeline_bins,
                        self.frame_stats.timeline_cache_misses
                    ));
                    if self.spectrum_pending {
                        ui.label("Spectrum: analyzing");
                    } else if self.frame_stats.spectrum_compute_ms > 0.0 {
                        ui.label(format!(
                            "Spectrum worker {:.1} ms",
                            self.frame_stats.spectrum_compute_ms
                        ));
                    }
                    if !self.load_status.is_empty() {
                        ui.separator();
                        ui.label(&self.load_status);
                    }
                });
            });
    }

    fn draw_left_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("music_lab_left")
            .resizable(true)
            .default_width(220.0)
            .frame(app_panel_frame())
            .show(ctx, |ui| {
                ui.heading("Bookmarks");
                if ui
                    .add_enabled(self.track.is_some(), egui::Button::new("Add current"))
                    .clicked()
                {
                    let pos = self.player_snapshot().position_secs;
                    self.bookmarks.push(MusicBookmark {
                        id: self.next_bookmark_id,
                        position_secs: pos,
                        title: format!("Bookmark {}", self.bookmarks.len() + 1),
                    });
                    self.next_bookmark_id += 1;
                }
                ui.separator();
                for bookmark in &self.bookmarks {
                    let selected = self.selected_bookmark == Some(bookmark.id);
                    let label = format!(
                        "{}  {}",
                        format_time(bookmark.position_secs),
                        bookmark.title
                    );
                    if ui.selectable_label(selected, label).clicked() {
                        self.selected_bookmark = Some(bookmark.id);
                        if let Some(player) = &self.player {
                            player.seek_secs(bookmark.position_secs);
                        }
                    }
                }
            });
    }

    fn draw_right_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("music_lab_right")
            .resizable(true)
            .default_width(260.0)
            .frame(app_panel_frame())
            .show(ctx, |ui| {
                ui.heading("Details");
                if let Some(track) = &self.track {
                    ui.label(track.path.display().to_string());
                    ui.separator();
                    ui.label(format!(
                        "Duration: {}",
                        format_time(track.decoded.info.duration_secs)
                    ));
                    ui.label(format!(
                        "Sample rate: {} Hz",
                        track.decoded.info.sample_rate
                    ));
                    ui.label(format!("Channels: {}", track.decoded.info.channels));
                    ui.label(format!("Wave bins: {}", track.analysis.bins.len()));
                    ui.label(format!(
                        "Row: {}",
                        format_row_secs(self.timeline_row_secs())
                    ));
                    if let Some(bpm) = track.analysis.beat_grid.bpm {
                        let grid_visible =
                            track.analysis.beat_grid.confidence >= BEAT_GRID_MIN_CONFIDENCE;
                        ui.label(format!(
                            "BPM: {:.1} ({:?}, {:.0}%)",
                            bpm,
                            track.analysis.beat_grid.status,
                            track.analysis.beat_grid.confidence * 100.0
                        ));
                        ui.label(if grid_visible {
                            "Beat grid: visible lab estimate"
                        } else {
                            "Beat grid: hidden below confidence threshold"
                        });
                    } else {
                        ui.label("BPM: not estimated");
                    }
                    ui.heading("Tags");
                    ui.add_enabled(
                        false,
                        egui::TextEdit::singleline(&mut "#lab #music".to_string()),
                    );
                    ui.label("本体統合時に既存 tags.db へ接続する想定");
                    ui.separator();
                    ui.heading("Perf");
                    ui.label(format!(
                        "Frame/UI: {:.1} / {:.1} ms",
                        self.frame_stats.frame_ms, self.frame_stats.update_ms
                    ));
                    ui.label(format!(
                        "Central: {:.1} ms, rows {}, raster bins {}, misses {}",
                        self.frame_stats.central_ms,
                        self.frame_stats.timeline_rows,
                        self.frame_stats.timeline_bins,
                        self.frame_stats.timeline_cache_misses
                    ));
                    ui.label(format!(
                        "Spectrum draw/worker: {:.1} / {:.1} ms",
                        self.frame_stats.bottom_ms, self.frame_stats.spectrum_compute_ms
                    ));
                    ui.label(format!("Log: {}", self.frame_stats.log_path().display()));
                } else if let Some(loading) = &self.loading_track {
                    ui.label(loading.path.display().to_string());
                    ui.separator();
                    ui.label("Status: loading in background");
                    ui.label(format!(
                        "Duration: {}",
                        format_time(loading.info.duration_secs)
                    ));
                    ui.label(format!("Sample rate: {} Hz", loading.info.sample_rate));
                    ui.label(format!("Channels: {}", loading.info.channels));
                    ui.label(format!(
                        "Row: {}",
                        format_row_secs(self.timeline_row_secs())
                    ));
                    if let Some(partial) = &self.partial_analysis {
                        ui.label(format!("Wave bins: {} loaded", partial.bins.len()));
                    } else {
                        ui.label("Wave bins: analyzing");
                    }
                    ui.label(if self.player.is_some() {
                        "Playback: streaming during analysis"
                    } else {
                        "Playback: available after decode"
                    });
                    ui.separator();
                    ui.heading("Perf");
                    ui.label(format!(
                        "Frame/UI: {:.1} / {:.1} ms",
                        self.frame_stats.frame_ms, self.frame_stats.update_ms
                    ));
                    ui.label(format!("Log: {}", self.frame_stats.log_path().display()));
                } else {
                    ui.label("Open or drop an audio/video file to inspect its audio track.");
                }
            });
    }

    fn draw_bottom_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("music_lab_bottom")
            .exact_height(190.0)
            .frame(app_panel_frame())
            .show(ctx, |ui| {
                if self.track.is_none() {
                    let text = if self.loading_track.is_some() {
                        "Spectrum analyzer will start after timeline analysis"
                    } else {
                        "Spectrum analyzer placeholder"
                    };
                    ui.centered_and_justified(|ui| ui.label(text));
                    return;
                }
                draw_spectrum(
                    ui,
                    &self.spectrum_bands,
                    &mut self.spectrum_trail,
                    &mut self.spectrum_prev_bands,
                    &mut self.spectrum_onsets,
                    &self.spectrum_notes,
                    &mut self.spectrum_note_sustain,
                    &mut self.spectrum_note_trail,
                );
            });
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped_files = ctx.input(|input| input.raw.dropped_files.clone());
        let Some(path) = dropped_files.into_iter().find_map(|file| file.path) else {
            return;
        };
        if is_supported_media_path(&path) {
            self.start_load(path, ctx);
        } else {
            self.load_status = format!(
                "Unsupported drop: {} (audio/video files only)",
                path.display()
            );
        }
    }

    fn start_load(&mut self, path: PathBuf, ctx: &egui::Context) {
        if let Some(cancel) = self.load_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.load_status = format!("Probing {}", path.display());
        self.active_path = Some(path.clone());
        self.player = None;
        self.track = None;
        self.loading_track = None;
        self.partial_analysis = None;
        self.spectrum_bands.clear();
        self.spectrum_trail.clear();
        self.spectrum_prev_bands.clear();
        self.spectrum_onsets.clear();
        self.spectrum_notes.clear();
        self.spectrum_note_sustain.clear();
        self.spectrum_note_trail.clear();
        self.spectrum_tx = None;
        self.spectrum_rx = None;
        self.spectrum_pending = false;
        self.last_spectrum_request = None;
        self.timeline_follow_playhead = false;
        self.timeline_cache.clear();
        let streaming_sink = match LabPlayer::new_streaming(AudioStreamInfo::default(), true) {
            Ok((player, sink)) => {
                self.player = Some(player);
                Some(sink)
            }
            Err(err) => {
                self.load_status = format!("Probing {}; playback pending: {err}", path.display());
                None
            }
        };
        let (tx, rx) = mpsc::channel();
        let load_path = path.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let spawn_result = std::thread::Builder::new()
            .name("music-lab-load".into())
            .spawn(move || {
                match probe_audio_file(&load_path) {
                    Ok(info) => {
                        if tx
                            .send(LoadMsg::Probed {
                                path: load_path.clone(),
                                info,
                            })
                            .is_err()
                            || worker_cancel.load(Ordering::Relaxed)
                        {
                            return;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(LoadMsg::Failed(err));
                        return;
                    }
                }
                let decode_status = if streaming_sink.is_some() {
                    "Streaming playback; decoding timeline samples..."
                } else {
                    "Decoding timeline samples; playback starts after decode..."
                };
                let _ = tx.send(LoadMsg::Status(decode_status.to_string()));
                let msg = match decode_audio_file(
                    &load_path,
                    &worker_cancel,
                    streaming_sink.as_ref(),
                    Some(&tx),
                ) {
                    Ok(decoded) => {
                        if worker_cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let _ = tx.send(LoadMsg::Probed {
                            path: load_path.clone(),
                            info: decoded.info,
                        });
                        let _ = tx.send(LoadMsg::Status("Analyzing timeline...".to_string()));
                        let analysis = analyze_stereo_timeline(
                            &decoded.stereo_samples,
                            decoded.info.sample_rate,
                            music_lab_analysis_config(),
                        );
                        if worker_cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        LoadMsg::Loaded(Box::new(LoadedTrack {
                            path: load_path,
                            decoded: Arc::new(decoded),
                            analysis: Arc::new(analysis),
                        }))
                    }
                    Err(err) => {
                        if let Some(sink) = &streaming_sink {
                            sink.fail();
                        }
                        LoadMsg::Failed(err)
                    }
                };
                let _ = tx.send(msg);
            });
        match spawn_result {
            Ok(_) => {
                self.load_rx = Some(rx);
                self.load_cancel = Some(cancel);
                ctx.request_repaint();
            }
            Err(err) => {
                self.load_status = format!("Failed to start loader: {err}");
                self.load_rx = None;
            }
        }
    }

    fn poll_loader(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.load_rx.as_ref() else {
            return;
        };
        let mut clear_loader = false;
        let mut handled = 0usize;
        loop {
            if handled >= LOAD_MESSAGES_PER_FRAME {
                ctx.request_repaint_after(Duration::from_millis(1));
                break;
            }
            handled += 1;
            match rx.try_recv() {
                Ok(LoadMsg::Probed { path, info }) => {
                    if let Some(player) = &self.player {
                        player.set_duration_secs(info.duration_secs);
                    }
                    if let Some(partial) = self.partial_analysis.as_mut() {
                        partial.stream = info;
                    } else {
                        self.partial_analysis = Some(TimelineAnalysis {
                            stream: info,
                            config: music_lab_analysis_config(),
                            ..TimelineAnalysis::default()
                        });
                    }
                    self.loading_track = Some(LoadingTrack { path, info });
                    self.load_status = if self.player.is_some() {
                        "Streaming playback; decoding timeline samples..."
                    } else {
                        "Decoding timeline samples; playback starts after decode..."
                    }
                    .to_string();
                    ctx.request_repaint();
                }
                Ok(LoadMsg::Status(status)) => {
                    self.load_status = status;
                    ctx.request_repaint();
                }
                Ok(LoadMsg::PartialTimeline {
                    bins,
                    decoded_duration_secs,
                }) => {
                    if !bins.is_empty() {
                        let start_secs = bins
                            .first()
                            .map(|bin| bin.start_secs)
                            .unwrap_or(decoded_duration_secs);
                        let end_secs = bins
                            .last()
                            .map(|bin| bin.start_secs + bin.duration_secs)
                            .unwrap_or(decoded_duration_secs);
                        let partial =
                            self.partial_analysis
                                .get_or_insert_with(|| TimelineAnalysis {
                                    stream: AudioStreamInfo {
                                        duration_secs: decoded_duration_secs,
                                        ..AudioStreamInfo::default()
                                    },
                                    config: music_lab_analysis_config(),
                                    ..TimelineAnalysis::default()
                                });
                        partial.stream.duration_secs =
                            partial.stream.duration_secs.max(decoded_duration_secs);
                        partial.bins.extend(bins);
                        self.timeline_cache.invalidate_time_range(
                            start_secs,
                            end_secs,
                            self.timeline_row_secs(),
                        );
                        ctx.request_repaint();
                    }
                }
                Ok(LoadMsg::Loaded(track)) => {
                    self.load_status = "Loaded".to_string();
                    let track = *track;
                    self.loading_track = None;
                    self.partial_analysis = None;
                    self.timeline_cache.invalidate_all_rows();
                    if self.player.is_none() {
                        match LabPlayer::new(Arc::clone(&track.decoded), true) {
                            Ok(player) => self.player = Some(player),
                            Err(err) => {
                                self.load_status = format!("Loaded; playback disabled: {err}")
                            }
                        }
                    }
                    self.start_spectrum_worker(Arc::clone(&track.decoded), ctx);
                    self.track = Some(track);
                    clear_loader = true;
                    ctx.request_repaint();
                    break;
                }
                Ok(LoadMsg::Failed(err)) => {
                    self.load_status = err;
                    self.loading_track = None;
                    self.partial_analysis = None;
                    clear_loader = true;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.load_status = "Loader stopped".to_string();
                    self.loading_track = None;
                    self.partial_analysis = None;
                    clear_loader = true;
                    break;
                }
            }
        }
        if clear_loader {
            self.load_rx = None;
            self.load_cancel = None;
        }
    }

    fn poll_spectrum_analyzer(&mut self, ctx: &egui::Context) {
        let mut spectrum_disconnected = false;
        if let Some(rx) = self.spectrum_rx.as_ref() {
            loop {
                match rx.try_recv() {
                    Ok(msg) => {
                        self.frame_stats.record_spectrum_compute(msg.compute_ms);
                        self.spectrum_bands = msg.analysis.bands;
                        self.spectrum_notes = msg.analysis.notes;
                        self.spectrum_pending = false;
                        ctx.request_repaint();
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        spectrum_disconnected = true;
                        break;
                    }
                }
            }
        }
        if spectrum_disconnected {
            self.spectrum_pending = false;
            self.spectrum_tx = None;
            self.spectrum_rx = None;
        }

        if self.spectrum_pending {
            return;
        }
        let should_request = self.spectrum_bands.is_empty()
            || match self.last_spectrum_request {
                Some(last) => last.elapsed() >= SPECTRUM_REFRESH_INTERVAL,
                None => true,
            };
        if !should_request {
            return;
        }
        let Some(tx) = self.spectrum_tx.as_ref() else {
            return;
        };
        let position_secs = self.player_snapshot().position_secs;
        if tx.send(SpectrumRequest { position_secs }).is_ok() {
            self.spectrum_pending = true;
            self.last_spectrum_request = Some(Instant::now());
            ctx.request_repaint_after(SPECTRUM_REFRESH_INTERVAL);
        } else {
            self.spectrum_pending = false;
            self.spectrum_tx = None;
            self.spectrum_rx = None;
        }
    }

    fn start_spectrum_worker(&mut self, decoded: Arc<DecodedAudio>, ctx: &egui::Context) {
        let (request_tx, request_rx) = mpsc::channel::<SpectrumRequest>();
        let (result_tx, result_rx) = mpsc::channel::<SpectrumMsg>();
        let repaint_ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("music-lab-spectrum".into())
            .spawn(move || {
                let mut analyzer = SpectrumAnalyzer::new(SPECTRUM_BANDS);
                while let Ok(mut request) = request_rx.recv() {
                    while let Ok(next) = request_rx.try_recv() {
                        request = next;
                    }
                    let compute_start = Instant::now();
                    let analysis = analyzer.analyze(
                        &decoded.stereo_samples,
                        decoded.info.sample_rate,
                        request.position_secs,
                    );
                    let compute_ms = compute_start.elapsed().as_secs_f32() * 1000.0;
                    if result_tx
                        .send(SpectrumMsg {
                            analysis,
                            compute_ms,
                        })
                        .is_err()
                    {
                        break;
                    }
                    repaint_ctx.request_repaint();
                }
            });
        if spawned.is_ok() {
            self.spectrum_tx = Some(request_tx);
            self.spectrum_rx = Some(result_rx);
            self.spectrum_pending = false;
            self.last_spectrum_request = None;
        }
    }

    fn player_snapshot(&self) -> PlaybackSnapshot {
        self.player
            .as_ref()
            .map(LabPlayer::snapshot)
            .unwrap_or_default()
    }

    fn display_duration_secs(&self) -> f64 {
        self.player
            .as_ref()
            .map(|player| player.snapshot().duration_secs)
            .filter(|duration| duration.is_finite() && *duration > 0.0)
            .or_else(|| {
                self.track
                    .as_ref()
                    .map(|track| track.decoded.info.duration_secs)
            })
            .or_else(|| {
                self.loading_track
                    .as_ref()
                    .map(|loading| loading.info.duration_secs)
            })
            .unwrap_or(0.0)
    }

    fn timeline_row_secs(&self) -> f64 {
        if self.timeline_row_secs > 0.0 {
            self.timeline_row_secs
        } else {
            30.0
        }
    }
}

fn perf_log_path() -> PathBuf {
    std::env::temp_dir().join("miv_music_lab_perf.log")
}

fn draw_empty_state(ui: &mut egui::Ui, status: &str) {
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.heading("Music lab");
            ui.label("Open or drop an audio/video file to test the music timeline.");
            if !status.is_empty() {
                ui.label(status);
            }
        });
    });
}

#[derive(Clone, Copy, Debug, Default)]
struct TimelineDrawStats {
    visible_rows: usize,
    drawn_bins: usize,
    cache_misses: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimelineMetricKind {
    LoudnessBassRoot,
    Key,
}

const TIMELINE_METRIC_KINDS: [TimelineMetricKind; TIMELINE_METRIC_LANE_COUNT] = [
    TimelineMetricKind::LoudnessBassRoot,
    TimelineMetricKind::Key,
];

fn draw_timeline(
    ui: &mut egui::Ui,
    analysis: &TimelineAnalysis,
    duration_secs: f64,
    player: Option<&LabPlayer>,
    snap: PlaybackSnapshot,
    cache: &mut TimelineTextureCache,
    row_secs: f64,
    follow_playhead: &mut bool,
) -> TimelineDrawStats {
    let mut stats = TimelineDrawStats::default();
    let row_secs = row_secs.max(1.0);
    let rows = timeline_row_count(duration_secs, row_secs);
    let row_gap = TIMELINE_ROW_GAP;
    let row_h = TIMELINE_WAVEFORM_H + TIMELINE_INNER_GAP + TIMELINE_METRICS_H;
    let content_h = 16.0 + rows as f32 * row_h + rows.saturating_sub(1) as f32 * row_gap;
    let available = egui::vec2(ui.available_width(), ui.available_height().max(content_h));
    let (rect, response) = ui.allocate_exact_size(available, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, app_bg());

    let label_w = 56.0;
    let graph_rect = egui::Rect::from_min_max(
        rect.min + egui::vec2(label_w, 8.0),
        egui::pos2(rect.max.x - 8.0, rect.min.y + content_h - 8.0),
    );
    let ppp = ui.ctx().pixels_per_point().clamp(1.0, 3.0);
    let width_px =
        ((graph_rect.width() * ppp).round() as usize).clamp(1, TIMELINE_TEXTURE_MAX_WIDTH);
    let waveform_h_px = ((TIMELINE_WAVEFORM_H * ppp).round() as usize).max(1);
    let gap_px = ((TIMELINE_INNER_GAP * ppp).round() as usize).max(1);
    let metrics_h_px = ((TIMELINE_METRICS_H * ppp).round() as usize).max(1);
    let texture_key = TimelineTextureCacheKey {
        width_px,
        waveform_h_px,
        gap_px,
        metrics_h_px,
        row_secs_millis: (row_secs * 1000.0).round() as u32,
        rows,
        dark: true,
    };
    cache.ensure(texture_key);
    let (uploaded_rows, uploaded_bins) =
        cache.poll_finished_rows(ui.ctx(), TIMELINE_ROW_TEXTURE_UPLOAD_BUDGET_PER_FRAME);
    stats.cache_misses += uploaded_rows;
    stats.drawn_bins += uploaded_bins;

    let clip_rect = ui.clip_rect();
    if timeline_manual_scroll_requested(ui, &response, clip_rect) {
        *follow_playhead = false;
    }
    if let Some(playhead_rect) = timeline_playhead_row_rect(
        graph_rect,
        snap.position_secs,
        row_secs,
        row_h,
        row_gap,
        rows,
    ) {
        let vertically_visible = clip_rect.intersects(playhead_rect);
        let fully_visible = clip_rect_vertically_contains(clip_rect, playhead_rect);
        if vertically_visible {
            *follow_playhead = true;
        }
        if snap.playing && *follow_playhead && !fully_visible {
            ui.scroll_to_rect(playhead_rect.expand(row_gap), None);
        }
    }

    let mut pending_raster = false;
    for row in 0..rows {
        let row_top = graph_rect.min.y + row as f32 * (row_h + row_gap);
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(graph_rect.min.x, row_top),
            egui::vec2(graph_rect.width(), row_h),
        );
        if !clip_rect.intersects(row_rect.expand(row_gap)) {
            continue;
        }
        let row_start = row as f64 * row_secs;
        stats.visible_rows += 1;
        let fresh_before = cache.row_is_fresh(row, texture_key);
        if !fresh_before {
            pending_raster = true;
        }
        let (row_texture, request_sent) =
            cache.row_texture(&analysis.bins, row, row_secs, texture_key);
        if request_sent {
            pending_raster = true;
        }
        if let Some((texture, _represented_bins)) = row_texture {
            painter.image(
                texture.id(),
                row_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        painter.text(
            egui::pos2(rect.min.x + 8.0, row_rect.center().y),
            egui::Align2::LEFT_CENTER,
            format_time(row_start),
            egui::FontId::monospace(12.0),
            ui.visuals().text_color(),
        );
        draw_beat_grid(&painter, row_rect, row_start, row_secs, analysis);
    }
    if pending_raster {
        ui.ctx().request_repaint_after(Duration::from_millis(1));
    }

    draw_playhead(
        &painter,
        graph_rect,
        snap.position_secs,
        row_secs,
        row_h,
        row_gap,
    );

    if let Some(pos) = response.hover_pos() {
        draw_timeline_metric_hover(
            &painter,
            pos,
            graph_rect,
            row_secs,
            row_h,
            row_gap,
            rows,
            &analysis.bins,
        );
    }

    if (response.clicked() || response.dragged())
        && let Some(pos) = response.interact_pointer_pos()
    {
        let local_y = pos.y - graph_rect.min.y;
        let row = (local_y / (row_h + row_gap)).floor().max(0.0) as usize;
        let row_top = graph_rect.min.y + row as f32 * (row_h + row_gap);
        if pos.y >= row_top && pos.y <= row_top + row_h {
            let frac = ((pos.x - graph_rect.min.x) / graph_rect.width()).clamp(0.0, 1.0);
            let seek = row as f64 * row_secs + frac as f64 * row_secs;
            if let Some(player) = player {
                player.seek_secs(seek);
            }
        }
    }
    stats
}

fn draw_pending_timeline(
    ui: &mut egui::Ui,
    loading: &LoadingTrack,
    row_secs: f64,
    status: &str,
) -> TimelineDrawStats {
    let mut stats = TimelineDrawStats::default();
    let row_secs = row_secs.max(1.0);
    let rows = timeline_row_count(loading.info.duration_secs, row_secs);
    let row_gap = TIMELINE_ROW_GAP;
    let row_h = TIMELINE_WAVEFORM_H + TIMELINE_INNER_GAP + TIMELINE_METRICS_H;
    let content_h = 16.0 + rows as f32 * row_h + rows.saturating_sub(1) as f32 * row_gap;
    let available = egui::vec2(ui.available_width(), ui.available_height().max(content_h));
    let (rect, _response) = ui.allocate_exact_size(available, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, app_bg());

    let label_w = 56.0;
    let graph_rect = egui::Rect::from_min_max(
        rect.min + egui::vec2(label_w, 8.0),
        egui::pos2(rect.max.x - 8.0, rect.min.y + content_h - 8.0),
    );
    let clip_rect = ui.clip_rect();
    for row in 0..rows {
        let row_top = graph_rect.min.y + row as f32 * (row_h + row_gap);
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(graph_rect.min.x, row_top),
            egui::vec2(graph_rect.width(), row_h),
        );
        if !clip_rect.intersects(row_rect.expand(row_gap)) {
            continue;
        }
        stats.visible_rows += 1;
        let waveform_rect = egui::Rect::from_min_size(
            row_rect.min,
            egui::vec2(row_rect.width(), TIMELINE_WAVEFORM_H),
        );
        let gap_rect = egui::Rect::from_min_size(
            egui::pos2(row_rect.left(), waveform_rect.bottom()),
            egui::vec2(row_rect.width(), TIMELINE_INNER_GAP),
        );
        let metrics_rect =
            egui::Rect::from_min_max(egui::pos2(row_rect.left(), gap_rect.bottom()), row_rect.max);
        painter.rect_filled(waveform_rect, 0.0, egui::Color32::BLACK);
        painter.rect_filled(
            gap_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(36, 48, 60, 120),
        );
        painter.rect_filled(metrics_rect, 0.0, egui::Color32::from_rgb(3, 5, 7));
        painter.rect_stroke(
            row_rect,
            0.0,
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(92, 112, 134, 112),
            ),
            egui::StrokeKind::Inside,
        );
        painter.rect_stroke(
            waveform_rect,
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(74, 96, 120, 118)),
            egui::StrokeKind::Inside,
        );
        painter.rect_stroke(
            metrics_rect,
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(74, 96, 120, 88)),
            egui::StrokeKind::Inside,
        );
        painter.text(
            egui::pos2(rect.min.x + 8.0, row_rect.center().y),
            egui::Align2::LEFT_CENTER,
            format_time(row as f64 * row_secs),
            egui::FontId::monospace(12.0),
            ui.visuals().text_color(),
        );
    }

    let message = if loading.info.duration_secs > 0.0 {
        format!(
            "{}  {} rows reserved",
            status,
            timeline_row_count(loading.info.duration_secs, row_secs)
        )
    } else {
        status.to_string()
    };
    painter.text(
        graph_rect.min + egui::vec2(14.0, 16.0),
        egui::Align2::LEFT_TOP,
        message,
        egui::FontId::proportional(14.0),
        egui::Color32::from_rgb(204, 214, 224),
    );
    stats
}

fn timeline_row_count(duration_secs: f64, row_secs: f64) -> usize {
    if !duration_secs.is_finite()
        || duration_secs <= 0.0
        || !row_secs.is_finite()
        || row_secs <= 0.0
    {
        1
    } else {
        (duration_secs / row_secs).ceil().max(1.0) as usize
    }
}

fn clip_rect_vertically_contains(clip_rect: egui::Rect, target: egui::Rect) -> bool {
    target.top() >= clip_rect.top() && target.bottom() <= clip_rect.bottom()
}

fn timeline_manual_scroll_requested(
    ui: &egui::Ui,
    response: &egui::Response,
    clip_rect: egui::Rect,
) -> bool {
    let pointer_over_timeline = ui
        .input(|input| input.pointer.hover_pos())
        .is_some_and(|pos| clip_rect.contains(pos) || response.rect.contains(pos));
    if !pointer_over_timeline {
        return false;
    }
    ui.input(|input| {
        input.raw_scroll_delta.y.abs() > 0.0 || input.smooth_scroll_delta.y.abs() > 0.5
    })
}

fn timeline_playhead_row_rect(
    graph_rect: egui::Rect,
    position_secs: f64,
    row_secs: f64,
    row_h: f32,
    row_gap: f32,
    rows: usize,
) -> Option<egui::Rect> {
    if row_secs <= 0.0 || rows == 0 || !position_secs.is_finite() {
        return None;
    }
    let row = (position_secs.max(0.0) / row_secs)
        .floor()
        .max(0.0)
        .min(rows.saturating_sub(1) as f64) as usize;
    let row_top = graph_rect.min.y + row as f32 * (row_h + row_gap);
    Some(egui::Rect::from_min_size(
        egui::pos2(graph_rect.min.x, row_top),
        egui::vec2(graph_rect.width(), row_h),
    ))
}

fn draw_timeline_metric_hover(
    painter: &egui::Painter,
    pos: egui::Pos2,
    graph_rect: egui::Rect,
    row_secs: f64,
    row_h: f32,
    row_gap: f32,
    rows: usize,
    bins: &[WaveformBin],
) {
    if row_secs <= 0.0 || rows == 0 || bins.is_empty() || !graph_rect.contains(pos) {
        return;
    }
    let row_stride = row_h + row_gap;
    let local_y = pos.y - graph_rect.top();
    if local_y < 0.0 {
        return;
    }
    let row = (local_y / row_stride).floor() as usize;
    if row >= rows {
        return;
    }
    let row_top = graph_rect.top() + row as f32 * row_stride;
    let metrics_rect = egui::Rect::from_min_max(
        egui::pos2(
            graph_rect.left(),
            row_top + TIMELINE_WAVEFORM_H + TIMELINE_INNER_GAP,
        ),
        egui::pos2(graph_rect.right(), row_top + row_h),
    );
    if !metrics_rect.contains(pos) {
        return;
    }

    let loudness_bottom =
        metrics_rect.top() + metrics_rect.height() * TIMELINE_LOUDNESS_ROOT_LANE_FRACTION;
    let lane_idx = if pos.y < loudness_bottom { 0 } else { 1 };
    let kind = TIMELINE_METRIC_KINDS[lane_idx];
    let row_start = row as f64 * row_secs;
    let frac = ((pos.x - graph_rect.left()) / graph_rect.width()).clamp(0.0, 1.0);
    let time_secs = row_start + frac as f64 * row_secs;
    let Some(bin) = timeline_bin_at_time(bins, time_secs) else {
        return;
    };
    let value = timeline_metric_hover_value(kind, bins, bin, time_secs).clamp(0.0, 1.0);
    let x = graph_rect.left() + frac * graph_rect.width();
    let (lane_top, lane_bottom) =
        timeline_metric_lane_bounds_f32(metrics_rect.top(), metrics_rect.height(), lane_idx);
    let lane_rect = egui::Rect::from_min_max(
        egui::pos2(metrics_rect.left(), lane_top),
        egui::pos2(metrics_rect.right(), lane_bottom),
    );
    painter.rect_stroke(
        lane_rect,
        0.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(210, 230, 250, 150),
        ),
        egui::StrokeKind::Inside,
    );
    painter.line_segment(
        [
            egui::pos2(x, lane_rect.top()),
            egui::pos2(x, lane_rect.bottom()),
        ],
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(230, 240, 255, 150),
        ),
    );

    let label = format!(
        "{}{} {:.0}%  {}  {}",
        timeline_metric_name(kind),
        timeline_metric_extra(kind, bins, bin, time_secs),
        value * 100.0,
        format_time(time_secs),
        timeline_metric_description(kind)
    );
    let label_w = (label.chars().count() as f32 * 7.0 + 14.0).min(graph_rect.width() - 8.0);
    let label_h = 22.0;
    let label_x = if x + 10.0 + label_w <= graph_rect.right() {
        x + 10.0
    } else {
        (x - 10.0 - label_w).max(graph_rect.left() + 4.0)
    };
    let label_y = if lane_rect.top() - label_h - 6.0 >= graph_rect.top() {
        lane_rect.top() - label_h - 6.0
    } else {
        lane_rect.bottom() + 6.0
    };
    let label_rect =
        egui::Rect::from_min_size(egui::pos2(label_x, label_y), egui::vec2(label_w, label_h));
    painter.rect_filled(
        label_rect,
        3.0,
        egui::Color32::from_rgba_unmultiplied(5, 8, 12, 230),
    );
    painter.rect_stroke(
        label_rect,
        3.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(100, 150, 190, 180),
        ),
        egui::StrokeKind::Inside,
    );
    painter.text(
        label_rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(11.0),
        egui::Color32::from_rgb(234, 244, 252),
    );
}

fn timeline_bin_at_time(bins: &[WaveformBin], time_secs: f64) -> Option<&WaveformBin> {
    if bins.is_empty() || !time_secs.is_finite() {
        return None;
    }
    let idx = bins.partition_point(|bin| bin.start_secs + bin.duration_secs <= time_secs);
    bins.get(idx).or_else(|| bins.last())
}

fn music_lab_analysis_config() -> AnalysisConfig {
    AnalysisConfig {
        bin_secs: 0.010,
        ..AnalysisConfig::default()
    }
}

fn partial_timeline_analysis_config() -> AnalysisConfig {
    music_lab_analysis_config()
}

fn timeline_bins_for_raster(
    bins: &[WaveformBin],
    row_start: f64,
    row_secs: f64,
) -> Vec<WaveformBin> {
    if bins.is_empty() {
        return Vec::new();
    }
    let row_end = row_start + row_secs;
    let pad = TIMELINE_KEY_WINDOW_SECS * 0.5;
    let copy_start = (row_start - pad).max(0.0);
    let copy_end = row_end + pad;
    let start_idx = bins.partition_point(|bin| bin.start_secs + bin.duration_secs < copy_start);
    let end_idx = bins.partition_point(|bin| bin.start_secs <= copy_end);
    bins[start_idx..end_idx].to_vec()
}

fn render_timeline_row_image(
    row_start: f64,
    row_secs: f64,
    bins: &[WaveformBin],
    width: usize,
    waveform_h: usize,
    gap_h: usize,
    metrics_h: usize,
    _dark: bool,
) -> (egui::ColorImage, usize) {
    let height = waveform_h + gap_h + metrics_h;
    let bg = app_bg();
    let mut pixels = vec![bg; width * height];

    fill_rect_px(
        &mut pixels,
        width,
        height,
        0,
        0,
        width,
        waveform_h,
        egui::Color32::BLACK,
    );
    let metrics_top = waveform_h + gap_h;
    fill_rect_px(
        &mut pixels,
        width,
        height,
        0,
        metrics_top,
        width,
        height,
        egui::Color32::from_rgb(3, 5, 7),
    );
    let center_y = waveform_h as f32 * 0.5;
    fill_rect_f32(
        &mut pixels,
        width,
        height,
        0.0,
        center_y - 0.5,
        width as f32,
        center_y + 0.5,
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 28),
    );

    let mut drawn_bins = 0;
    let mut visible_bins = Vec::new();
    for (index, bin) in bins.iter().enumerate() {
        let bin_end = bin.start_secs + bin.duration_secs;
        if bin_end < row_start || bin.start_secs > row_start + row_secs {
            continue;
        }
        let visible_start = bin.start_secs.max(row_start);
        let visible_end = bin_end.min(row_start + row_secs);
        let x0 = ((visible_start - row_start) / row_secs) as f32 * width as f32;
        let x1 = (((visible_end - row_start) / row_secs) as f32 * width as f32)
            .max(x0 + 1.0)
            .min(width as f32);
        visible_bins.push(TimelineVisibleBin { index, x0, x1 });
    }

    let rhythm_segments = build_timeline_rhythm_segments(bins, &visible_bins);
    let bass_hints = build_bass_root_display_hints(bins, &visible_bins, &rhythm_segments);
    let key_hints = build_key_display_hints(bins, &visible_bins, &rhythm_segments);

    for (visible_idx, visible) in visible_bins.iter().copied().enumerate() {
        let bin = &bins[visible.index];
        drawn_bins += 1;
        let amp = (bin.peak.max(bin.rms * 2.0)).sqrt().clamp(0.025, 1.0);
        let outer_half_h = (waveform_h as f32 * 0.46 * amp).max(1.0);
        let core_scale = 0.42 + bin.rms.sqrt().clamp(0.0, 1.0) * 0.45;
        let core_half_h = (outer_half_h * core_scale).max(1.0).min(outer_half_h);
        draw_spectral_waveform_bin_pixels(
            &mut pixels,
            width,
            height,
            center_y,
            visible.x0,
            visible.x1,
            outer_half_h,
            core_half_h,
            bin.band_energy,
        );
        if bin.transient > TRANSIENT_ACCENT_MIN {
            let transient = ((bin.transient - TRANSIENT_ACCENT_MIN) / (1.0 - TRANSIENT_ACCENT_MIN))
                .sqrt()
                .clamp(0.0, 1.0);
            let accent_half_h = waveform_h as f32 * (0.12 + transient * 0.22);
            let accent_center = ((visible.x0 + visible.x1) * 0.5).clamp(0.0, width as f32);
            let accent_half_w =
                ((visible.x1 - visible.x0).max(1.5) * (0.30 + transient * 0.45)).min(6.0);
            let accent = transient_color(bin.transient_band, transient);
            fill_rect_f32(
                &mut pixels,
                width,
                height,
                accent_center - accent_half_w,
                center_y - accent_half_h,
                accent_center + accent_half_w,
                center_y + accent_half_h,
                color_with_alpha(accent, (34.0 + transient * 76.0) as u8),
            );
            fill_rect_f32(
                &mut pixels,
                width,
                height,
                accent_center - 0.5,
                center_y - accent_half_h,
                accent_center + 0.5,
                center_y + accent_half_h,
                color_with_alpha(
                    brighten_color(accent, 1.18),
                    (54.0 + transient * 96.0) as u8,
                ),
            );
        }

        draw_timeline_metric_bins(
            &mut pixels,
            width,
            height,
            metrics_top,
            metrics_h,
            visible.x0,
            visible.x1,
            bin,
            bass_hints[visible_idx],
            key_hints[visible_idx],
        );
    }

    fill_rect_px(
        &mut pixels,
        width,
        height,
        0,
        waveform_h,
        width,
        (waveform_h + gap_h).min(height),
        egui::Color32::from_rgba_unmultiplied(36, 48, 60, 120),
    );
    draw_rect_stroke_px(
        &mut pixels,
        width,
        height,
        0,
        0,
        width,
        height,
        egui::Color32::from_rgba_unmultiplied(92, 112, 134, 112),
    );
    draw_rect_stroke_px(
        &mut pixels,
        width,
        height,
        0,
        0,
        width,
        waveform_h,
        egui::Color32::from_rgba_unmultiplied(74, 96, 120, 118),
    );
    draw_rect_stroke_px(
        &mut pixels,
        width,
        height,
        0,
        metrics_top,
        width,
        height,
        egui::Color32::from_rgba_unmultiplied(74, 96, 120, 88),
    );

    (egui::ColorImage::new([width, height], pixels), drawn_bins)
}

#[derive(Clone, Copy, Debug)]
struct TimelineVisibleBin {
    index: usize,
    x0: f32,
    x1: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct TimelinePitchHint {
    pitch_class: u8,
    confidence: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimelineKeyMode {
    Major,
    Minor,
}

#[derive(Clone, Copy, Debug)]
struct TimelineKeyHint {
    pitch_class: u8,
    mode: TimelineKeyMode,
    confidence: f32,
}

impl Default for TimelineKeyHint {
    fn default() -> Self {
        Self {
            pitch_class: 0,
            mode: TimelineKeyMode::Major,
            confidence: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct TimelineRhythmSegment {
    start: usize,
    end: usize,
}

fn build_timeline_rhythm_segments(
    bins: &[WaveformBin],
    visible_bins: &[TimelineVisibleBin],
) -> Vec<TimelineRhythmSegment> {
    let mut segments = Vec::new();
    if visible_bins.is_empty() {
        return segments;
    }

    let bin_secs = bins[visible_bins[0].index].duration_secs.max(0.005);
    let min_bins = (TIMELINE_ROOT_MIN_SEGMENT_SECS / bin_secs).ceil().max(1.0) as usize;
    let max_bins = (TIMELINE_ROOT_MAX_SEGMENT_SECS / bin_secs)
        .ceil()
        .max(min_bins as f64) as usize;

    let mut start = 0usize;
    while start < visible_bins.len() {
        let min_end = (start + min_bins).min(visible_bins.len());
        let max_end = (start + max_bins).min(visible_bins.len());
        let mut end = max_end.max(start + 1).min(visible_bins.len());

        if min_end < max_end {
            let mut best_idx = None;
            let mut best_score = TIMELINE_ROOT_TRANSIENT_THRESHOLD;
            for idx in min_end..max_end {
                let bin = &bins[visible_bins[idx].index];
                let score =
                    bin.transient * (0.55 + normalize_range(bin.band_energy[0], 0.05, 0.45) * 0.45);
                if score > best_score {
                    best_score = score;
                    best_idx = Some(idx);
                }
            }
            if let Some(idx) = best_idx {
                end = (idx + 1).max(start + 1);
            }
        }

        segments.push(TimelineRhythmSegment { start, end });
        start = end;
    }

    segments
}

fn build_bass_root_display_hints(
    bins: &[WaveformBin],
    visible_bins: &[TimelineVisibleBin],
    rhythm_segments: &[TimelineRhythmSegment],
) -> Vec<TimelinePitchHint> {
    let mut hints = vec![TimelinePitchHint::default(); visible_bins.len()];
    for segment in rhythm_segments {
        let hint = vote_bass_root_hint(bins, &visible_bins[segment.start..segment.end]);
        for slot in &mut hints[segment.start..segment.end] {
            *slot = hint;
        }
    }

    hints
}

fn build_key_display_hints(
    bins: &[WaveformBin],
    visible_bins: &[TimelineVisibleBin],
    rhythm_segments: &[TimelineRhythmSegment],
) -> Vec<TimelineKeyHint> {
    let mut hints = vec![TimelineKeyHint::default(); visible_bins.len()];
    for segment in rhythm_segments {
        if segment.start >= segment.end {
            continue;
        }
        let center_visible = &visible_bins[(segment.start + segment.end - 1) / 2];
        let center_bin = &bins[center_visible.index];
        let center_secs = center_bin.start_secs + center_bin.duration_secs * 0.5;
        let window_start = center_secs - TIMELINE_KEY_WINDOW_SECS * 0.5;
        let window_end = center_secs + TIMELINE_KEY_WINDOW_SECS * 0.5;
        let start_idx =
            bins.partition_point(|bin| bin.start_secs + bin.duration_secs <= window_start);
        let end_idx = bins.partition_point(|bin| bin.start_secs < window_end);
        let hint = vote_key_hint(&bins[start_idx..end_idx]);
        for slot in &mut hints[segment.start..segment.end] {
            *slot = hint;
        }
    }

    hints
}

fn vote_bass_root_hint(
    bins: &[WaveformBin],
    visible_bins: &[TimelineVisibleBin],
) -> TimelinePitchHint {
    let mut votes = [0.0_f32; 12];
    let mut count = 0usize;
    for visible in visible_bins {
        let bin = &bins[visible.index];
        let weight = timeline_metric_value(TimelineMetricKind::LoudnessBassRoot, bin).powi(2);
        votes[bin.bass_pitch_class as usize % 12] += weight;
        count += 1;
    }
    best_pitch_hint(votes, count)
}

fn vote_key_hint(bins: &[WaveformBin]) -> TimelineKeyHint {
    let mut chroma = [0.0_f32; 12];
    let mut presence = [0.0_f32; 12];
    let mut total_weight = 0.0_f32;
    for bin in bins {
        let weight = key_chroma_bin_weight(bin);
        if weight <= 1.0e-5 {
            continue;
        }
        let bin_max = bin.chroma.iter().copied().fold(0.0_f32, f32::max);
        for (pc, value) in bin.chroma.iter().copied().enumerate() {
            let value = value.max(0.0);
            chroma[pc] += value.powf(1.08) * weight;
            if bin_max > 1.0e-6 {
                presence[pc] += smoothstep(bin_max * 0.36, bin_max * 0.70, value) * weight.sqrt();
            }
        }
        total_weight += weight;
    }
    profile_key_hint(chroma, presence, total_weight)
}

fn key_hint_at_time(bins: &[WaveformBin], time_secs: f64) -> TimelineKeyHint {
    if bins.is_empty() || !time_secs.is_finite() {
        return TimelineKeyHint::default();
    }
    let window_start = time_secs - TIMELINE_KEY_WINDOW_SECS * 0.5;
    let window_end = time_secs + TIMELINE_KEY_WINDOW_SECS * 0.5;
    let start_idx = bins.partition_point(|bin| bin.start_secs + bin.duration_secs <= window_start);
    let end_idx = bins.partition_point(|bin| bin.start_secs < window_end);
    vote_key_hint(&bins[start_idx..end_idx])
}

fn key_chroma_bin_weight(bin: &WaveformBin) -> f32 {
    // Lightweight HPSS-style gate: keep sustained harmonic chroma and discount percussive bins.
    let loudness = timeline_loudness_value(bin).powf(1.18);
    let transient_keep = 1.0
        - TIMELINE_KEY_TRANSIENT_REDUCTION
            * smoothstep(
                TIMELINE_KEY_TRANSIENT_PENALTY_START,
                TIMELINE_KEY_TRANSIENT_PENALTY_END,
                bin.transient,
            );
    let density_keep = 1.0
        - TIMELINE_KEY_DENSITY_REDUCTION
            * smoothstep(
                TIMELINE_KEY_DENSITY_PENALTY_START,
                TIMELINE_KEY_DENSITY_PENALTY_END,
                bin.transient_density,
            );
    let confidence = 0.36 + bin.key_confidence.clamp(0.0, 1.0) * 0.64;
    (loudness * transient_keep * density_keep * confidence).clamp(0.0, 1.0)
}

fn profile_key_hint(chroma: [f32; 12], presence: [f32; 12], total_weight: f32) -> TimelineKeyHint {
    if total_weight <= 1.0e-6 || chroma.iter().copied().sum::<f32>() <= 1.0e-6 {
        return TimelineKeyHint::default();
    }

    let mut best = TimelineKeyCandidate::default();
    let mut second = TimelineKeyCandidate::default();
    for root in 0..12 {
        for mode in [TimelineKeyMode::Major, TimelineKeyMode::Minor] {
            let score = blended_key_profile_score(&chroma, &presence, root, mode);
            let candidate = TimelineKeyCandidate {
                pitch_class: root as u8,
                mode,
                score,
            };
            if candidate.score > best.score {
                second = best;
                best = candidate;
            } else if candidate.score > second.score {
                second = candidate;
            }
        }
    }

    let energy =
        (chroma.iter().copied().sum::<f32>() / total_weight.max(1.0e-6)).clamp(0.0, 4.0) / 4.0;
    let margin = (best.score - second.score).max(0.0);
    let score_support = smoothstep(-0.10, 0.32, best.score);
    let margin_support = smoothstep(0.0, 0.16, margin);
    let confidence =
        (score_support * (0.22 + margin_support * 0.78) * (0.45 + 0.55 * energy.sqrt()))
            .clamp(0.0, 1.0);

    TimelineKeyHint {
        pitch_class: best.pitch_class,
        mode: best.mode,
        confidence,
    }
}

fn blended_key_profile_score(
    chroma: &[f32; 12],
    presence: &[f32; 12],
    root: usize,
    mode: TimelineKeyMode,
) -> f32 {
    let (krumhansl, temperley) = match mode {
        TimelineKeyMode::Major => (
            TIMELINE_KEY_KRUMHANSL_MAJOR_PROFILE,
            TIMELINE_KEY_TEMPERLEY_MAJOR_PROFILE,
        ),
        TimelineKeyMode::Minor => (
            TIMELINE_KEY_KRUMHANSL_MINOR_PROFILE,
            TIMELINE_KEY_TEMPERLEY_MINOR_PROFILE,
        ),
    };
    let chroma_score = profile_correlation(chroma, &krumhansl, root);
    let presence_score = profile_correlation(presence, &temperley, root);
    chroma_score * TIMELINE_KEY_KRUMHANSL_WEIGHT + presence_score * TIMELINE_KEY_TEMPERLEY_WEIGHT
}

#[derive(Clone, Copy, Debug)]
struct TimelineKeyCandidate {
    pitch_class: u8,
    mode: TimelineKeyMode,
    score: f32,
}

impl Default for TimelineKeyCandidate {
    fn default() -> Self {
        Self {
            pitch_class: 0,
            mode: TimelineKeyMode::Major,
            score: f32::NEG_INFINITY,
        }
    }
}

fn profile_correlation(chroma: &[f32; 12], profile: &[f32; 12], root: usize) -> f32 {
    let chroma_mean = chroma.iter().copied().sum::<f32>() / 12.0;
    let profile_mean = profile.iter().copied().sum::<f32>() / 12.0;
    let mut numerator = 0.0_f32;
    let mut chroma_norm = 0.0_f32;
    let mut profile_norm = 0.0_f32;
    for pc in 0..12 {
        let chroma_value = chroma[pc] - chroma_mean;
        let degree = (pc + 12 - root) % 12;
        let profile_value = profile[degree] - profile_mean;
        numerator += chroma_value * profile_value;
        chroma_norm += chroma_value * chroma_value;
        profile_norm += profile_value * profile_value;
    }
    if chroma_norm <= 1.0e-8 || profile_norm <= 1.0e-8 {
        0.0
    } else {
        numerator / (chroma_norm.sqrt() * profile_norm.sqrt())
    }
}

fn best_pitch_hint(votes: [f32; 12], count: usize) -> TimelinePitchHint {
    let mut best_idx = 0usize;
    let mut best = 0.0_f32;
    let mut sum = 0.0_f32;
    for (idx, value) in votes.iter().copied().enumerate() {
        sum += value;
        if value > best {
            best = value;
            best_idx = idx;
        }
    }
    if sum <= 1.0e-6 || count == 0 {
        return TimelinePitchHint::default();
    }
    let dominance = (best / sum).clamp(0.0, 1.0);
    let support = (best / count as f32).sqrt().clamp(0.0, 1.0);
    TimelinePitchHint {
        pitch_class: best_idx as u8,
        confidence: (support * (0.42 + dominance * 0.58)).clamp(0.0, 1.0),
    }
}

fn draw_timeline_metric_bins(
    pixels: &mut [egui::Color32],
    width: usize,
    height: usize,
    metrics_top: usize,
    metrics_h: usize,
    x0: f32,
    x1: f32,
    bin: &WaveformBin,
    bass_hint: TimelinePitchHint,
    key_hint: TimelineKeyHint,
) {
    if metrics_h == 0 {
        return;
    }
    for (idx, kind) in TIMELINE_METRIC_KINDS.into_iter().enumerate() {
        let (top, bottom) = timeline_metric_lane_bounds(metrics_top, metrics_h, idx);
        let lane_bottom = bottom - 0.6;
        let lane_top = top + 0.6;
        let available_h = (lane_bottom - lane_top).max(1.0);
        let value = timeline_metric_display_value(kind, bin, bass_hint, key_hint).clamp(0.0, 1.0);
        let full_pitch_lane = matches!(kind, TimelineMetricKind::Key) && value > 0.03;
        let bar_top = if full_pitch_lane {
            lane_top
        } else {
            lane_bottom - available_h * value.max(0.025)
        };
        fill_rect_f32(
            pixels,
            width,
            height,
            x0,
            bar_top,
            x1,
            lane_bottom,
            timeline_metric_color(kind, value, bin, bass_hint, key_hint),
        );
        if idx > 0 {
            fill_rect_f32(
                pixels,
                width,
                height,
                0.0,
                top,
                width as f32,
                top + 0.5,
                egui::Color32::from_rgba_unmultiplied(80, 98, 118, 42),
            );
        }
    }
}

fn timeline_metric_lane_bounds(
    metrics_top: usize,
    metrics_h: usize,
    lane_idx: usize,
) -> (f32, f32) {
    timeline_metric_lane_bounds_f32(metrics_top as f32, metrics_h as f32, lane_idx)
}

fn timeline_metric_lane_bounds_f32(
    metrics_top: f32,
    metrics_h: f32,
    lane_idx: usize,
) -> (f32, f32) {
    let top = metrics_top;
    let bottom = metrics_top + metrics_h;
    let loudness_bottom = top + metrics_h * TIMELINE_LOUDNESS_ROOT_LANE_FRACTION;
    if lane_idx == 0 {
        (top, loudness_bottom)
    } else {
        (loudness_bottom, bottom)
    }
}

fn timeline_metric_display_value(
    kind: TimelineMetricKind,
    bin: &WaveformBin,
    bass_hint: TimelinePitchHint,
    key_hint: TimelineKeyHint,
) -> f32 {
    match kind {
        TimelineMetricKind::LoudnessBassRoot => {
            timeline_loudness_value(bin) * (0.72 + 0.28 * bass_hint.confidence.clamp(0.0, 1.0))
        }
        TimelineMetricKind::Key if key_hint.confidence > 0.0 => {
            TIMELINE_KEY_DISPLAY_FLOOR + key_hint.confidence * (1.0 - TIMELINE_KEY_DISPLAY_FLOOR)
        }
        TimelineMetricKind::Key => 0.0,
    }
}

fn timeline_metric_hover_value(
    kind: TimelineMetricKind,
    bins: &[WaveformBin],
    bin: &WaveformBin,
    time_secs: f64,
) -> f32 {
    match kind {
        TimelineMetricKind::LoudnessBassRoot => timeline_loudness_value(bin),
        TimelineMetricKind::Key => key_hint_at_time(bins, time_secs).confidence,
    }
}

fn timeline_metric_value(kind: TimelineMetricKind, bin: &WaveformBin) -> f32 {
    match kind {
        TimelineMetricKind::LoudnessBassRoot => (bin.bass_pitch_confidence
            * normalize_range(bin.band_energy[0], 0.06, 0.48)
            * timeline_loudness_value(bin).powf(0.90))
        .sqrt()
        .clamp(0.0, 1.0),
        TimelineMetricKind::Key => {
            (bin.key_confidence * timeline_loudness_value(bin).powf(0.72)).clamp(0.0, 1.0)
        }
    }
}

fn timeline_loudness_value(bin: &WaveformBin) -> f32 {
    ((bin.loudness_db + 52.0) / 52.0).clamp(0.0, 1.0).powf(0.72)
}

fn normalize_range(value: f32, low: f32, high: f32) -> f32 {
    if high <= low {
        return if value >= high { 1.0 } else { 0.0 };
    }
    ((value - low) / (high - low)).clamp(0.0, 1.0)
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    if (edge1 - edge0).abs() <= f32::EPSILON {
        return if value >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn timeline_metric_name(kind: TimelineMetricKind) -> &'static str {
    match kind {
        TimelineMetricKind::LoudnessBassRoot => "Loudness+Bass",
        TimelineMetricKind::Key => "Key",
    }
}

fn timeline_metric_description(kind: TimelineMetricKind) -> &'static str {
    match kind {
        TimelineMetricKind::LoudnessBassRoot => "Height RMS, color bass root",
        TimelineMetricKind::Key => "Transient-suppressed chroma profile",
    }
}

fn timeline_metric_extra(
    kind: TimelineMetricKind,
    bins: &[WaveformBin],
    bin: &WaveformBin,
    time_secs: f64,
) -> String {
    match kind {
        TimelineMetricKind::LoudnessBassRoot if bin.bass_pitch_confidence > 0.08 => {
            format!(" {}", pitch_class_name(bin.bass_pitch_class))
        }
        TimelineMetricKind::Key => {
            let hint = key_hint_at_time(bins, time_secs);
            if hint.confidence > 0.01 {
                format!(
                    " {} {}",
                    pitch_class_name(hint.pitch_class),
                    key_mode_label(hint.mode)
                )
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

fn timeline_metric_color(
    kind: TimelineMetricKind,
    value: f32,
    _bin: &WaveformBin,
    bass_hint: TimelinePitchHint,
    key_hint: TimelineKeyHint,
) -> egui::Color32 {
    let value = value.clamp(0.0, 1.0);
    let base = match kind {
        TimelineMetricKind::LoudnessBassRoot if bass_hint.confidence > 0.06 => {
            let color_value = (bass_hint.confidence * 0.78).clamp(0.32, 0.74);
            return color_with_alpha(
                brighten_color(
                    key_color(60 + bass_hint.pitch_class % 12, color_value),
                    0.86 + value * 0.16 + bass_hint.confidence * 0.12,
                ),
                (86.0 + value * 70.0 + bass_hint.confidence * 28.0).min(202.0) as u8,
            );
        }
        TimelineMetricKind::LoudnessBassRoot => egui::Color32::from_rgb(188, 198, 92),
        TimelineMetricKind::Key if key_hint.confidence > 0.0 => {
            let color_value = (0.36 + key_hint.confidence * 0.38).min(0.74);
            return color_with_alpha(
                brighten_color(
                    key_color(60 + key_hint.pitch_class % 12, color_value),
                    0.78 + key_hint.confidence * 0.30,
                ),
                (70.0 + key_hint.confidence * 120.0).min(190.0) as u8,
            );
        }
        TimelineMetricKind::Key => egui::Color32::from_rgb(104, 214, 186),
    };
    color_with_alpha(
        brighten_color(base, 0.42 + value * 0.72),
        (58.0 + value * 162.0) as u8,
    )
}

fn key_mode_label(mode: TimelineKeyMode) -> &'static str {
    match mode {
        TimelineKeyMode::Major => "maj",
        TimelineKeyMode::Minor => "min",
    }
}

fn draw_spectral_waveform_bin_pixels(
    pixels: &mut [egui::Color32],
    width: usize,
    height: usize,
    center_y: f32,
    x0: f32,
    x1: f32,
    outer_half_h: f32,
    core_half_h: f32,
    band: [f32; 3],
) {
    let weights = spectral_weights(band);
    let x0 = (x0 - 0.35).max(0.0);
    let x1 = (x1 + 0.35).min(width as f32).max(x0 + 0.75);
    fill_rect_f32(
        pixels,
        width,
        height,
        x0,
        center_y - outer_half_h,
        x1,
        center_y + outer_half_h,
        egui::Color32::from_rgba_unmultiplied(126, 104, 62, 52),
    );

    draw_spectral_half_pixels(
        pixels,
        width,
        height,
        x0,
        x1,
        center_y,
        -1.0,
        outer_half_h,
        weights,
        88,
    );
    draw_spectral_half_pixels(
        pixels,
        width,
        height,
        x0,
        x1,
        center_y,
        1.0,
        outer_half_h,
        weights,
        88,
    );
    draw_spectral_half_pixels(
        pixels,
        width,
        height,
        x0,
        x1,
        center_y,
        -1.0,
        core_half_h,
        weights,
        218,
    );
    draw_spectral_half_pixels(
        pixels,
        width,
        height,
        x0,
        x1,
        center_y,
        1.0,
        core_half_h,
        weights,
        218,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_spectral_half_pixels(
    pixels: &mut [egui::Color32],
    width: usize,
    height: usize,
    x0: f32,
    x1: f32,
    center_y: f32,
    direction: f32,
    half_h: f32,
    weights: [f32; 3],
    alpha: u8,
) {
    let colors = [
        egui::Color32::from_rgb(222, 154, 58),
        egui::Color32::from_rgb(126, 210, 90),
        egui::Color32::from_rgb(78, 186, 236),
    ];
    let mut cursor = center_y;
    for (idx, weight) in weights.into_iter().enumerate() {
        let h = (half_h * weight).max(0.0);
        if h < 0.35 {
            continue;
        }
        let next = cursor + direction * h;
        fill_rect_f32(
            pixels,
            width,
            height,
            x0,
            cursor.min(next),
            x1,
            cursor.max(next),
            color_with_alpha(colors[idx], alpha),
        );
        cursor = next;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_rect_stroke_px(
    pixels: &mut [egui::Color32],
    width: usize,
    height: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    color: egui::Color32,
) {
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    fill_rect_px(pixels, width, height, x0, y0, x1, (y0 + 1).min(y1), color);
    fill_rect_px(
        pixels,
        width,
        height,
        x0,
        y1.saturating_sub(1),
        x1,
        y1,
        color,
    );
    fill_rect_px(pixels, width, height, x0, y0, (x0 + 1).min(x1), y1, color);
    fill_rect_px(
        pixels,
        width,
        height,
        x1.saturating_sub(1),
        y0,
        x1,
        y1,
        color,
    );
}

#[allow(clippy::too_many_arguments)]
fn fill_rect_f32(
    pixels: &mut [egui::Color32],
    width: usize,
    height: usize,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    color: egui::Color32,
) {
    let x0 = x0.floor().clamp(0.0, width as f32) as usize;
    let y0 = y0.floor().clamp(0.0, height as f32) as usize;
    let x1 = x1.ceil().clamp(0.0, width as f32) as usize;
    let y1 = y1.ceil().clamp(0.0, height as f32) as usize;
    fill_rect_px(pixels, width, height, x0, y0, x1, y1, color);
}

#[allow(clippy::too_many_arguments)]
fn fill_rect_px(
    pixels: &mut [egui::Color32],
    width: usize,
    height: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    color: egui::Color32,
) {
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let x1 = x1.min(width);
    let y1 = y1.min(height);
    for y in y0.min(height)..y1 {
        let row = y * width;
        for x in x0.min(width)..x1 {
            blend_pixel(&mut pixels[row + x], color);
        }
    }
}

fn blend_pixel(dst: &mut egui::Color32, src: egui::Color32) {
    let alpha = src.a() as u32;
    if alpha >= 255 {
        *dst = egui::Color32::from_rgb(src.r(), src.g(), src.b());
        return;
    }
    if alpha == 0 {
        return;
    }
    let inv = 255 - alpha;
    let blend = |s: u8, d: u8| ((s as u32 * alpha + d as u32 * inv + 127) / 255) as u8;
    *dst = egui::Color32::from_rgb(
        blend(src.r(), dst.r()),
        blend(src.g(), dst.g()),
        blend(src.b(), dst.b()),
    );
}

fn draw_beat_grid(
    painter: &egui::Painter,
    rect: egui::Rect,
    row_start: f64,
    row_secs: f64,
    analysis: &TimelineAnalysis,
) {
    if analysis.beat_grid.confidence < BEAT_GRID_MIN_CONFIDENCE {
        return;
    }
    let row_end = row_start + row_secs;
    let alpha_scale = ((analysis.beat_grid.confidence - BEAT_GRID_MIN_CONFIDENCE)
        / (1.0 - BEAT_GRID_MIN_CONFIDENCE))
        .clamp(0.0, 1.0);
    for beat in &analysis.beat_grid.beats {
        if beat.time_secs < row_start || beat.time_secs > row_end {
            continue;
        }
        let x = rect.left() + ((beat.time_secs - row_start) / row_secs) as f32 * rect.width();
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(
                0.5,
                egui::Color32::from_rgba_unmultiplied(
                    150,
                    190,
                    220,
                    (24.0 + alpha_scale * 54.0) as u8,
                ),
            ),
        );
    }
    let mut last_bar_label_x = f32::NEG_INFINITY;
    for bar in &analysis.beat_grid.bars {
        if bar.time_secs < row_start || bar.time_secs > row_end {
            continue;
        }
        let x = rect.left() + ((bar.time_secs - row_start) / row_secs) as f32 * rect.width();
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(
                1.5,
                egui::Color32::from_rgba_unmultiplied(
                    55,
                    170,
                    255,
                    (72.0 + alpha_scale * 92.0) as u8,
                ),
            ),
        );
        if x - last_bar_label_x >= 32.0 {
            painter.text(
                egui::pos2(x + 3.0, rect.top() + 3.0),
                egui::Align2::LEFT_TOP,
                (bar.index + 1).to_string(),
                egui::FontId::monospace(10.0),
                egui::Color32::from_rgba_unmultiplied(95, 185, 255, 210),
            );
            last_bar_label_x = x;
        }
    }
}

fn draw_playhead(
    painter: &egui::Painter,
    graph_rect: egui::Rect,
    position_secs: f64,
    row_secs: f64,
    row_h: f32,
    row_gap: f32,
) {
    if position_secs <= 0.0 || row_secs <= 0.0 {
        return;
    }
    let row = (position_secs / row_secs).floor() as usize;
    let row_start = row as f64 * row_secs;
    let row_top = graph_rect.min.y + row as f32 * (row_h + row_gap);
    let row_rect = egui::Rect::from_min_size(
        egui::pos2(graph_rect.min.x, row_top),
        egui::vec2(graph_rect.width(), row_h),
    );
    if !graph_rect.intersects(row_rect) {
        return;
    }
    let x = row_rect.left() + ((position_secs - row_start) / row_secs) as f32 * row_rect.width();
    painter.line_segment(
        [
            egui::pos2(x, row_rect.top()),
            egui::pos2(x, row_rect.bottom()),
        ],
        egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 230, 80)),
    );
}

fn draw_spectrum(
    ui: &mut egui::Ui,
    bands: &[f32],
    trail: &mut Vec<f32>,
    prev_bands: &mut Vec<f32>,
    onsets: &mut Vec<f32>,
    notes: &[f32],
    note_sustain: &mut Vec<f32>,
    note_trail: &mut Vec<f32>,
) {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::BLACK);

    let inner = rect.shrink2(egui::vec2(18.0, 12.0));
    let keyboard_rect = egui::Rect::from_min_max(
        egui::pos2(inner.left(), inner.bottom() - SPECTRUM_KEYBOARD_H),
        inner.right_bottom(),
    );
    let plot = egui::Rect::from_min_max(
        inner.min,
        egui::pos2(
            inner.right(),
            (keyboard_rect.top() - SPECTRUM_PANEL_GAP).max(inner.top()),
        ),
    );
    painter.rect_filled(plot, 0.0, egui::Color32::BLACK);
    painter.rect_stroke(
        plot,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(70, 92, 116, 130)),
        egui::StrokeKind::Inside,
    );
    painter.line_segment(
        [
            egui::pos2(plot.left(), plot.bottom() - 1.0),
            egui::pos2(plot.right(), plot.bottom() - 1.0),
        ],
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 35),
        ),
    );

    if bands.is_empty() {
        draw_pitch_keyboard(&painter, keyboard_rect, notes, note_sustain, note_trail);
        return;
    }
    if trail.len() != bands.len() {
        trail.clear();
        trail.resize(bands.len(), 0.0);
    }
    if prev_bands.len() != bands.len() {
        prev_bands.clear();
        prev_bands.extend_from_slice(bands);
    }
    if onsets.len() != bands.len() {
        onsets.clear();
        onsets.resize(bands.len(), 0.0);
    }

    for (i, value) in bands.iter().enumerate() {
        let value = value.clamp(0.0, 1.0);
        let rise = (value - prev_bands[i]).max(0.0);
        onsets[i] = (onsets[i] * 0.86).max((rise * 2.8).clamp(0.0, 1.0));
        prev_bands[i] = prev_bands[i] * 0.25 + value * 0.75;
        trail[i] = (trail[i] * SPECTRUM_TRAIL_DECAY).max(value);
        let (band_low_hz, band_high_hz) = spectrum_band_hz_range(i, bands.len());
        let x0 = (spectrum_axis_x(plot, band_low_hz) + 0.25).max(plot.left());
        let x1 = (spectrum_axis_x(plot, band_high_hz) - 0.25)
            .max(x0 + 0.75)
            .min(plot.right());
        let band_corner = if x1 - x0 < 2.0 { 0.0 } else { 1.0 };
        let ghost_h = (plot.height() - 3.0) * (trail[i] * 0.72).max(0.012);
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2((x0 - 0.45).max(plot.left()), plot.bottom() - 2.0 - ghost_h),
                egui::pos2((x1 + 0.45).min(plot.right()), plot.bottom() - 2.0),
            ),
            band_corner,
            color_with_alpha(spectrum_color(i, bands.len(), trail[i]), 48),
        );
        let trail_h = (plot.height() - 3.0) * trail[i].max(0.015);
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2((x0 - 0.2).max(plot.left()), plot.bottom() - 2.0 - trail_h),
                egui::pos2((x1 + 0.2).min(plot.right()), plot.bottom() - 2.0),
            ),
            band_corner,
            color_with_alpha(spectrum_color(i, bands.len(), trail[i]), 100),
        );
        let h = (plot.height() - 3.0) * value.max(0.015);
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x0, plot.bottom() - 2.0 - h),
                egui::pos2(x1, plot.bottom() - 2.0),
            ),
            band_corner,
            spectrum_color(i, bands.len(), value),
        );
        let onset = onsets[i].clamp(0.0, 1.0);
        if onset > 0.025 {
            let accent = brighten_color(spectrum_color(i, bands.len(), value.max(onset)), 1.18);
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x0, plot.bottom() - 2.0 - h),
                    egui::pos2(x1, plot.bottom() - 2.0),
                ),
                band_corner,
                color_with_alpha(accent, (24.0 + onset * 68.0) as u8),
            );
        }
    }
    if let Some(pointer) = response
        .hover_pos()
        .filter(|pointer| plot.contains(*pointer) || keyboard_rect.contains(*pointer))
    {
        draw_spectrum_hover(&painter, plot, pointer);
    }
    draw_pitch_keyboard(&painter, keyboard_rect, notes, note_sustain, note_trail);
}

fn draw_spectrum_hover(painter: &egui::Painter, plot: egui::Rect, pointer: egui::Pos2) {
    let x = pointer.x.clamp(plot.left(), plot.right());
    let hz = spectrum_axis_hz(plot, x);
    let label = format!("{:.1} Hz  {}", hz, note_label_for_hz(hz));
    painter.line_segment(
        [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(230, 240, 255, 150),
        ),
    );
    let label_w = (label.chars().count() as f32 * 7.2 + 16.0).min(plot.width());
    let label_h = 22.0;
    let label_x = if x + 10.0 + label_w <= plot.right() {
        x + 10.0
    } else {
        (x - 10.0 - label_w).max(plot.left() + 4.0)
    };
    let label_rect = egui::Rect::from_min_size(
        egui::pos2(label_x, plot.top() + 7.0),
        egui::vec2(label_w, label_h),
    );
    painter.rect_filled(
        label_rect,
        3.0,
        egui::Color32::from_rgba_unmultiplied(5, 8, 12, 224),
    );
    painter.rect_stroke(
        label_rect,
        3.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(120, 150, 176, 180),
        ),
        egui::StrokeKind::Inside,
    );
    painter.text(
        label_rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(12.0),
        egui::Color32::from_rgb(238, 244, 250),
    );
}

fn draw_pitch_keyboard(
    painter: &egui::Painter,
    rect: egui::Rect,
    notes: &[f32],
    note_sustain: &mut Vec<f32>,
    note_trail: &mut Vec<f32>,
) {
    painter.rect_filled(rect, 0.0, egui::Color32::BLACK);
    let note_count = (SPECTRUM_NOTE_MAX_MIDI - SPECTRUM_NOTE_MIN_MIDI + 1) as usize;
    if note_sustain.len() != note_count {
        note_sustain.clear();
        note_sustain.resize(note_count, 0.0);
    }
    if note_trail.len() != note_count {
        note_trail.clear();
        note_trail.resize(note_count, 0.0);
    }
    let sustained_notes = update_keyboard_sustain(notes, note_sustain);
    let highlight_targets = keyboard_highlight_targets(&sustained_notes, note_count);
    for (trail, target) in note_trail.iter_mut().zip(highlight_targets) {
        *trail = (*trail * KEY_HIGHLIGHT_DECAY).max(target);
    }

    for c_midi in (KEYBOARD_DISPLAY_MIN_MIDI..=144_u8).step_by(12) {
        let x = spectrum_axis_x(rect, midi_to_hz(c_midi));
        if x > rect.left() && x < rect.right() {
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(
                    0.8,
                    egui::Color32::from_rgba_unmultiplied(120, 134, 148, 82),
                ),
            );
        }
    }

    for midi in KEYBOARD_DISPLAY_MIN_MIDI..=KEYBOARD_DISPLAY_MAX_MIDI {
        if is_black_key(midi) {
            continue;
        }
        let Some(key_rect) = conventional_key_rect(rect, midi, false) else {
            continue;
        };
        let real_key = (SPECTRUM_NOTE_MIN_MIDI..=SPECTRUM_NOTE_MAX_MIDI).contains(&midi);
        let value = real_key
            .then(|| note_trail[(midi - SPECTRUM_NOTE_MIN_MIDI) as usize])
            .unwrap_or(0.0);
        let base = if real_key {
            egui::Color32::from_rgb(208, 214, 219)
        } else {
            egui::Color32::from_rgb(58, 64, 70)
        };
        let active = key_color(midi, value);
        let fill = if real_key {
            lerp_color(base, active, (value * 0.94).clamp(0.0, 1.0))
        } else {
            base
        };
        painter.rect_filled(key_rect, 0.0, fill);
        painter.rect_stroke(
            key_rect,
            0.0,
            egui::Stroke::new(0.8, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 155)),
            egui::StrokeKind::Inside,
        );
    }

    for midi in KEYBOARD_DISPLAY_MIN_MIDI..=KEYBOARD_DISPLAY_MAX_MIDI {
        if !is_black_key(midi) {
            continue;
        }
        let Some(key_rect) = conventional_key_rect(rect, midi, true) else {
            continue;
        };
        let real_key = (SPECTRUM_NOTE_MIN_MIDI..=SPECTRUM_NOTE_MAX_MIDI).contains(&midi);
        let value = real_key
            .then(|| note_trail[(midi - SPECTRUM_NOTE_MIN_MIDI) as usize])
            .unwrap_or(0.0);
        let base = if real_key {
            egui::Color32::from_rgb(13, 16, 19)
        } else {
            egui::Color32::from_rgb(34, 39, 44)
        };
        let active = key_color(midi, value);
        let fill = if real_key {
            lerp_color(base, active, (value * 0.96).clamp(0.0, 1.0))
        } else {
            base
        };
        painter.rect_filled(key_rect, 1.0, fill);
        painter.rect_stroke(
            key_rect,
            1.0,
            egui::Stroke::new(
                0.75,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 80),
            ),
            egui::StrokeKind::Inside,
        );
    }
}

fn update_keyboard_sustain(notes: &[f32], sustain: &mut [f32]) -> Vec<f32> {
    let mut sustained = Vec::with_capacity(sustain.len());
    let raw_peak = (0..sustain.len())
        .map(|idx| notes.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0))
        .fold(0.0_f32, f32::max);
    let broad_threshold = raw_peak * 0.42;
    let broad_count = if raw_peak > KEY_HIGHLIGHT_MIN_PEAK {
        (0..sustain.len())
            .filter(|idx| notes.get(*idx).copied().unwrap_or(0.0).clamp(0.0, 1.0) > broad_threshold)
            .count()
    } else {
        0
    };
    let broad_ratio = broad_count as f32 / sustain.len().max(1) as f32;
    let attack_scale = if broad_ratio > 0.34 { 0.35 } else { 1.0 };

    for (idx, slot) in sustain.iter_mut().enumerate() {
        let current = notes.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        if current >= *slot {
            let attack = KEY_SUSTAIN_ATTACK * attack_scale;
            *slot = *slot * (1.0 - attack) + current * attack;
        } else {
            *slot = (*slot * KEY_SUSTAIN_RELEASE).max(current);
        }
        sustained.push(*slot);
    }
    sustained
}

fn keyboard_highlight_targets(notes: &[f32], note_count: usize) -> Vec<f32> {
    let raw: Vec<f32> = (0..note_count)
        .map(|idx| notes.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0))
        .collect();
    let peak = raw.iter().copied().fold(0.0_f32, f32::max);
    if peak < KEY_HIGHLIGHT_MIN_PEAK {
        return vec![0.0; note_count];
    }

    let mut prominence = vec![0.0; note_count];
    for idx in 0..note_count {
        let value = raw[idx];
        let near = local_shoulder(&raw, idx);
        let bed = local_spectral_bed(&raw, idx);
        let relative_loudness = (value / peak).clamp(0.0, 1.0);
        prominence[idx] = (value - near - bed * 0.18).max(0.0) * relative_loudness.powf(2.0);
    }

    let prominence_peak = prominence.iter().copied().fold(0.0_f32, f32::max);
    if prominence_peak <= 1.0e-6 || prominence_peak < peak * 0.12 {
        return vec![0.0; note_count];
    }
    let floor = (prominence_peak * 0.08).max(0.006);
    let sustained_presence = ((peak - 0.06) / 0.28).clamp(0.0, 1.0).powf(0.7);
    prominence
        .into_iter()
        .zip(raw)
        .map(|(value, raw_value)| {
            let contrast = ((value - floor) / (prominence_peak - floor).max(1.0e-6))
                .clamp(0.0, 1.0)
                .powf(1.35);
            let loudness = (raw_value / peak).clamp(0.0, 1.0).powf(0.20);
            contrast * loudness * sustained_presence
        })
        .collect()
}

fn local_shoulder(values: &[f32], idx: usize) -> f32 {
    let mut shoulder = 0.0_f32;
    for offset in [-2_isize, -1, 1, 2] {
        let Some(neighbor_idx) = idx.checked_add_signed(offset) else {
            continue;
        };
        let Some(value) = values.get(neighbor_idx) else {
            continue;
        };
        let weight = if offset.abs() == 1 { 0.72 } else { 0.52 };
        shoulder = shoulder.max(value * weight);
    }
    shoulder
}

fn local_spectral_bed(values: &[f32], idx: usize) -> f32 {
    let mut weighted_sum = 0.0;
    let mut weight_sum = 0.0;
    for offset in -6_isize..=6 {
        let distance = offset.abs();
        if distance <= 2 {
            continue;
        }
        let Some(neighbor_idx) = idx.checked_add_signed(offset) else {
            continue;
        };
        let Some(value) = values.get(neighbor_idx) else {
            continue;
        };
        let weight = 1.0 / distance as f32;
        weighted_sum += value * weight;
        weight_sum += weight;
    }
    if weight_sum > 0.0 {
        weighted_sum / weight_sum
    } else {
        0.0
    }
}

fn conventional_key_rect(rect: egui::Rect, midi: u8, black: bool) -> Option<egui::Rect> {
    let pc = midi % 12;
    let octave_c = midi - pc;
    let (x0, white_w) = conventional_octave_geometry(rect, octave_c)?;
    let (left, right, bottom) = if black {
        let center = match pc {
            1 => 1.0,
            3 => 2.0,
            6 => 4.0,
            8 => 5.0,
            10 => 6.0,
            _ => return None,
        };
        let w = white_w * 0.56;
        (
            x0 + white_w * center - w * 0.5,
            x0 + white_w * center + w * 0.5,
            rect.top() + rect.height() * 0.64,
        )
    } else {
        let index = match pc {
            0 => 0,
            2 => 1,
            4 => 2,
            5 => 3,
            7 => 4,
            9 => 5,
            11 => 6,
            _ => return None,
        } as f32;
        (
            x0 + white_w * index,
            x0 + white_w * (index + 1.0),
            rect.bottom(),
        )
    };
    let left = left.max(rect.left()) + 0.25;
    let right = right.min(rect.right()) - 0.25;
    if right <= left {
        return None;
    }
    Some(egui::Rect::from_min_max(
        egui::pos2(left, rect.top()),
        egui::pos2(right, bottom),
    ))
}

fn conventional_octave_geometry(rect: egui::Rect, octave_c: u8) -> Option<(f32, f32)> {
    let c_x = spectrum_axis_x(rect, midi_to_hz(octave_c));
    let next_c_x = spectrum_axis_x(rect, midi_to_hz(octave_c.saturating_add(12)));
    let octave_w = next_c_x - c_x;
    if octave_w <= 1.0 {
        return None;
    }
    let white_w = octave_w / 7.0;
    Some((c_x - white_w * 0.5, white_w))
}

fn spectrum_axis_x(rect: egui::Rect, hz: f32) -> f32 {
    let min = SPECTRUM_AXIS_MIN_HZ;
    let max = SPECTRUM_VIEW_MAX_HZ;
    let t = (hz.clamp(min, max).log2() - min.log2()) / (max.log2() - min.log2());
    rect.left() + t.clamp(0.0, 1.0) * rect.width()
}

fn spectrum_axis_hz(rect: egui::Rect, x: f32) -> f32 {
    let min = SPECTRUM_AXIS_MIN_HZ;
    let max = SPECTRUM_VIEW_MAX_HZ;
    let t = ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
    2.0_f32.powf(min.log2() + t * (max.log2() - min.log2()))
}

fn spectrum_band_hz(index: usize, total: usize) -> f32 {
    if total <= 1 {
        return SPECTRUM_ANALYSIS_MIN_HZ;
    }
    let ratio = SPECTRUM_VIEW_MAX_HZ / SPECTRUM_ANALYSIS_MIN_HZ;
    let t = index as f32 / (total - 1) as f32;
    SPECTRUM_ANALYSIS_MIN_HZ * ratio.powf(t)
}

fn spectrum_band_hz_range(index: usize, total: usize) -> (f32, f32) {
    if total <= 1 {
        let half = 2.0_f32.powf(1.0 / 24.0);
        return (
            SPECTRUM_ANALYSIS_MIN_HZ / half,
            SPECTRUM_ANALYSIS_MIN_HZ * half,
        );
    }
    let center = spectrum_band_hz(index, total);
    let ratio = SPECTRUM_VIEW_MAX_HZ / SPECTRUM_ANALYSIS_MIN_HZ;
    let step = ratio.powf(1.0 / (total - 1) as f32);
    let edge_scale = step.sqrt();
    (center / edge_scale, center * edge_scale)
}

fn midi_to_hz(midi: u8) -> f32 {
    440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0)
}

fn note_label_for_hz(hz: f32) -> String {
    if !hz.is_finite() || hz <= 0.0 {
        return "--".to_string();
    }
    let midi_exact = 69.0 + 12.0 * (hz / 440.0).log2();
    let nearest = midi_exact.round() as i32;
    let cents = ((midi_exact - nearest as f32) * 100.0).round() as i32;
    let name = note_name_from_midi(nearest);
    if cents == 0 {
        name
    } else if cents > 0 {
        format!("{name} +{cents}c")
    } else {
        format!("{name} {cents}c")
    }
}

fn note_name_from_midi(midi: i32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let pitch_class = midi.rem_euclid(12) as usize;
    let octave = midi.div_euclid(12) - 1;
    format!("{}{}", NAMES[pitch_class], octave)
}

fn pitch_class_name(pitch_class: u8) -> &'static str {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    NAMES[pitch_class as usize % 12]
}

fn is_black_key(midi: u8) -> bool {
    matches!(midi % 12, 1 | 3 | 6 | 8 | 10)
}

fn key_color(midi: u8, value: f32) -> egui::Color32 {
    let pitch_class = midi % 12;
    let fifth_index = ((pitch_class as usize * 7) % 12) as f32;
    let hue = (fifth_index / 12.0 + 0.57) % 1.0;
    let saturation = 0.62 + value.clamp(0.0, 1.0) * 0.28;
    let brightness = 0.38 + value.clamp(0.0, 1.0) * 0.58;
    hsv_to_rgb(hue, saturation, brightness)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> egui::Color32 {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor() as i32;
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i.rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    egui::Color32::from_rgb(
        (r.clamp(0.0, 1.0) * 255.0) as u8,
        (g.clamp(0.0, 1.0) * 255.0) as u8,
        (b.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

fn spectral_weights(band: [f32; 3]) -> [f32; 3] {
    let low = (band[0] * 0.95).sqrt().clamp(0.0, 1.0);
    let mid = (band[1] * 0.88).sqrt().clamp(0.0, 1.0);
    let high = (band[2] * 1.22).sqrt().clamp(0.0, 1.0);
    let sum = (low + mid + high).max(1.0e-6);
    [low / sum, mid / sum, high / sum]
}

fn transient_color(band: [f32; 3], strength: f32) -> egui::Color32 {
    let weights = spectral_weights(band);
    let low = weights[0];
    let mid = weights[1];
    let high = weights[2];
    let color = egui::Color32::from_rgb(
        ((238.0 * low + 205.0 * mid + 70.0 * high).min(255.0)) as u8,
        ((126.0 * low + 92.0 * mid + 220.0 * high).min(255.0)) as u8,
        ((32.0 * low + 210.0 * mid + 255.0 * high).min(255.0)) as u8,
    );
    brighten_color(color, 1.0 + strength.clamp(0.0, 1.0) * 0.24)
}

fn spectrum_color(index: usize, total: usize, value: f32) -> egui::Color32 {
    let t = if total <= 1 {
        0.0
    } else {
        index as f32 / (total - 1) as f32
    };
    let base = if t < 0.20 {
        lerp_color(
            egui::Color32::from_rgb(188, 58, 34),
            egui::Color32::from_rgb(236, 132, 34),
            t / 0.20,
        )
    } else if t < 0.52 {
        lerp_color(
            egui::Color32::from_rgb(255, 216, 28),
            egui::Color32::from_rgb(70, 232, 76),
            (t - 0.20) / 0.32,
        )
    } else if t < 0.78 {
        lerp_color(
            egui::Color32::from_rgb(38, 230, 190),
            egui::Color32::from_rgb(44, 138, 255),
            (t - 0.52) / 0.26,
        )
    } else {
        lerp_color(
            egui::Color32::from_rgb(44, 138, 255),
            egui::Color32::from_rgb(245, 82, 210),
            (t - 0.78) / 0.22,
        )
    };
    let alpha = (92.0 + 150.0 * value.clamp(0.0, 1.0)) as u8;
    color_with_alpha(
        brighten_color(base, 0.55 + value.clamp(0.0, 1.0) * 0.65),
        alpha,
    )
}

fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |av: u8, bv: u8| av as f32 + (bv as f32 - av as f32) * t;
    egui::Color32::from_rgb(
        lerp(a.r(), b.r()) as u8,
        lerp(a.g(), b.g()) as u8,
        lerp(a.b(), b.b()) as u8,
    )
}

fn brighten_color(color: egui::Color32, scale: f32) -> egui::Color32 {
    egui::Color32::from_rgb(
        ((color.r() as f32 * scale).min(255.0)) as u8,
        ((color.g() as f32 * scale).min(255.0)) as u8,
        ((color.b() as f32 * scale).min(255.0)) as u8,
    )
}

fn color_with_alpha(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn is_supported_media_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            let ext = ext.trim_start_matches('.').to_ascii_lowercase();
            MEDIA_EXTENSIONS
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&ext))
        })
}

fn probe_audio_file(path: &Path) -> Result<AudioStreamInfo, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("probe: {e}"))?;
    let format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| {
            track.codec_params.codec != CODEC_TYPE_NULL
                && track.codec_params.sample_rate.is_some()
                && track.codec_params.channels.is_some()
        })
        .or_else(|| {
            format.tracks().iter().find(|track| {
                track.codec_params.codec != CODEC_TYPE_NULL
                    && (track.codec_params.sample_rate.is_some()
                        || track.codec_params.channels.is_some())
            })
        })
        .or_else(|| {
            format
                .tracks()
                .iter()
                .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        })
        .ok_or_else(|| "no supported audio track".to_string())?;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(0);
    let channels = track
        .codec_params
        .channels
        .map(|channels| channels.count() as u16)
        .unwrap_or(0);
    let duration_secs = track
        .codec_params
        .n_frames
        .and_then(|frames| {
            if sample_rate > 0 {
                Some(frames as f64 / sample_rate as f64)
            } else {
                None
            }
        })
        .unwrap_or(0.0);
    Ok(AudioStreamInfo {
        sample_rate,
        channels,
        duration_secs,
    })
}

fn decode_audio_file(
    path: &Path,
    cancel: &AtomicBool,
    streaming_sink: Option<&StreamingAudioSink>,
    partial_tx: Option<&mpsc::Sender<LoadMsg>>,
) -> Result<DecodedAudio, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("probe: {e}"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| {
            track.codec_params.codec != CODEC_TYPE_NULL
                && track.codec_params.sample_rate.is_some()
                && track.codec_params.channels.is_some()
        })
        .or_else(|| {
            format.tracks().iter().find(|track| {
                track.codec_params.codec != CODEC_TYPE_NULL
                    && (track.codec_params.sample_rate.is_some()
                        || track.codec_params.channels.is_some())
            })
        })
        .or_else(|| {
            format
                .tracks()
                .iter()
                .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        })
        .ok_or_else(|| "no supported audio track".to_string())?;
    let track_id = track.id;
    let codec_params = track.codec_params.clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .map_err(|e| format!("decoder: {e}"))?;

    let mut stereo_samples = Vec::new();
    let mut partial_samples = Vec::new();
    let mut partial_start_secs = 0.0_f64;
    let mut stream_info = AudioStreamInfo::default();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_string());
        }
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(_)) => break,
            Err(SymphoniaError::ResetRequired) => {
                return Err("decoder reset required; not handled in lab".to_string());
            }
            Err(err) => return Err(format!("packet: {err}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(err) => return Err(format!("decode: {err}")),
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        let mut sample_buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);
        let samples = sample_buf.samples();
        stream_info.sample_rate = spec.rate;
        stream_info.channels = channels as u16;
        let mut packet_stereo = Vec::with_capacity(samples.len() / channels * 2);
        for frame in samples.chunks(channels) {
            let left: f32 = frame.first().copied().unwrap_or(0.0).into_sample();
            let right: f32 = frame.get(1).copied().unwrap_or(left).into_sample();
            packet_stereo.push(left.clamp(-1.0, 1.0));
            packet_stereo.push(right.clamp(-1.0, 1.0));
        }
        if let Some(sink) = streaming_sink {
            sink.append_source_samples(&packet_stereo, spec.rate);
        }
        if partial_tx.is_some() {
            partial_samples.extend_from_slice(&packet_stereo);
            flush_ready_partial_timeline(
                partial_tx,
                &mut partial_samples,
                &mut partial_start_secs,
                spec.rate,
                false,
            );
        }
        stereo_samples.extend_from_slice(&packet_stereo);
    }
    stream_info.duration_secs =
        stereo_samples.len() as f64 / 2.0 / stream_info.sample_rate.max(1) as f64;
    if stereo_samples.is_empty() {
        return Err("no decoded samples".to_string());
    }
    if partial_tx.is_some() {
        flush_ready_partial_timeline(
            partial_tx,
            &mut partial_samples,
            &mut partial_start_secs,
            stream_info.sample_rate,
            true,
        );
    }
    if let Some(sink) = streaming_sink {
        sink.finish(stream_info.duration_secs);
    }
    Ok(DecodedAudio {
        info: stream_info,
        stereo_samples,
    })
}

fn flush_ready_partial_timeline(
    partial_tx: Option<&mpsc::Sender<LoadMsg>>,
    partial_samples: &mut Vec<f32>,
    partial_start_secs: &mut f64,
    sample_rate: u32,
    force: bool,
) {
    let Some(tx) = partial_tx else {
        return;
    };
    let sample_rate = sample_rate.max(1);
    let chunk_frames = (TIMELINE_PARTIAL_CHUNK_SECS * sample_rate as f64)
        .round()
        .max(1.0) as usize;
    loop {
        let available_frames = partial_samples.len() / 2;
        if available_frames < chunk_frames && !(force && available_frames > 0) {
            break;
        }
        let take_frames = if force {
            available_frames.min(chunk_frames).max(1)
        } else {
            chunk_frames
        };
        let take_samples = take_frames * 2;
        let rest = partial_samples.split_off(take_samples);
        let chunk = std::mem::replace(partial_samples, rest);
        let decoded_duration_secs = *partial_start_secs + take_frames as f64 / sample_rate as f64;
        let mut analysis =
            analyze_stereo_timeline(&chunk, sample_rate, partial_timeline_analysis_config());
        for bin in &mut analysis.bins {
            bin.start_secs += *partial_start_secs;
        }
        *partial_start_secs = decoded_duration_secs;
        let _ = tx.send(LoadMsg::PartialTimeline {
            bins: analysis.bins,
            decoded_duration_secs,
        });
        if force && partial_samples.is_empty() {
            break;
        }
    }
}

struct LabPlayer {
    _stream: cpal::Stream,
    shared: Arc<Mutex<PlayerShared>>,
}

struct PlayerShared {
    samples: Vec<f32>,
    sample_rate: u32,
    duration_secs: f64,
    position_frames: usize,
    playing: bool,
    eof: bool,
}

#[derive(Clone)]
struct StreamingAudioSink {
    shared: Arc<Mutex<PlayerShared>>,
    output_rate: u32,
}

impl LabPlayer {
    fn new(decoded: Arc<DecodedAudio>, autoplay: bool) -> Result<Self, String> {
        let (player, sink) = Self::new_streaming(decoded.info, autoplay)?;
        sink.append_source_samples(&decoded.stereo_samples, decoded.info.sample_rate);
        sink.finish(decoded.info.duration_secs);
        Ok(player)
    }

    fn new_streaming(
        info: AudioStreamInfo,
        autoplay: bool,
    ) -> Result<(Self, StreamingAudioSink), String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no default output device".to_string())?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        if config.sample_format() != cpal::SampleFormat::F32 {
            return Err(format!(
                "lab currently supports f32 output only, got {:?}",
                config.sample_format()
            ));
        }
        let output_rate = config.sample_rate().0;
        let shared = Arc::new(Mutex::new(PlayerShared {
            samples: Vec::new(),
            sample_rate: output_rate,
            duration_secs: info.duration_secs,
            position_frames: 0,
            playing: autoplay,
            eof: false,
        }));
        let stream_shared = Arc::clone(&shared);
        let stream_config: cpal::StreamConfig = config.into();
        let out_channels = stream_config.channels.max(1) as usize;
        let stream = device
            .build_output_stream(
                &stream_config,
                move |out: &mut [f32], _| fill_output(out, out_channels, &stream_shared),
                move |err| eprintln!("music_lab audio stream error: {err}"),
                None,
            )
            .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        let sink = StreamingAudioSink {
            shared: Arc::clone(&shared),
            output_rate,
        };
        Ok((
            Self {
                _stream: stream,
                shared,
            },
            sink,
        ))
    }

    fn set_playing(&self, playing: bool) {
        if let Ok(mut s) = self.shared.lock() {
            s.playing = playing;
        }
    }

    fn seek_secs(&self, secs: f64) {
        if let Ok(mut s) = self.shared.lock() {
            let frame = (secs.max(0.0) * s.sample_rate as f64) as usize;
            s.position_frames = if s.eof {
                frame.min(s.samples.len() / 2)
            } else {
                frame
            };
        }
    }

    fn set_duration_secs(&self, duration_secs: f64) {
        if duration_secs.is_finite()
            && duration_secs > 0.0
            && let Ok(mut s) = self.shared.lock()
        {
            s.duration_secs = duration_secs;
        }
    }

    fn snapshot(&self) -> PlaybackSnapshot {
        let Ok(s) = self.shared.lock() else {
            return PlaybackSnapshot::default();
        };
        PlaybackSnapshot {
            position_secs: s.position_frames as f64 / s.sample_rate.max(1) as f64,
            duration_secs: s.duration_secs,
            playing: s.playing,
            effect_chain_active: false,
            effect_latency_samples: 0,
        }
    }
}

impl StreamingAudioSink {
    fn append_source_samples(&self, source_samples: &[f32], source_rate: u32) {
        if source_samples.is_empty() {
            return;
        }
        let output_samples = if source_rate == self.output_rate {
            source_samples.to_vec()
        } else {
            resample_linear_stereo(source_samples, source_rate, self.output_rate)
        };
        if output_samples.is_empty() {
            return;
        }
        if let Ok(mut shared) = self.shared.lock() {
            shared.samples.extend_from_slice(&output_samples);
        }
    }

    fn finish(&self, duration_secs: f64) {
        if let Ok(mut shared) = self.shared.lock() {
            if duration_secs.is_finite() && duration_secs > 0.0 {
                shared.duration_secs = duration_secs;
            }
            shared.eof = true;
        }
    }

    fn fail(&self) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.eof = true;
            shared.playing = false;
        }
    }
}

fn fill_output(out: &mut [f32], out_channels: usize, shared: &Arc<Mutex<PlayerShared>>) {
    let Ok(mut state) = shared.lock() else {
        out.fill(0.0);
        return;
    };
    for frame in out.chunks_mut(out_channels) {
        let available_frames = state.samples.len() / 2;
        let (l, r) = if state.playing && state.position_frames < available_frames {
            let i = state.position_frames * 2;
            state.position_frames += 1;
            (state.samples[i], state.samples[i + 1])
        } else {
            if state.playing && state.eof {
                state.playing = false;
            }
            (0.0, 0.0)
        };
        for (ch, sample) in frame.iter_mut().enumerate() {
            *sample = match ch {
                0 => l,
                1 => r,
                _ => (l + r) * 0.5,
            };
        }
    }
}

fn format_time(secs: f64) -> String {
    if !secs.is_finite() {
        return "--:--".to_string();
    }
    let secs = secs.max(0.0).round() as u64;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn format_row_secs(secs: f64) -> String {
    if secs < 60.0 {
        format!("{secs:.0}s")
    } else {
        let minutes = secs / 60.0;
        if minutes.fract().abs() <= f64::EPSILON {
            format!("{minutes:.0}m")
        } else {
            format!("{minutes:.1}m")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_chroma(root: usize, mode: TimelineKeyMode) -> [f32; 12] {
        let profile = match mode {
            TimelineKeyMode::Major => TIMELINE_KEY_KRUMHANSL_MAJOR_PROFILE,
            TimelineKeyMode::Minor => TIMELINE_KEY_KRUMHANSL_MINOR_PROFILE,
        };
        let mut chroma = [0.0_f32; 12];
        for (degree, value) in profile.into_iter().enumerate() {
            chroma[(root + degree) % 12] = value / 6.35;
        }
        chroma
    }

    fn key_test_bin(
        start_secs: f64,
        chroma: [f32; 12],
        transient: f32,
        transient_density: f32,
    ) -> WaveformBin {
        WaveformBin {
            start_secs,
            duration_secs: 0.1,
            loudness_db: -12.0,
            transient,
            transient_density,
            key_confidence: 0.85,
            chroma,
            ..WaveformBin::default()
        }
    }

    #[test]
    fn keyboard_highlight_suppresses_flat_note_bed() {
        let notes = vec![0.45; 24];
        let targets = keyboard_highlight_targets(&notes, notes.len());

        assert!(targets.iter().all(|value| *value < 0.05));
    }

    #[test]
    fn keyboard_highlight_prefers_local_peaks_over_adjacent_spill() {
        let mut notes = vec![0.04; 24];
        notes[9] = 0.55;
        notes[10] = 0.90;
        notes[11] = 0.58;
        notes[14] = 0.70;

        let targets = keyboard_highlight_targets(&notes, notes.len());

        assert!(targets[10] > 0.75);
        assert!(targets[14] > 0.45);
        assert!(targets[9] < 0.10);
        assert!(targets[11] < 0.10);
    }

    #[test]
    fn keyboard_sustain_suppresses_broad_single_frame_transients() {
        let mut sustain = vec![0.0; 24];
        let mut transient = vec![0.0; 24];
        for idx in 5..19 {
            transient[idx] = 0.95;
        }

        let sustained = update_keyboard_sustain(&transient, &mut sustain);
        let targets = keyboard_highlight_targets(&sustained, sustained.len());

        assert!(targets.iter().all(|value| *value < 0.12));
    }

    #[test]
    fn keyboard_sustain_brightens_continuing_tones() {
        let mut sustain = vec![0.0; 24];
        let mut notes = vec![0.02; 24];
        notes[10] = 0.85;

        let mut targets = Vec::new();
        for _ in 0..8 {
            let sustained = update_keyboard_sustain(&notes, &mut sustain);
            targets = keyboard_highlight_targets(&sustained, sustained.len());
        }

        assert!(targets[10] > 0.65);
    }

    #[test]
    fn conventional_keyboard_centers_c_on_spectrum_axis() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 40.0));
        let midi_c7 = 96;

        let key = conventional_key_rect(rect, midi_c7, false).expect("C7 key should be visible");
        let axis_x = spectrum_axis_x(rect, midi_to_hz(midi_c7));

        assert!((key.center().x - axis_x).abs() < 0.01);
    }

    #[test]
    fn timeline_raster_bins_include_key_window_padding() {
        let bins: Vec<WaveformBin> = (0..20)
            .map(|idx| WaveformBin {
                start_secs: idx as f64,
                duration_secs: 1.0,
                ..WaveformBin::default()
            })
            .collect();

        let row_bins = timeline_bins_for_raster(&bins, 10.0, 5.0);

        assert!(row_bins.first().is_some_and(|bin| bin.start_secs <= 7.0));
        assert!(row_bins.last().is_some_and(|bin| bin.start_secs >= 18.0));
    }

    #[test]
    fn timeline_partial_invalidation_keeps_generation_stable() {
        let key = TimelineTextureCacheKey {
            width_px: 120,
            waveform_h_px: 24,
            gap_px: 2,
            metrics_h_px: 8,
            row_secs_millis: 5000,
            rows: 3,
            dark: true,
        };
        let mut cache = TimelineTextureCache::default();
        cache.ensure(key);
        let generation = cache.generation;

        cache.invalidate_time_range(5.2, 9.8, 5.0);

        assert_eq!(cache.generation, generation);
        assert_eq!(cache.row_versions, vec![0, 1, 0]);

        cache.invalidate_time_range(0.0, 5.1, 5.0);

        assert_eq!(cache.generation, generation);
        assert_eq!(cache.row_versions, vec![1, 2, 0]);

        cache.invalidate_all_rows();

        assert_eq!(cache.generation, generation);
        assert_eq!(cache.row_versions, vec![2, 3, 1]);
    }

    #[test]
    fn streaming_output_keeps_playing_during_buffer_underrun() {
        let shared = Arc::new(Mutex::new(PlayerShared {
            samples: Vec::new(),
            sample_rate: 48_000,
            duration_secs: 10.0,
            position_frames: 0,
            playing: true,
            eof: false,
        }));
        let mut out = vec![1.0; 8];

        fill_output(&mut out, 2, &shared);

        assert!(out.iter().all(|sample| *sample == 0.0));
        let state = shared.lock().unwrap();
        assert!(state.playing);
        assert_eq!(state.position_frames, 0);
    }

    #[test]
    fn streaming_output_stops_after_eof_underrun() {
        let shared = Arc::new(Mutex::new(PlayerShared {
            samples: Vec::new(),
            sample_rate: 48_000,
            duration_secs: 10.0,
            position_frames: 0,
            playing: true,
            eof: true,
        }));
        let mut out = vec![1.0; 8];

        fill_output(&mut out, 2, &shared);

        assert!(out.iter().all(|sample| *sample == 0.0));
        assert!(!shared.lock().unwrap().playing);
    }

    #[test]
    fn timeline_metric_values_stay_normalized() {
        let bin = WaveformBin {
            loudness_db: -18.0,
            band_energy: [0.62, 0.18, 0.72],
            transient: 0.55,
            transient_density: 0.58,
            brightness: 0.64,
            novelty: 0.48,
            bass_pitch_class: 9,
            bass_pitch_confidence: 0.82,
            key_pitch_class: 0,
            key_confidence: 0.70,
            vocal_score: 0.42,
            ..WaveformBin::default()
        };

        for kind in TIMELINE_METRIC_KINDS {
            let value = timeline_metric_value(kind, &bin);
            assert!((0.0..=1.0).contains(&value), "{kind:?}={value}");
        }
        assert!(timeline_metric_value(TimelineMetricKind::LoudnessBassRoot, &bin) > 0.7);
        assert!(timeline_metric_value(TimelineMetricKind::Key, &bin) > 0.4);
    }

    #[test]
    fn timeline_rhythm_segments_snap_to_strong_transients() {
        let mut bins = Vec::new();
        let mut visible = Vec::new();
        for idx in 0..20 {
            bins.push(WaveformBin {
                start_secs: idx as f64 * 0.01,
                duration_secs: 0.01,
                transient: if idx == 6 { 0.8 } else { 0.0 },
                band_energy: [0.6, 0.3, 0.1],
                ..WaveformBin::default()
            });
            visible.push(TimelineVisibleBin {
                index: idx,
                x0: idx as f32,
                x1: idx as f32 + 1.0,
            });
        }

        let segments = build_timeline_rhythm_segments(&bins, &visible);

        assert_eq!(segments[0].start, 0);
        assert_eq!(segments[0].end, 7);
        assert!(
            segments
                .iter()
                .take(segments.len().saturating_sub(1))
                .all(|segment| (5..=10).contains(&(segment.end - segment.start)))
        );
    }

    #[test]
    fn timeline_row_count_reserves_rows_from_probe_duration() {
        assert_eq!(timeline_row_count(0.0, 30.0), 1);
        assert_eq!(timeline_row_count(f64::NAN, 30.0), 1);
        assert_eq!(timeline_row_count(29.9, 30.0), 1);
        assert_eq!(timeline_row_count(30.0, 30.0), 1);
        assert_eq!(timeline_row_count(30.1, 30.0), 2);
        assert_eq!(timeline_row_count(121.0, 30.0), 5);
    }

    #[test]
    fn timeline_key_hint_uses_long_window_vote() {
        let mut bins = Vec::new();
        for idx in 0..100 {
            let first_key = idx < 55;
            let root = if first_key { 2 } else { 7 };
            bins.push(key_test_bin(
                idx as f64 * 0.1,
                profile_chroma(root, TimelineKeyMode::Major),
                0.05,
                0.05,
            ));
        }

        let first = key_hint_at_time(&bins, 2.0);
        let second = key_hint_at_time(&bins, 8.0);
        assert_eq!(first.pitch_class, 2);
        assert_eq!(first.mode, TimelineKeyMode::Major);
        assert_eq!(second.pitch_class, 7);
        assert_eq!(second.mode, TimelineKeyMode::Major);
    }

    #[test]
    fn timeline_key_hint_matches_major_and_minor_profiles() {
        let c_major = vote_key_hint(
            &(0..24)
                .map(|idx| {
                    key_test_bin(
                        idx as f64 * 0.1,
                        profile_chroma(0, TimelineKeyMode::Major),
                        0.04,
                        0.04,
                    )
                })
                .collect::<Vec<_>>(),
        );
        let a_minor = vote_key_hint(
            &(0..24)
                .map(|idx| {
                    key_test_bin(
                        idx as f64 * 0.1,
                        profile_chroma(9, TimelineKeyMode::Minor),
                        0.04,
                        0.04,
                    )
                })
                .collect::<Vec<_>>(),
        );

        assert_eq!(c_major.pitch_class, 0);
        assert_eq!(c_major.mode, TimelineKeyMode::Major);
        assert!(c_major.confidence > 0.4);
        assert_eq!(a_minor.pitch_class, 9);
        assert_eq!(a_minor.mode, TimelineKeyMode::Minor);
        assert!(a_minor.confidence > 0.4);
    }

    #[test]
    fn timeline_key_hint_downweights_transient_chroma() {
        let mut bins = Vec::new();
        for idx in 0..12 {
            bins.push(key_test_bin(
                idx as f64 * 0.1,
                profile_chroma(0, TimelineKeyMode::Major),
                0.04,
                0.04,
            ));
            bins.push(key_test_bin(
                idx as f64 * 0.1 + 0.05,
                profile_chroma(6, TimelineKeyMode::Major),
                1.0,
                1.0,
            ));
        }

        let hint = vote_key_hint(&bins);

        assert_eq!(hint.pitch_class, 0);
        assert_eq!(hint.mode, TimelineKeyMode::Major);
    }

    #[test]
    fn timeline_key_lane_keeps_low_confidence_hint_visible() {
        let bin = WaveformBin::default();
        let bass_hint = TimelinePitchHint::default();
        let key_hint = TimelineKeyHint {
            pitch_class: 0,
            mode: TimelineKeyMode::Major,
            confidence: 0.02,
        };

        let value =
            timeline_metric_display_value(TimelineMetricKind::Key, &bin, bass_hint, key_hint);

        assert!(value >= TIMELINE_KEY_DISPLAY_FLOOR);
    }

    #[test]
    fn timeline_bin_lookup_uses_time_span() {
        let bins = [
            WaveformBin {
                start_secs: 0.0,
                duration_secs: 0.5,
                ..WaveformBin::default()
            },
            WaveformBin {
                start_secs: 0.5,
                duration_secs: 0.5,
                rms: 0.7,
                ..WaveformBin::default()
            },
        ];

        let bin = timeline_bin_at_time(&bins, 0.75).expect("bin");
        assert_eq!(bin.rms, 0.7);
    }
}
