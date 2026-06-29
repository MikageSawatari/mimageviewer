use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use music_core::{
    AnalysisConfig, AudioStreamInfo, DecodedAudio, MusicBookmark, PlaybackSnapshot,
    TimelineAnalysis, WaveformBin, analyze_stereo_timeline, resample_linear_stereo,
    spectrum_bands_from_stereo_window,
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
const SPECTRUM_BANDS: usize = 108;
const SPECTRUM_TRAIL_DECAY: f32 = 0.982;
const SPECTRUM_REFRESH_INTERVAL: Duration = Duration::from_millis(75);
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
        Box::new(|_| Ok(Box::<MusicLabApp>::default())),
    )
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
    bands: Vec<f32>,
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
    spectrum_rx: Option<mpsc::Receiver<SpectrumMsg>>,
    spectrum_pending: bool,
    last_spectrum_request: Option<Instant>,
}

impl eframe::App for MusicLabApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_loader(ctx);
        self.poll_spectrum_analyzer(ctx);
        self.draw_top_bar(ctx);
        self.draw_left_panel(ctx);
        self.draw_right_panel(ctx);
        self.draw_bottom_bar(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(track) = &self.track {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        draw_timeline(ui, track, self.player.as_ref(), self.player_snapshot());
                    });
            } else {
                draw_empty_state(ui, &self.load_status);
            }
        });

        if self.player.as_ref().is_some_and(|p| p.snapshot().playing) {
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        }
    }
}

impl MusicLabApp {
    fn draw_top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("music_lab_top")
            .exact_height(48.0)
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
                } else {
                    ui.label("Open an audio file to inspect it.");
                }
            });
    }

    fn draw_bottom_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("music_lab_bottom")
            .exact_height(152.0)
            .show(ctx, |ui| {
                if self.track.is_none() {
                    ui.centered_and_justified(|ui| ui.label("Spectrum analyzer placeholder"));
                    return;
                }
                draw_spectrum(ui, &self.spectrum_bands, &mut self.spectrum_trail);
            });
    }

    fn start_load(&mut self, path: PathBuf) {
        self.load_status = format!("Loading {}", path.display());
        self.player = None;
        self.track = None;
        self.spectrum_bands.clear();
        self.spectrum_trail.clear();
        self.spectrum_rx = None;
        self.spectrum_pending = false;
        self.last_spectrum_request = None;
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
                    self.spectrum_bands = msg.bands;
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
                let bands = spectrum_bands_from_stereo_window(
                    &decoded.stereo_samples,
                    decoded.info.sample_rate,
                    position_secs,
                    SPECTRUM_BANDS,
                );
                let _ = tx.send(SpectrumMsg { bands });
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

fn draw_timeline(
    ui: &mut egui::Ui,
    track: &LoadedTrack,
    player: Option<&LabPlayer>,
    snap: PlaybackSnapshot,
) {
    let row_secs = track.analysis.config.row_secs.max(1.0);
    let rows = (track.decoded.info.duration_secs / row_secs)
        .ceil()
        .max(1.0) as usize;
    let row_gap = TIMELINE_ROW_GAP;
    let row_h = TIMELINE_WAVEFORM_H + TIMELINE_INNER_GAP + TIMELINE_LOUDNESS_H;
    let content_h = 16.0 + rows as f32 * row_h + rows.saturating_sub(1) as f32 * row_gap;
    let available = egui::vec2(ui.available_width(), ui.available_height().max(content_h));
    let (rect, response) = ui.allocate_exact_size(available, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);

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
        let row_start = row as f64 * row_secs;
        draw_timeline_row(
            &painter,
            row_rect,
            row_start,
            row_secs,
            &track.analysis.bins,
            ui.visuals().dark_mode,
        );
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
}

fn draw_timeline_row(
    painter: &egui::Painter,
    rect: egui::Rect,
    row_start: f64,
    row_secs: f64,
    bins: &[WaveformBin],
    dark: bool,
) {
    let bg = if dark {
        egui::Color32::from_rgb(18, 22, 24)
    } else {
        egui::Color32::from_rgb(238, 241, 243)
    };
    let waveform_rect =
        egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), TIMELINE_WAVEFORM_H));
    let loudness_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), waveform_rect.bottom() + TIMELINE_INNER_GAP),
        egui::vec2(rect.width(), TIMELINE_LOUDNESS_H),
    );

    painter.rect_filled(waveform_rect, 0.0, egui::Color32::BLACK);
    painter.rect_filled(loudness_rect, 0.0, bg);
    painter.rect_stroke(
        waveform_rect,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(70, 92, 116, 140)),
        egui::StrokeKind::Inside,
    );
    painter.rect_stroke(
        loudness_rect,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(70, 92, 116, 90)),
        egui::StrokeKind::Inside,
    );

    let center_y = waveform_rect.center().y;
    painter.line_segment(
        [
            egui::pos2(waveform_rect.left(), center_y),
            egui::pos2(waveform_rect.right(), center_y),
        ],
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 28),
        ),
    );

    for bin in bins {
        let bin_end = bin.start_secs + bin.duration_secs;
        if bin_end < row_start || bin.start_secs > row_start + row_secs {
            continue;
        }
        let visible_start = bin.start_secs.max(row_start);
        let visible_end = bin_end.min(row_start + row_secs);

        let x0 = waveform_rect.left()
            + ((visible_start - row_start) / row_secs) as f32 * waveform_rect.width();
        let x1 = (waveform_rect.left()
            + ((visible_end - row_start) / row_secs) as f32 * waveform_rect.width())
        .max(x0 + 1.0)
        .min(waveform_rect.right());
        let amp = (bin.peak.max(bin.rms * 2.0)).sqrt().clamp(0.025, 1.0);
        let outer_half_h = (waveform_rect.height() * 0.46 * amp).max(1.0);
        let core_scale = 0.42 + bin.rms.sqrt().clamp(0.0, 1.0) * 0.45;
        let core_half_h = (outer_half_h * core_scale).max(1.0).min(outer_half_h);
        draw_spectral_waveform_bin(
            painter,
            waveform_rect,
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
            let accent_half_h = waveform_rect.height() * (0.12 + transient * 0.22);
            let accent_center =
                ((x0 + x1) * 0.5).clamp(waveform_rect.left(), waveform_rect.right());
            let accent_half_w = ((x1 - x0).max(1.5) * (0.30 + transient * 0.45)).min(3.0);
            let accent = transient_color(bin.transient_band, transient);
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(
                        (accent_center - accent_half_w).max(waveform_rect.left()),
                        center_y - accent_half_h,
                    ),
                    egui::pos2(
                        (accent_center + accent_half_w).min(waveform_rect.right()),
                        center_y + accent_half_h,
                    ),
                ),
                0.0,
                color_with_alpha(accent, (34.0 + transient * 76.0) as u8),
            );
            painter.line_segment(
                [
                    egui::pos2(accent_center, center_y - accent_half_h),
                    egui::pos2(accent_center, center_y + accent_half_h),
                ],
                egui::Stroke::new(
                    1.0,
                    color_with_alpha(
                        brighten_color(accent, 1.18),
                        (54.0 + transient * 96.0) as u8,
                    ),
                ),
            );
        }

        let loudness = ((bin.loudness_db + 52.0) / 52.0).clamp(0.0, 1.0);
        let loudness_h = (loudness_rect.height() - 2.0) * loudness.powf(0.72).max(0.02);
        let loudness_x0 = loudness_rect.left()
            + ((visible_start - row_start) / row_secs) as f32 * loudness_rect.width();
        let loudness_x1 = (loudness_rect.left()
            + ((visible_end - row_start) / row_secs) as f32 * loudness_rect.width())
        .max(loudness_x0 + 1.0)
        .min(loudness_rect.right());
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(loudness_x0, loudness_rect.bottom() - 1.0 - loudness_h),
                egui::pos2(loudness_x1, loudness_rect.bottom() - 1.0),
            ),
            0.0,
            loudness_color(loudness),
        );
    }
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

fn draw_spectrum(ui: &mut egui::Ui, bands: &[f32], trail: &mut Vec<f32>) {
    let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);

    let plot = rect.shrink2(egui::vec2(18.0, 12.0));
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
        return;
    }
    if trail.len() != bands.len() {
        trail.clear();
        trail.resize(bands.len(), 0.0);
    }

    let band_w = plot.width() / bands.len() as f32;
    for (i, value) in bands.iter().enumerate() {
        let value = value.clamp(0.0, 1.0);
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
    }
}

fn draw_spectral_waveform_bin(
    painter: &egui::Painter,
    waveform_rect: egui::Rect,
    center_y: f32,
    x0: f32,
    x1: f32,
    outer_half_h: f32,
    core_half_h: f32,
    band: [f32; 3],
) {
    let weights = spectral_weights(band);
    let x0 = (x0 - 0.35).max(waveform_rect.left());
    let x1 = (x1 + 0.35).min(waveform_rect.right()).max(x0 + 0.75);
    let outer_bg = egui::Rect::from_min_max(
        egui::pos2(x0, center_y - outer_half_h),
        egui::pos2(x1, center_y + outer_half_h),
    );
    painter.rect_filled(
        outer_bg,
        0.0,
        egui::Color32::from_rgba_unmultiplied(126, 104, 62, 52),
    );

    draw_spectral_half(painter, x0, x1, center_y, -1.0, outer_half_h, weights, 88);
    draw_spectral_half(painter, x0, x1, center_y, 1.0, outer_half_h, weights, 88);

    draw_spectral_half(painter, x0, x1, center_y, -1.0, core_half_h, weights, 218);
    draw_spectral_half(painter, x0, x1, center_y, 1.0, core_half_h, weights, 218);
}

fn draw_spectral_half(
    painter: &egui::Painter,
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
        let rect = egui::Rect::from_min_max(
            egui::pos2(x0, cursor.min(next)),
            egui::pos2(x1, cursor.max(next)),
        );
        painter.rect_filled(rect, 0.0, color_with_alpha(colors[idx], alpha));
        cursor = next;
    }
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
