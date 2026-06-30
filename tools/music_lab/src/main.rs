use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use music_core::{
    AnalysisConfig, AudioStreamInfo, DecodedAudio, MusicBookmark, PlaybackSnapshot,
    SPECTRUM_NOTE_MAX_MIDI, SPECTRUM_NOTE_MIN_MIDI, SpectrumAnalysis, TimelineAnalysis,
    WaveformBin, analyze_stereo_timeline, resample_linear_stereo,
    spectrum_analysis_from_stereo_window,
};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::conv::IntoSample;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

const TIMELINE_WAVEFORM_H: f32 = 68.0;
const TIMELINE_LOUDNESS_H: f32 = 18.0;
const TIMELINE_INNER_GAP: f32 = 4.0;
const TIMELINE_ROW_GAP: f32 = 12.0;
const TIMELINE_TEXTURE_MAX_WIDTH: usize = 4096;
const TIMELINE_ROW_SECS_CHOICES: [f64; 8] = [5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0];
const SPECTRUM_BANDS: usize = 108;
const SPECTRUM_TRAIL_DECAY: f32 = 0.982;
const SPECTRUM_REFRESH_INTERVAL: Duration = Duration::from_millis(5);
const SPECTRUM_KEYBOARD_H: f32 = 34.0;
const SPECTRUM_PANEL_GAP: f32 = 8.0;
const SPECTRUM_VIEW_MIN_HZ: f32 = 40.0;
const SPECTRUM_VIEW_MAX_HZ: f32 = 18_000.0;
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
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = app_panel_bg();
    visuals.window_fill = app_panel_bg();
    visuals.extreme_bg_color = app_bg();
    visuals.faint_bg_color = egui::Color32::from_rgb(10, 12, 14);
    visuals.widgets.noninteractive.bg_fill = app_panel_bg();
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(20, 23, 26);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(34, 39, 44);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(45, 52, 58);
    visuals.selection.bg_fill = egui::Color32::from_rgb(40, 94, 150);
    visuals.override_text_color = Some(egui::Color32::from_rgb(222, 226, 230));
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
    analysis: TimelineAnalysis,
}

enum LoadMsg {
    Loaded(Box<LoadedTrack>),
    Failed(String),
}

struct SpectrumMsg {
    analysis: SpectrumAnalysis,
    compute_ms: f32,
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
}

struct TimelineRowTexture {
    texture: egui::TextureHandle,
    represented_bins: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TimelineTextureCacheKey {
    width_px: usize,
    waveform_h_px: usize,
    gap_px: usize,
    loudness_h_px: usize,
    row_secs_millis: u32,
    rows: usize,
    bins_len: usize,
    dark: bool,
}

impl TimelineTextureCache {
    fn clear(&mut self) {
        self.key = None;
        self.rows.clear();
    }

    fn ensure(&mut self, key: TimelineTextureCacheKey) {
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        self.rows.clear();
        self.rows.resize_with(key.rows, || None);
    }

    fn row_texture(
        &mut self,
        ctx: &egui::Context,
        track: &LoadedTrack,
        row: usize,
        row_secs: f64,
        key: TimelineTextureCacheKey,
    ) -> Option<(&egui::TextureHandle, usize, bool)> {
        self.ensure(key);
        if row >= self.rows.len() {
            return None;
        }
        let mut cache_miss = false;
        if self.rows[row].is_none() {
            let row_start = row as f64 * row_secs;
            let (image, represented_bins) = render_timeline_row_image(
                row_start,
                row_secs,
                &track.analysis.bins,
                key.width_px,
                key.waveform_h_px,
                key.gap_px,
                key.loudness_h_px,
                key.dark,
            );
            let texture = ctx.load_texture(
                format!(
                    "music_timeline_row_{row}_{}x{}_{}",
                    key.width_px,
                    key.height_px(),
                    key.dark as u8
                ),
                image,
                egui::TextureOptions::LINEAR,
            );
            self.rows[row] = Some(TimelineRowTexture {
                texture,
                represented_bins,
            });
            cache_miss = true;
        }
        let row = self.rows[row].as_ref()?;
        Some((&row.texture, row.represented_bins, cache_miss))
    }
}

impl TimelineTextureCacheKey {
    fn height_px(self) -> usize {
        self.waveform_h_px + self.gap_px + self.loudness_h_px
    }
}

#[derive(Default)]
struct MusicLabApp {
    track: Option<LoadedTrack>,
    load_rx: Option<mpsc::Receiver<LoadMsg>>,
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
    spectrum_note_trail: Vec<f32>,
    spectrum_rx: Option<mpsc::Receiver<SpectrumMsg>>,
    spectrum_pending: bool,
    last_spectrum_request: Option<Instant>,
    timeline_cache: TimelineTextureCache,
    timeline_row_secs: f64,
    frame_stats: FrameStats,
}

impl eframe::App for MusicLabApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let update_start = Instant::now();
        self.frame_stats.record_frame();

        let stage_start = Instant::now();
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
        let player = self.player.as_ref();
        let snap = self.player_snapshot();
        let row_secs = self.timeline_row_secs();
        let load_status = self.load_status.clone();
        let timeline_cache = &mut self.timeline_cache;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(app_bg()))
            .show(ctx, |ui| {
                if let Some(track) = track {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            timeline_stats =
                                draw_timeline(ui, track, player, snap, timeline_cache, row_secs);
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
                            .add_filter(
                                "Audio",
                                &["mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "alac"],
                            )
                            .pick_file()
                        {
                            self.start_load(path);
                        }
                    }

                    let can_play = self.track.is_some();
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
                    ui.label(format!(
                        "{} / {}",
                        format_time(snap.position_secs),
                        format_time(snap.duration_secs)
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
                    ui.separator();
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
                } else {
                    ui.label("Open an audio file to inspect it.");
                }
            });
    }

    fn draw_bottom_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("music_lab_bottom")
            .exact_height(190.0)
            .frame(app_panel_frame())
            .show(ctx, |ui| {
                if self.track.is_none() {
                    ui.centered_and_justified(|ui| ui.label("Spectrum analyzer placeholder"));
                    return;
                }
                draw_spectrum(
                    ui,
                    &self.spectrum_bands,
                    &mut self.spectrum_trail,
                    &mut self.spectrum_prev_bands,
                    &mut self.spectrum_onsets,
                    &self.spectrum_notes,
                    &mut self.spectrum_note_trail,
                );
            });
    }

    fn start_load(&mut self, path: PathBuf) {
        self.load_status = format!("Loading {}", path.display());
        self.player = None;
        self.track = None;
        self.spectrum_bands.clear();
        self.spectrum_trail.clear();
        self.spectrum_prev_bands.clear();
        self.spectrum_onsets.clear();
        self.spectrum_notes.clear();
        self.spectrum_note_trail.clear();
        self.spectrum_rx = None;
        self.spectrum_pending = false;
        self.last_spectrum_request = None;
        self.timeline_cache.clear();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("music-lab-load".into())
            .spawn(move || {
                let msg = match decode_audio_file(&path) {
                    Ok(decoded) => {
                        let analysis = analyze_stereo_timeline(
                            &decoded.stereo_samples,
                            decoded.info.sample_rate,
                            AnalysisConfig::default(),
                        );
                        LoadMsg::Loaded(Box::new(LoadedTrack {
                            path,
                            decoded: Arc::new(decoded),
                            analysis,
                        }))
                    }
                    Err(err) => LoadMsg::Failed(err),
                };
                let _ = tx.send(msg);
            })
            .ok();
        self.load_rx = Some(rx);
    }

    fn poll_loader(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.load_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(LoadMsg::Loaded(track)) => {
                self.load_status = "Loaded".to_string();
                let track = *track;
                match LabPlayer::new(Arc::clone(&track.decoded)) {
                    Ok(player) => self.player = Some(player),
                    Err(err) => self.load_status = format!("Loaded; playback disabled: {err}"),
                }
                self.track = Some(track);
                self.load_rx = None;
                ctx.request_repaint();
            }
            Ok(LoadMsg::Failed(err)) => {
                self.load_status = err;
                self.load_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.load_status = "Loader stopped".to_string();
                self.load_rx = None;
            }
        }
    }

    fn poll_spectrum_analyzer(&mut self, ctx: &egui::Context) {
        if let Some(rx) = self.spectrum_rx.as_ref() {
            match rx.try_recv() {
                Ok(msg) => {
                    self.frame_stats.record_spectrum_compute(msg.compute_ms);
                    self.spectrum_bands = msg.analysis.bands;
                    self.spectrum_notes = msg.analysis.notes;
                    self.spectrum_pending = false;
                    self.spectrum_rx = None;
                    ctx.request_repaint();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(16));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.spectrum_pending = false;
                    self.spectrum_rx = None;
                }
            }
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
        let Some(decoded) = self.track.as_ref().map(|track| Arc::clone(&track.decoded)) else {
            return;
        };
        let position_secs = self.player_snapshot().position_secs;
        let (tx, rx) = mpsc::channel();
        let repaint_ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("music-lab-spectrum".into())
            .spawn(move || {
                let compute_start = Instant::now();
                let analysis = spectrum_analysis_from_stereo_window(
                    &decoded.stereo_samples,
                    decoded.info.sample_rate,
                    position_secs,
                    SPECTRUM_BANDS,
                );
                let compute_ms = compute_start.elapsed().as_secs_f32() * 1000.0;
                let _ = tx.send(SpectrumMsg {
                    analysis,
                    compute_ms,
                });
                repaint_ctx.request_repaint();
            });
        if spawned.is_ok() {
            self.spectrum_rx = Some(rx);
            self.spectrum_pending = true;
            self.last_spectrum_request = Some(Instant::now());
            ctx.request_repaint_after(SPECTRUM_REFRESH_INTERVAL);
        }
    }

    fn player_snapshot(&self) -> PlaybackSnapshot {
        self.player
            .as_ref()
            .map(LabPlayer::snapshot)
            .unwrap_or_default()
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
            ui.label("Open an audio file to test the thirty-second-row music view.");
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

fn draw_timeline(
    ui: &mut egui::Ui,
    track: &LoadedTrack,
    player: Option<&LabPlayer>,
    snap: PlaybackSnapshot,
    cache: &mut TimelineTextureCache,
    row_secs: f64,
) -> TimelineDrawStats {
    let mut stats = TimelineDrawStats::default();
    let row_secs = row_secs.max(1.0);
    let rows = (track.decoded.info.duration_secs / row_secs)
        .ceil()
        .max(1.0) as usize;
    let row_gap = TIMELINE_ROW_GAP;
    let row_h = TIMELINE_WAVEFORM_H + TIMELINE_INNER_GAP + TIMELINE_LOUDNESS_H;
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
    let loudness_h_px = ((TIMELINE_LOUDNESS_H * ppp).round() as usize).max(1);
    let texture_key = TimelineTextureCacheKey {
        width_px,
        waveform_h_px,
        gap_px,
        loudness_h_px,
        row_secs_millis: (row_secs * 1000.0).round() as u32,
        rows,
        bins_len: track.analysis.bins.len(),
        dark: ui.visuals().dark_mode,
    };
    cache.ensure(texture_key);

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
        let row_start = row as f64 * row_secs;
        stats.visible_rows += 1;
        if let Some((texture, represented_bins, cache_miss)) =
            cache.row_texture(ui.ctx(), track, row, row_secs, texture_key)
        {
            painter.image(
                texture.id(),
                row_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            if cache_miss {
                stats.cache_misses += 1;
                stats.drawn_bins += represented_bins;
            }
        }
        painter.text(
            egui::pos2(rect.min.x + 8.0, row_rect.center().y),
            egui::Align2::LEFT_CENTER,
            format_time(row_start),
            egui::FontId::monospace(12.0),
            ui.visuals().text_color(),
        );
        draw_beat_grid(&painter, row_rect, row_start, row_secs, &track.analysis);
    }

    draw_playhead(
        &painter,
        graph_rect,
        snap.position_secs,
        row_secs,
        row_h,
        row_gap,
    );

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

fn render_timeline_row_image(
    row_start: f64,
    row_secs: f64,
    bins: &[WaveformBin],
    width: usize,
    waveform_h: usize,
    gap_h: usize,
    loudness_h: usize,
    dark: bool,
) -> (egui::ColorImage, usize) {
    let height = waveform_h + gap_h + loudness_h;
    let bg = if dark {
        app_soft_bg()
    } else {
        egui::Color32::from_rgb(238, 241, 243)
    };
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
    let loudness_top = waveform_h + gap_h;
    fill_rect_px(
        &mut pixels,
        width,
        height,
        0,
        loudness_top,
        width,
        height,
        bg,
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
    for bin in bins {
        let bin_end = bin.start_secs + bin.duration_secs;
        if bin_end < row_start || bin.start_secs > row_start + row_secs {
            continue;
        }
        drawn_bins += 1;
        let visible_start = bin.start_secs.max(row_start);
        let visible_end = bin_end.min(row_start + row_secs);

        let x0 = ((visible_start - row_start) / row_secs) as f32 * width as f32;
        let x1 = (((visible_end - row_start) / row_secs) as f32 * width as f32)
            .max(x0 + 1.0)
            .min(width as f32);
        let amp = (bin.peak.max(bin.rms * 2.0)).sqrt().clamp(0.025, 1.0);
        let outer_half_h = (waveform_h as f32 * 0.46 * amp).max(1.0);
        let core_scale = 0.42 + bin.rms.sqrt().clamp(0.0, 1.0) * 0.45;
        let core_half_h = (outer_half_h * core_scale).max(1.0).min(outer_half_h);
        draw_spectral_waveform_bin_pixels(
            &mut pixels,
            width,
            height,
            center_y,
            x0,
            x1,
            outer_half_h,
            core_half_h,
            bin.band_energy,
        );
        if bin.transient > TRANSIENT_ACCENT_MIN {
            let transient = ((bin.transient - TRANSIENT_ACCENT_MIN) / (1.0 - TRANSIENT_ACCENT_MIN))
                .sqrt()
                .clamp(0.0, 1.0);
            let accent_half_h = waveform_h as f32 * (0.12 + transient * 0.22);
            let accent_center = ((x0 + x1) * 0.5).clamp(0.0, width as f32);
            let accent_half_w = ((x1 - x0).max(1.5) * (0.30 + transient * 0.45)).min(6.0);
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

        let loudness = ((bin.loudness_db + 52.0) / 52.0).clamp(0.0, 1.0);
        let loudness_value_h =
            (loudness_h.saturating_sub(2) as f32) * loudness.powf(0.72).max(0.02);
        let loudness_x0 = ((visible_start - row_start) / row_secs) as f32 * width as f32;
        let loudness_x1 = (((visible_end - row_start) / row_secs) as f32 * width as f32)
            .max(loudness_x0 + 1.0)
            .min(width as f32);
        let loudness_bottom = height.saturating_sub(1) as f32;
        fill_rect_f32(
            &mut pixels,
            width,
            height,
            loudness_x0,
            loudness_bottom - loudness_value_h,
            loudness_x1,
            loudness_bottom,
            loudness_color(loudness),
        );
    }

    draw_rect_stroke_px(
        &mut pixels,
        width,
        height,
        0,
        0,
        width,
        waveform_h,
        egui::Color32::from_rgba_unmultiplied(70, 92, 116, 140),
    );
    draw_rect_stroke_px(
        &mut pixels,
        width,
        height,
        0,
        loudness_top,
        width,
        height,
        egui::Color32::from_rgba_unmultiplied(70, 92, 116, 90),
    );

    (egui::ColorImage::new([width, height], pixels), drawn_bins)
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
    note_trail: &mut Vec<f32>,
) {
    let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
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
        draw_pitch_keyboard(&painter, keyboard_rect, notes, note_trail);
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

    let band_w = plot.width() / bands.len() as f32;
    for (i, value) in bands.iter().enumerate() {
        let value = value.clamp(0.0, 1.0);
        let rise = (value - prev_bands[i]).max(0.0);
        onsets[i] = (onsets[i] * 0.86).max((rise * 2.8).clamp(0.0, 1.0));
        prev_bands[i] = prev_bands[i] * 0.25 + value * 0.75;
        trail[i] = (trail[i] * SPECTRUM_TRAIL_DECAY).max(value);
        let x0 = plot.left() + i as f32 * band_w + 0.25;
        let x1 = (plot.left() + (i + 1) as f32 * band_w - 0.25)
            .max(x0 + 0.75)
            .min(plot.right());
        let trail_h = (plot.height() - 3.0) * trail[i].max(0.015);
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x0, plot.bottom() - 2.0 - trail_h),
                egui::pos2(x1, plot.bottom() - 2.0),
            ),
            1.0,
            color_with_alpha(spectrum_color(i, bands.len(), trail[i]), 58),
        );
        let h = (plot.height() - 3.0) * value.max(0.015);
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x0, plot.bottom() - 2.0 - h),
                egui::pos2(x1, plot.bottom() - 2.0),
            ),
            1.0,
            spectrum_color(i, bands.len(), value),
        );
        let onset = onsets[i].clamp(0.0, 1.0);
        if onset > 0.025 {
            let cap_h = (2.0 + onset * 12.0).min(plot.height() * 0.18);
            let y = plot.bottom() - 2.0 - h;
            let accent = brighten_color(spectrum_color(i, bands.len(), value.max(onset)), 1.45);
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x0, (y - cap_h).max(plot.top())),
                    egui::pos2(x1, (y + 1.5).min(plot.bottom())),
                ),
                1.0,
                color_with_alpha(accent, (82.0 + onset * 150.0) as u8),
            );
        }
    }
    draw_pitch_keyboard(&painter, keyboard_rect, notes, note_trail);
}

fn draw_pitch_keyboard(
    painter: &egui::Painter,
    rect: egui::Rect,
    notes: &[f32],
    note_trail: &mut Vec<f32>,
) {
    painter.rect_filled(rect, 0.0, egui::Color32::BLACK);
    let note_count = (SPECTRUM_NOTE_MAX_MIDI - SPECTRUM_NOTE_MIN_MIDI + 1) as usize;
    if note_trail.len() != note_count {
        note_trail.clear();
        note_trail.resize(note_count, 0.0);
    }

    for midi in SPECTRUM_NOTE_MIN_MIDI..=SPECTRUM_NOTE_MAX_MIDI {
        if is_black_key(midi) {
            continue;
        }
        let idx = (midi - SPECTRUM_NOTE_MIN_MIDI) as usize;
        let value = notes.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        note_trail[idx] = (note_trail[idx] * 0.965).max(value);
        let (x0, x1) = note_axis_range(rect, midi);
        if x1 <= rect.left() || x0 >= rect.right() {
            continue;
        }
        let key_rect = egui::Rect::from_min_max(
            egui::pos2(x0.max(rect.left()) + 0.25, rect.top()),
            egui::pos2(x1.min(rect.right()) - 0.25, rect.bottom()),
        );
        let base = egui::Color32::from_rgb(218, 222, 224);
        let active = key_color(midi, note_trail[idx]);
        let fill = lerp_color(
            base,
            active,
            (0.15 + note_trail[idx] * 0.85).clamp(0.0, 1.0),
        );
        painter.rect_filled(key_rect, 0.0, fill);
        painter.rect_stroke(
            key_rect,
            0.0,
            egui::Stroke::new(0.75, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 120)),
            egui::StrokeKind::Inside,
        );
    }

    let black_h = rect.height() * 0.64;
    for midi in SPECTRUM_NOTE_MIN_MIDI..=SPECTRUM_NOTE_MAX_MIDI {
        if !is_black_key(midi) {
            continue;
        }
        let idx = (midi - SPECTRUM_NOTE_MIN_MIDI) as usize;
        let value = notes.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        note_trail[idx] = (note_trail[idx] * 0.965).max(value);
        let (x0, x1) = note_axis_range(rect, midi);
        if x1 <= rect.left() || x0 >= rect.right() {
            continue;
        }
        let key_rect = egui::Rect::from_min_max(
            egui::pos2(x0.max(rect.left()) + 0.35, rect.top()),
            egui::pos2(x1.min(rect.right()) - 0.35, rect.top() + black_h),
        );
        let base = egui::Color32::from_rgb(18, 20, 22);
        let active = key_color(midi, note_trail[idx]);
        let fill = lerp_color(base, active, (note_trail[idx] * 0.95).clamp(0.0, 1.0));
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

fn note_axis_range(rect: egui::Rect, midi: u8) -> (f32, f32) {
    let hz = midi_to_hz(midi);
    let half = 2.0_f32.powf(1.0 / 24.0);
    (
        spectrum_axis_x(rect, hz / half),
        spectrum_axis_x(rect, hz * half),
    )
}

fn spectrum_axis_x(rect: egui::Rect, hz: f32) -> f32 {
    let min = SPECTRUM_VIEW_MIN_HZ;
    let max = SPECTRUM_VIEW_MAX_HZ;
    let t = (hz.clamp(min, max).log2() - min.log2()) / (max.log2() - min.log2());
    rect.left() + t.clamp(0.0, 1.0) * rect.width()
}

fn midi_to_hz(midi: u8) -> f32 {
    440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0)
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

fn loudness_color(value: f32) -> egui::Color32 {
    let value = value.clamp(0.0, 1.0);
    let base = if value < 0.58 {
        lerp_color(
            egui::Color32::from_rgb(40, 190, 80),
            egui::Color32::from_rgb(215, 225, 54),
            value / 0.58,
        )
    } else {
        lerp_color(
            egui::Color32::from_rgb(215, 225, 54),
            egui::Color32::from_rgb(248, 72, 38),
            (value - 0.58) / 0.42,
        )
    };
    color_with_alpha(base, 210)
}

fn spectrum_color(index: usize, total: usize, value: f32) -> egui::Color32 {
    let t = if total <= 1 {
        0.0
    } else {
        index as f32 / (total - 1) as f32
    };
    let base = if t < 0.20 {
        lerp_color(
            egui::Color32::from_rgb(244, 42, 24),
            egui::Color32::from_rgb(255, 154, 22),
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

fn decode_audio_file(path: &Path) -> Result<DecodedAudio, String> {
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
        .default_track()
        .ok_or_else(|| "no default audio track".to_string())?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("decoder: {e}"))?;

    let mut stereo_samples = Vec::new();
    let mut stream_info = AudioStreamInfo::default();
    loop {
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
        for frame in samples.chunks(channels) {
            let left: f32 = frame.first().copied().unwrap_or(0.0).into_sample();
            let right: f32 = frame.get(1).copied().unwrap_or(left).into_sample();
            stereo_samples.push(left.clamp(-1.0, 1.0));
            stereo_samples.push(right.clamp(-1.0, 1.0));
        }
    }
    stream_info.duration_secs =
        stereo_samples.len() as f64 / 2.0 / stream_info.sample_rate.max(1) as f64;
    if stereo_samples.is_empty() {
        return Err("no decoded samples".to_string());
    }
    Ok(DecodedAudio {
        info: stream_info,
        stereo_samples,
    })
}

struct LabPlayer {
    _stream: cpal::Stream,
    shared: Arc<Mutex<PlayerShared>>,
}

struct PlayerShared {
    samples: Arc<Vec<f32>>,
    sample_rate: u32,
    duration_secs: f64,
    position_frames: usize,
    playing: bool,
}

impl LabPlayer {
    fn new(decoded: Arc<DecodedAudio>) -> Result<Self, String> {
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
        let samples = if decoded.info.sample_rate == output_rate {
            decoded.stereo_samples.clone()
        } else {
            resample_linear_stereo(
                &decoded.stereo_samples,
                decoded.info.sample_rate,
                output_rate,
            )
        };
        let samples = Arc::new(samples);
        let shared = Arc::new(Mutex::new(PlayerShared {
            samples,
            sample_rate: output_rate,
            duration_secs: decoded.info.duration_secs,
            position_frames: 0,
            playing: false,
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
        Ok(Self {
            _stream: stream,
            shared,
        })
    }

    fn set_playing(&self, playing: bool) {
        if let Ok(mut s) = self.shared.lock() {
            s.playing = playing;
        }
    }

    fn seek_secs(&self, secs: f64) {
        if let Ok(mut s) = self.shared.lock() {
            let frame = (secs.max(0.0) * s.sample_rate as f64) as usize;
            s.position_frames = frame.min(s.samples.len() / 2);
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

fn fill_output(out: &mut [f32], out_channels: usize, shared: &Arc<Mutex<PlayerShared>>) {
    let Ok(mut state) = shared.lock() else {
        out.fill(0.0);
        return;
    };
    for frame in out.chunks_mut(out_channels) {
        let (l, r) = if state.playing && state.position_frames < state.samples.len() / 2 {
            let i = state.position_frames * 2;
            state.position_frames += 1;
            (state.samples[i], state.samples[i + 1])
        } else {
            state.playing = false;
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
