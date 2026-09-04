use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;
use musicvideo_core::{
    Assets, Frame, ImageAsset, ImageId, Project, Smoothing, SpectrumFrame, SpectrumSource,
    evaluate, render_frame,
};

use crate::decode::{AudioData, decode_audio};
use crate::export::{ExportOutcome, export_png_sequence, frame_count};
use crate::project::{BACKGROUND_ID, DEFAULT_DURATION_SECS, SPECTRUM_BANDS, default_project};

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
    export_pending: Option<ExportPending>,
    last_export: Option<ExportOutcome>,
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
            export_pending: None,
            last_export: None,
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
                let bands = match audio {
                    Some(audio) => SpectrumSource::new(
                        audio.sample_rate,
                        SPECTRUM_BANDS,
                        Smoothing::Average { factor: 0.7 },
                    )
                    .map(|mut source| source.frame_at(&audio.stereo_pcm, position_secs).to_vec())
                    .map_err(|error| format!("スペクトラム解析に失敗しました: {error}")),
                    None => Ok(vec![0.0; SPECTRUM_BANDS]),
                };
                let result = bands.and_then(|bands| {
                    if worker_cancel.load(Ordering::Relaxed) {
                        return Err("プレビューをキャンセルしました".to_owned());
                    }
                    let scene = evaluate(&project, position_secs, &SpectrumFrame::new(bands));
                    let mut frame = Frame::new(project.output.width, project.output.height)
                        .map_err(|error| format!("プレビューを確保できません: {error}"))?;
                    let started = Instant::now();
                    render_frame(&scene, &assets, &mut frame)
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
                            "合成: 平均 {:.3} ms / p95 {:.3} ms / 実時間比 {:.3}x",
                            stats.mean_ms, stats.p95_ms, stats.realtime_ratio
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

fn format_clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0).floor() as u64;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
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
