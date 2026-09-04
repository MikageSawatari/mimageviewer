use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;
use musicvideo_core::{
    Animatable, Assets, Fill, Frame, Glow, GradientStop, GradientStops, ImageAsset, ImageId, Layer,
    LayerKind, Project, RenderReport, Rgba8, Smoothing, SpectrumDirection, SpectrumFrame,
    SpectrumLayer, SpectrumShape, SpectrumSource, UnresolvedAsset, evaluate, render_frame,
};

use crate::decode::{AudioData, decode_audio};
use crate::export::{ExportOutcome, export_png_sequence, frame_count};
use crate::project::{BACKGROUND_ID, DEFAULT_DURATION_SECS, default_project, spectrum_band_count};

type AudioLoadResult = Result<(PathBuf, Arc<AudioData>), String>;
type ImageLoadResult = Result<(PathBuf, ImageAsset), String>;

struct AudioPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<AudioLoadResult>,
}

struct ImagePending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<ImageLoadResult>,
}

struct PreviewResult {
    revision: u64,
    image: egui::ColorImage,
    render_ms: f64,
    report: RenderReport,
}

struct PreviewPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<PreviewResult, String>>,
}

struct ExportPending {
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    total_frames: usize,
    rx: mpsc::Receiver<Result<ExportOutcome, String>>,
}

pub struct MusicVideoLabApp {
    assets: Arc<Assets>,
    project: Project,
    position_secs: f64,
    audio: Option<Arc<AudioData>>,
    audio_path: Option<PathBuf>,
    image_path: Option<PathBuf>,
    status: String,
    audio_pending: Option<AudioPending>,
    image_pending: Option<ImagePending>,
    preview_pending: Option<PreviewPending>,
    preview_dirty: bool,
    preview_revision: u64,
    preview_texture: Option<egui::TextureHandle>,
    preview_render_ms: f64,
    preview_report: RenderReport,
    export_pending: Option<ExportPending>,
    last_export: Option<ExportOutcome>,
    selected_layer: usize,
}

impl MusicVideoLabApp {
    pub fn new(assets: Arc<Assets>) -> Self {
        let status = if assets.fonts.is_empty() {
            "日本語フォントが見つからないため、時間表示は描画されません".to_owned()
        } else {
            "音声と背景画像を選んでください".to_owned()
        };
        Self {
            assets,
            project: default_project(DEFAULT_DURATION_SECS),
            position_secs: 0.0,
            audio: None,
            audio_path: None,
            image_path: None,
            status,
            audio_pending: None,
            image_pending: None,
            preview_pending: None,
            preview_dirty: true,
            preview_revision: 1,
            preview_texture: None,
            preview_render_ms: 0.0,
            preview_report: RenderReport::default(),
            export_pending: None,
            last_export: None,
            selected_layer: 1,
        }
    }

    fn mark_preview_dirty(&mut self) {
        self.preview_revision = self.preview_revision.wrapping_add(1);
        self.preview_dirty = true;
        if let Some(pending) = &self.preview_pending {
            pending.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn start_audio_load(&mut self, path: PathBuf, ctx: &egui::Context) {
        if let Some(pending) = self.audio_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let worker_path = path.clone();
        let worker_ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-audio-load".to_owned())
            .spawn(move || {
                let result = decode_audio(&worker_path, &worker_cancel)
                    .map(Arc::new)
                    .map(|audio| (worker_path, audio));
                if !worker_cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(result);
                    worker_ctx.request_repaint();
                }
            });
        match spawned {
            Ok(_) => {
                self.status = format!("音声を読み込んでいます: {}", path.display());
                self.audio_pending = Some(AudioPending { cancel, rx });
            }
            Err(error) => self.status = format!("音声読み込みワーカーを開始できません: {error}"),
        }
    }

    fn start_image_load(&mut self, path: PathBuf, ctx: &egui::Context) {
        if let Some(pending) = self.image_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let worker_path = path.clone();
        let worker_ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-image-load".to_owned())
            .spawn(move || {
                let result = image::open(&worker_path)
                    .map_err(|error| format!("背景画像を読み込めません: {error}"))
                    .and_then(|image| {
                        let image = image.to_rgba8();
                        ImageAsset::new(image.width(), image.height(), image.into_raw())
                            .map_err(|error| format!("背景画像を保持できません: {error}"))
                    })
                    .map(|image| (worker_path, image));
                if !worker_cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(result);
                    worker_ctx.request_repaint();
                }
            });
        match spawned {
            Ok(_) => {
                self.status = format!("背景画像を読み込んでいます: {}", path.display());
                self.image_pending = Some(ImagePending { cancel, rx });
            }
            Err(error) => self.status = format!("画像読み込みワーカーを開始できません: {error}"),
        }
    }

    fn poll_loads(&mut self) {
        let audio_result = self
            .audio_pending
            .as_ref()
            .map(|pending| pending.rx.try_recv());
        match audio_result {
            Some(Ok(Ok((path, audio)))) => {
                self.audio_pending = None;
                self.project.duration_secs = audio.duration_secs;
                self.position_secs = self.position_secs.min(audio.duration_secs);
                self.audio_path = Some(path);
                self.audio = Some(audio);
                self.status = "音声を読み込みました".to_owned();
                self.mark_preview_dirty();
            }
            Some(Ok(Err(error))) => {
                self.audio_pending = None;
                self.status = error;
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.audio_pending = None;
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }

        let image_result = self
            .image_pending
            .as_ref()
            .map(|pending| pending.rx.try_recv());
        match image_result {
            Some(Ok(Ok((path, image)))) => {
                self.image_pending = None;
                self.assets.insert_image(ImageId::new(BACKGROUND_ID), image);
                self.image_path = Some(path);
                self.status = "背景画像を読み込みました".to_owned();
                self.mark_preview_dirty();
            }
            Some(Ok(Err(error))) => {
                self.image_pending = None;
                self.status = error;
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.image_pending = None;
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
    }

    fn start_preview_if_needed(&mut self, ctx: &egui::Context) {
        if !self.preview_dirty || self.preview_pending.is_some() {
            return;
        }
        self.preview_dirty = false;
        let revision = self.preview_revision;
        let project = self.project.clone();
        let position_secs = self.position_secs;
        let audio = self.audio.clone();
        let assets = Arc::clone(&self.assets);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-preview".to_owned())
            .spawn(move || {
                if worker_cancel.load(Ordering::Relaxed) {
                    return;
                }
                let band_count = spectrum_band_count(&project);
                let bands = match audio {
                    Some(audio) => SpectrumSource::new(
                        audio.sample_rate,
                        band_count,
                        Smoothing::Average { factor: 0.7 },
                    )
                    .map(|mut source| source.frame_at(&audio.stereo_pcm, position_secs).to_vec())
                    .map_err(|error| format!("スペクトラム解析に失敗しました: {error}")),
                    None => Ok(vec![0.0; band_count]),
                };
                let result = bands.and_then(|bands| {
                    if worker_cancel.load(Ordering::Relaxed) {
                        return Err("プレビューをキャンセルしました".to_owned());
                    }
                    let scene = evaluate(&project, position_secs, &SpectrumFrame::new(bands));
                    let mut frame = Frame::new(project.output.width, project.output.height)
                        .map_err(|error| format!("プレビューを確保できません: {error}"))?;
                    let started = Instant::now();
                    let report = render_frame(&scene, &assets, &mut frame)
                        .map_err(|error| format!("プレビュー合成に失敗しました: {error}"))?;
                    let render_ms = started.elapsed().as_secs_f64() * 1000.0;
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [frame.width as usize, frame.height as usize],
                        &frame.rgba,
                    );
                    Ok(PreviewResult {
                        revision,
                        image,
                        render_ms,
                        report,
                    })
                });
                if !worker_cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(result);
                    worker_ctx.request_repaint();
                }
            });
        match spawned {
            Ok(_) => self.preview_pending = Some(PreviewPending { cancel, rx }),
            Err(error) => {
                self.preview_dirty = true;
                self.status = format!("プレビューワーカーを開始できません: {error}");
            }
        }
    }

    fn poll_preview(&mut self, ctx: &egui::Context) {
        let result = self
            .preview_pending
            .as_ref()
            .map(|pending| pending.rx.try_recv());
        match result {
            Some(Ok(Ok(result))) => {
                self.preview_pending = None;
                if result.revision == self.preview_revision {
                    self.preview_render_ms = result.render_ms;
                    self.preview_report = result.report;
                    if let Some(texture) = &mut self.preview_texture {
                        texture.set(result.image, egui::TextureOptions::LINEAR);
                    } else {
                        self.preview_texture = Some(ctx.load_texture(
                            "musicvideo-preview",
                            result.image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                } else {
                    self.preview_dirty = true;
                }
            }
            Some(Ok(Err(error))) => {
                self.preview_pending = None;
                if !self.preview_dirty {
                    self.status = error;
                }
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.preview_pending = None;
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
    }

    fn start_export(&mut self, output_root: PathBuf, ctx: &egui::Context) {
        let Some(audio) = self.audio.clone() else {
            self.status = "先に音声を読み込んでください".to_owned();
            return;
        };
        let project = self.project.clone();
        let total_frames = match frame_count(&project) {
            Ok(total) => total,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        let assets = Arc::clone(&self.assets);
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicUsize::new(0));
        let worker_cancel = Arc::clone(&cancel);
        let worker_progress = Arc::clone(&progress);
        let worker_ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-png-export".to_owned())
            .spawn(move || {
                let notify_ctx = worker_ctx.clone();
                let result = export_png_sequence(
                    project,
                    assets,
                    audio,
                    &output_root,
                    &worker_cancel,
                    &worker_progress,
                    || notify_ctx.request_repaint(),
                );
                let _ = tx.send(result);
                worker_ctx.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.last_export = None;
                self.status = "連番 PNG を書き出しています".to_owned();
                self.export_pending = Some(ExportPending {
                    cancel,
                    progress,
                    total_frames,
                    rx,
                });
            }
            Err(error) => self.status = format!("書き出しワーカーを開始できません: {error}"),
        }
    }

    fn poll_export(&mut self) {
        let result = self
            .export_pending
            .as_ref()
            .map(|pending| pending.rx.try_recv());
        match result {
            Some(Ok(Ok(outcome))) => {
                self.export_pending = None;
                self.status = match &outcome {
                    ExportOutcome::Completed { output_dir, .. } => {
                        format!("連番 PNG を書き出しました: {}", output_dir.display())
                    }
                    ExportOutcome::Cancelled {
                        output_dir,
                        completed_frames,
                    } => format!(
                        "書き出しをキャンセルしました ({completed_frames} フレーム): {}",
                        output_dir.display()
                    ),
                };
                self.last_export = Some(outcome);
            }
            Some(Ok(Err(error))) => {
                self.export_pending = None;
                self.status = error;
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.export_pending = None;
                self.status = "書き出しワーカーが終了しました".to_owned();
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
    }
}

impl eframe::App for MusicVideoLabApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_loads();
        self.poll_preview(ctx);
        self.poll_export();

        egui::TopBottomPanel::top("musicvideo-controls").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.audio_pending.is_none(),
                        egui::Button::new("音声を開く"),
                    )
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter(
                            "音声",
                            &["mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "alac"],
                        )
                        .pick_file()
                {
                    self.start_audio_load(path, ctx);
                }
                if ui
                    .add_enabled(
                        self.image_pending.is_none(),
                        egui::Button::new("背景画像を開く"),
                    )
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("画像", &["png", "jpg", "jpeg", "webp", "bmp"])
                        .pick_file()
                {
                    self.start_image_load(path, ctx);
                }

                let ready_to_export = self.audio.is_some()
                    && self.assets.has_image(&ImageId::new(BACKGROUND_ID))
                    && self.export_pending.is_none();
                if ui
                    .add_enabled(ready_to_export, egui::Button::new("連番 PNG を書き出す"))
                    .clicked()
                    && let Some(folder) = rfd::FileDialog::new().pick_folder()
                {
                    self.start_export(folder, ctx);
                }
                if let Some(pending) = &self.export_pending
                    && ui.button("書き出しをキャンセル").clicked()
                {
                    pending.cancel.store(true, Ordering::Relaxed);
                    self.status = "キャンセルを要求しました".to_owned();
                }
            });

            ui.separator();
            ui.label(&self.status);
            if !self.preview_report.unresolved_assets.is_empty() {
                ui.label(format!(
                    "未解決素材: {}",
                    unresolved_assets_text(&self.preview_report)
                ));
            }
            if let Some(path) = &self.audio_path {
                ui.label(format!("音声: {}", path.display()));
            }
            if let Some(path) = &self.image_path {
                ui.label(format!("背景: {}", path.display()));
            }
            if let Some(pending) = &self.export_pending {
                let completed = pending.progress.load(Ordering::Relaxed);
                let fraction = completed as f32 / pending.total_frames.max(1) as f32;
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .show_percentage()
                        .text(format!("{completed} / {} フレーム", pending.total_frames)),
                );
            }
            if let Some(outcome) = &self.last_export {
                match outcome {
                    ExportOutcome::Completed { stats, .. } => {
                        ui.label(format!(
                            "解析: 平均 {:.3} ms / p95 {:.3} ms",
                            stats.analysis.mean_ms, stats.analysis.p95_ms
                        ));
                        ui.label(format!(
                            "合成: 平均 {:.3} ms / p95 {:.3} ms",
                            stats.composition.mean_ms, stats.composition.p95_ms
                        ));
                        ui.label(format!(
                            "合計: 平均 {:.3} ms / p95 {:.3} ms / 実時間比 {:.3}x",
                            stats.total.mean_ms, stats.total.p95_ms, stats.realtime_ratio
                        ));
                    }
                    ExportOutcome::Cancelled {
                        completed_frames, ..
                    } => {
                        ui.label(format!("完了済み: {completed_frames} フレーム"));
                    }
                }
            }

            ui.separator();
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} / {}",
                    format_clock(self.position_secs),
                    format_clock(self.project.duration_secs)
                ));
                let changed = ui
                    .add(
                        egui::Slider::new(
                            &mut self.position_secs,
                            0.0..=self.project.duration_secs.max(0.001),
                        )
                        .text("時刻 (秒)"),
                    )
                    .changed();
                if changed {
                    self.mark_preview_dirty();
                }
                ui.label(format!("合成 {:.2} ms", self.preview_render_ms));
            });
        });

        let mut project_changed = false;
        egui::SidePanel::right("musicvideo-properties")
            .default_width(330.0)
            .show(ctx, |ui| {
                ui.heading("レイヤー");
                for (index, layer) in self.project.layers.iter().enumerate() {
                    if ui
                        .selectable_label(self.selected_layer == index, &layer.name)
                        .clicked()
                    {
                        self.selected_layer = index;
                    }
                }
                ui.separator();
                if let Some(layer) = self.project.layers.get_mut(self.selected_layer) {
                    project_changed = edit_layer_properties(ui, layer);
                }
            });
        if project_changed {
            self.mark_preview_dirty();
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                if let Some(texture) = &self.preview_texture {
                    let source = texture.size_vec2();
                    let available = ui.available_size();
                    let scale = (available.x / source.x)
                        .min(available.y / source.y)
                        .max(0.01);
                    let size = source * scale;
                    ui.centered_and_justified(|ui| {
                        ui.image((texture.id(), size));
                    });
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.spinner();
                        ui.label("プレビューを準備しています");
                    });
                }
            });

        self.start_preview_if_needed(ctx);
        if self.audio_pending.is_some()
            || self.image_pending.is_some()
            || self.preview_pending.is_some()
            || self.export_pending.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

fn edit_layer_properties(ui: &mut egui::Ui, layer: &mut Layer) -> bool {
    let mut changed = ui.text_edit_singleline(&mut layer.name).changed();
    if let Some(visible) = const_mut(&mut layer.visible) {
        changed |= ui.checkbox(visible, "表示する").changed();
    }
    if let Some(opacity) = const_mut(&mut layer.opacity) {
        changed |= ui
            .add(egui::Slider::new(opacity, 0.0..=1.0).text("不透明度"))
            .changed();
    }
    if let Some(rotation) = const_mut(&mut layer.rotation) {
        changed |= ui
            .add(
                egui::DragValue::new(rotation)
                    .range(-180.0..=180.0)
                    .prefix("回転 ")
                    .suffix("°"),
            )
            .changed();
    }
    if let Some(radius) = const_mut(&mut layer.corner_radius) {
        changed |= ui
            .add(
                egui::DragValue::new(radius)
                    .range(0.0..=1000.0)
                    .suffix(" px 角丸"),
            )
            .changed();
    }
    changed |= edit_glow(ui, &mut layer.glow);
    ui.separator();
    changed |= match &mut layer.kind {
        LayerKind::Solid { fill } => edit_fill(ui, "塗り", fill),
        LayerKind::Image { .. } => false,
        LayerKind::Spectrum(spectrum) => edit_spectrum(ui, spectrum),
        LayerKind::ProgressBar(progress) => {
            let mut changed = ui
                .add(
                    egui::DragValue::new(&mut progress.border_thickness_px)
                        .range(0.0..=100.0)
                        .suffix(" px 枠"),
                )
                .changed();
            changed |= edit_fill(ui, "進捗の塗り", &mut progress.fill);
            changed |= edit_fill(ui, "枠の塗り", &mut progress.border_fill);
            changed
        }
        LayerKind::TimeText(text) => edit_fill(ui, "文字の塗り", &mut text.fill),
    };
    changed
}

fn const_mut<T>(value: &mut Animatable<T>) -> Option<&mut T> {
    match value {
        Animatable::Const(value) => Some(value),
        Animatable::Track(_) => None,
    }
}

fn edit_glow(ui: &mut egui::Ui, value: &mut Animatable<Option<Glow>>) -> bool {
    let Some(glow) = const_mut(value) else {
        ui.label("グローのキーフレーム編集は P3 で対応します");
        return false;
    };
    let mut enabled = glow.is_some();
    let mut changed = ui.checkbox(&mut enabled, "グロー").changed();
    if enabled && glow.is_none() {
        *glow = Some(Glow {
            color: Rgba8::WHITE,
            radius_px: 12.0,
            spread_px: 0.0,
        });
        changed = true;
    } else if !enabled && glow.is_some() {
        *glow = None;
        changed = true;
    }
    if let Some(glow) = glow {
        changed |= edit_color(ui, "グロー色", &mut glow.color);
        changed |= ui
            .add(
                egui::DragValue::new(&mut glow.radius_px)
                    .range(0.0..=128.0)
                    .suffix(" px 半径"),
            )
            .changed();
        changed |= ui
            .add(
                egui::DragValue::new(&mut glow.spread_px)
                    .range(0.0..=128.0)
                    .suffix(" px 広がり"),
            )
            .changed();
    }
    changed
}

fn edit_color(ui: &mut egui::Ui, label: &str, color: &mut Rgba8) -> bool {
    let mut value = egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        changed = ui.color_edit_button_srgba(&mut value).changed();
    });
    if changed {
        *color = Rgba8::new(value.r(), value.g(), value.b(), value.a());
    }
    changed
}

fn edit_spectrum(ui: &mut egui::Ui, spectrum: &mut SpectrumLayer) -> bool {
    let mut changed = ui
        .add(
            egui::DragValue::new(&mut spectrum.band_count)
                .range(1..=128)
                .prefix("解析バンド "),
        )
        .changed();
    changed |= ui
        .add(
            egui::DragValue::new(&mut spectrum.point_count)
                .range(1..=4096)
                .prefix("描画点 "),
        )
        .changed();
    let mut radial = matches!(spectrum.shape, SpectrumShape::Radial { .. });
    let was_radial = radial;
    ui.horizontal(|ui| {
        ui.radio_value(&mut radial, false, "直線");
        ui.radio_value(&mut radial, true, "円形");
    });
    if radial != was_radial {
        spectrum.shape = if radial {
            SpectrumShape::Radial {
                radius_px: 380.0,
                thickness_px: 4.0,
                start_angle_deg: -90.0,
                sweep_deg: 360.0,
                direction: SpectrumDirection::Clockwise,
            }
        } else {
            SpectrumShape::Bars {
                gap_ratio: 0.3,
                min_height_px: 3.0,
                thickness_px: 12.0,
            }
        };
        changed = true;
    }
    match &mut spectrum.shape {
        SpectrumShape::Bars {
            gap_ratio,
            min_height_px,
            thickness_px,
        } => {
            changed |= ui
                .add(egui::Slider::new(gap_ratio, 0.0..=1.0).text("隙間"))
                .changed();
            changed |= drag_pixels(ui, "最小高さ", min_height_px, 0.0..=1000.0);
            changed |= drag_pixels(ui, "太さ", thickness_px, 0.5..=200.0);
        }
        SpectrumShape::Radial {
            radius_px,
            thickness_px,
            start_angle_deg,
            sweep_deg,
            direction,
        } => {
            changed |= drag_pixels(ui, "半径", radius_px, 0.0..=2000.0);
            changed |= drag_pixels(ui, "太さ", thickness_px, 0.5..=200.0);
            changed |= ui
                .add(egui::DragValue::new(start_angle_deg).suffix("° 開始"))
                .changed();
            changed |= ui
                .add(
                    egui::DragValue::new(sweep_deg)
                        .range(0.0..=360.0)
                        .suffix("° 範囲"),
                )
                .changed();
            ui.horizontal(|ui| {
                changed |= ui
                    .radio_value(direction, SpectrumDirection::Clockwise, "時計回り")
                    .changed();
                changed |= ui
                    .radio_value(direction, SpectrumDirection::CounterClockwise, "反時計回り")
                    .changed();
            });
        }
    }
    changed |= edit_fill(ui, "スペクトラムの塗り", &mut spectrum.fill);
    changed
}

fn drag_pixels(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    ui.add(
        egui::DragValue::new(value)
            .range(range)
            .prefix(format!("{label} "))
            .suffix(" px"),
    )
    .changed()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FillKind {
    Solid,
    Linear,
    Radial,
    Band,
}

fn edit_fill(ui: &mut egui::Ui, label: &str, value: &mut Animatable<Fill>) -> bool {
    let Some(fill) = const_mut(value) else {
        ui.label(format!("{label}: キーフレーム編集は P3 で対応します"));
        return false;
    };
    ui.collapsing(label, |ui| {
        let mut kind = match fill {
            Fill::Solid(_) => FillKind::Solid,
            Fill::LinearGradient { .. } => FillKind::Linear,
            Fill::RadialGradient { .. } => FillKind::Radial,
            Fill::BandGradient { .. } => FillKind::Band,
        };
        let previous = kind;
        egui::ComboBox::from_id_salt(("fill-kind", label))
            .selected_text(fill_kind_label(kind))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut kind, FillKind::Solid, "単色");
                ui.selectable_value(&mut kind, FillKind::Linear, "線形グラデーション");
                ui.selectable_value(&mut kind, FillKind::Radial, "放射グラデーション");
                ui.selectable_value(&mut kind, FillKind::Band, "バンドグラデーション");
            });
        let mut changed = kind != previous;
        if changed {
            *fill = default_fill(kind);
        }
        match fill {
            Fill::Solid(color) => changed |= edit_color(ui, "色", color),
            Fill::LinearGradient { stops, angle_deg } => {
                changed |= ui
                    .add(egui::DragValue::new(angle_deg).suffix("° 角度"))
                    .changed();
                changed |= edit_gradient_stops(ui, stops);
            }
            Fill::RadialGradient {
                stops,
                center,
                radius,
            } => {
                changed |= ui
                    .add(egui::Slider::new(&mut center.0, 0.0..=1.0).text("中心 X "))
                    .changed();
                changed |= ui
                    .add(egui::Slider::new(&mut center.1, 0.0..=1.0).text("中心 Y "))
                    .changed();
                changed |= ui
                    .add(egui::Slider::new(radius, 0.01..=2.0).text("半径"))
                    .changed();
                changed |= edit_gradient_stops(ui, stops);
            }
            Fill::BandGradient { stops } => changed |= edit_gradient_stops(ui, stops),
        }
        changed
    })
    .body_returned
    .unwrap_or(false)
}

fn fill_kind_label(kind: FillKind) -> &'static str {
    match kind {
        FillKind::Solid => "単色",
        FillKind::Linear => "線形",
        FillKind::Radial => "放射",
        FillKind::Band => "バンド",
    }
}

fn default_fill(kind: FillKind) -> Fill {
    match kind {
        FillKind::Solid => Fill::Solid(Rgba8::WHITE),
        FillKind::Linear => Fill::LinearGradient {
            stops: default_gradient_stops(),
            angle_deg: 0.0,
        },
        FillKind::Radial => Fill::RadialGradient {
            stops: default_gradient_stops(),
            center: (0.5, 0.5),
            radius: 0.5,
        },
        FillKind::Band => Fill::BandGradient {
            stops: default_gradient_stops(),
        },
    }
}

fn default_gradient_stops() -> GradientStops {
    GradientStops::new(vec![
        GradientStop {
            position: 0.0,
            color: Rgba8::new(120, 255, 35, 255),
        },
        GradientStop {
            position: 1.0,
            color: Rgba8::new(255, 136, 255, 255),
        },
    ])
    .expect("fixed gradient stops are valid")
}

fn edit_gradient_stops(ui: &mut egui::Ui, stops: &mut GradientStops) -> bool {
    let mut values = stops.to_vec();
    let mut changed = false;
    let mut remove = None;
    let can_remove = values.len() > 1;
    for (index, stop) in values.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            ui.horizontal(|ui| {
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut stop.position)
                            .range(0.0..=1.0)
                            .speed(0.01),
                    )
                    .changed();
                changed |= edit_color(ui, "色", &mut stop.color);
                if can_remove && ui.small_button("削除").clicked() {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        values.remove(index);
        changed = true;
    }
    if ui.small_button("ストップを追加").clicked() {
        values.push(GradientStop {
            position: 1.0,
            color: Rgba8::WHITE,
        });
        changed = true;
    }
    if changed {
        *stops = GradientStops::new(values).expect("UI restricts gradient stop positions");
    }
    changed
}

fn format_clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0).floor() as u64;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn unresolved_assets_text(report: &RenderReport) -> String {
    report
        .unresolved_assets
        .iter()
        .map(|asset| match asset {
            UnresolvedAsset::Image(id) => format!("画像 {}", id.0),
            UnresolvedAsset::Font(key) => format!("フォント {key}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

impl Drop for MusicVideoLabApp {
    fn drop(&mut self) {
        if let Some(pending) = &self.audio_pending {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(pending) = &self.image_pending {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(pending) = &self.preview_pending {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(pending) = &self.export_pending {
            pending.cancel.store(true, Ordering::Relaxed);
        }
    }
}
