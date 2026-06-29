use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use music_core::{
    AnalysisConfig, AudioStreamInfo, DecodedAudio, MusicBookmark, PlaybackSnapshot,
    TimelineAnalysis, WaveformBin, analyze_stereo_timeline, resample_linear_stereo,
};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::conv::IntoSample;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

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

#[derive(Default)]
struct MusicLabApp {
    track: Option<LoadedTrack>,
    load_rx: Option<mpsc::Receiver<LoadMsg>>,
    load_status: String,
    player: Option<LabPlayer>,
    bookmarks: Vec<MusicBookmark>,
    next_bookmark_id: u64,
    selected_bookmark: Option<u64>,
}

impl eframe::App for MusicLabApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_loader(ctx);
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
                        ui.label(format!(
                            "BPM: {:.1} ({:?}, {:.0}%)",
                            bpm,
                            track.analysis.beat_grid.status,
                            track.analysis.beat_grid.confidence * 100.0
                        ));
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
            .exact_height(96.0)
            .show(ctx, |ui| {
                let Some(track) = &self.track else {
                    ui.centered_and_justified(|ui| ui.label("Spectrum analyzer placeholder"));
                    return;
                };
                draw_spectrum(ui, track, self.player_snapshot().position_secs);
            });
    }

    fn start_load(&mut self, path: PathBuf) {
        self.load_status = format!("Loading {}", path.display());
        self.player = None;
        self.track = None;
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
            ui.label("Open an audio file to test the one-minute-row music view.");
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
    let row_gap = 6.0;
    let row_h = 52.0;
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

    for row in 0..rows {
        let row_top = graph_rect.min.y + row as f32 * (row_h + row_gap);
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(graph_rect.min.x, row_top),
            egui::vec2(graph_rect.width(), row_h),
        );
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
    painter.rect_filled(rect, 4.0, bg);
    let center_y = rect.center().y;
    for bin in bins {
        if bin.start_secs + bin.duration_secs < row_start || bin.start_secs > row_start + row_secs {
            continue;
        }
        let x = rect.left()
            + (((bin.start_secs - row_start) / row_secs) as f32).clamp(0.0, 1.0) * rect.width();
        let amp = (bin.peak.max(bin.rms * 1.8)).sqrt().clamp(0.02, 1.0);
        let half_h = rect.height() * 0.46 * amp;
        painter.line_segment(
            [
                egui::pos2(x, center_y - half_h),
                egui::pos2(x, center_y + half_h),
            ],
            egui::Stroke::new(1.0, band_color(bin.band_energy)),
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
    let row_end = row_start + row_secs;
    for beat in &analysis.beat_grid.beats {
        if beat.time_secs < row_start || beat.time_secs > row_end {
            continue;
        }
        let x = rect.left() + ((beat.time_secs - row_start) / row_secs) as f32 * rect.width();
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(
                0.5,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 75),
            ),
        );
    }
    for bar in &analysis.beat_grid.bars {
        if bar.time_secs < row_start || bar.time_secs > row_end {
            continue;
        }
        let x = rect.left() + ((bar.time_secs - row_start) / row_secs) as f32 * rect.width();
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(
                1.5,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 160),
            ),
        );
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

fn draw_spectrum(ui: &mut egui::Ui, track: &LoadedTrack, position_secs: f64) {
    let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
    let bin = track
        .analysis
        .bins
        .iter()
        .min_by(|a, b| {
            (a.start_secs - position_secs)
                .abs()
                .total_cmp(&(b.start_secs - position_secs).abs())
        })
        .copied()
        .unwrap_or_default();
    let labels = ["LOW", "MID", "HIGH"];
    for (i, value) in bin.band_energy.iter().enumerate() {
        let x0 = rect.left() + 18.0 + i as f32 * (rect.width() - 36.0) / 3.0;
        let w = (rect.width() - 56.0) / 3.0;
        let h = (rect.height() - 30.0) * value.sqrt().clamp(0.02, 1.0);
        let bar =
            egui::Rect::from_min_size(egui::pos2(x0, rect.bottom() - h - 18.0), egui::vec2(w, h));
        painter.rect_filled(
            bar,
            4.0,
            band_color(match i {
                0 => [1.0, 0.0, 0.0],
                1 => [0.0, 1.0, 0.0],
                _ => [0.0, 0.0, 1.0],
            }),
        );
        painter.text(
            egui::pos2(x0, rect.bottom() - 14.0),
            egui::Align2::LEFT_CENTER,
            labels[i],
            egui::FontId::monospace(11.0),
            ui.visuals().text_color(),
        );
    }
}

fn band_color(band: [f32; 3]) -> egui::Color32 {
    let low = band[0].sqrt().clamp(0.0, 1.0);
    let mid = band[1].sqrt().clamp(0.0, 1.0);
    let high = band[2].sqrt().clamp(0.0, 1.0);
    egui::Color32::from_rgb(
        (230.0 * low + 70.0 * mid) as u8,
        (80.0 * low + 210.0 * mid + 150.0 * high) as u8,
        (60.0 * mid + 240.0 * high) as u8,
    )
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
