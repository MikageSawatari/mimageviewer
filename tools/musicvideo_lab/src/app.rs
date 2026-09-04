use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;
use musicvideo_core::{
    Assets, AudioRef, Frame, ImageAsset, ImageId, ImageRef, LayerKind, Project, RenderReport,
    Smoothing, SpectrumFrame, SpectrumSource, UnresolvedAsset, evaluate, render_frame,
};
use musicvideo_ui::{
    EditorState, MissingImageNotice, draw_editor, draw_missing_images, draw_preview,
};

use crate::decode::{AudioData, decode_audio};
use crate::export::{ExportOutcome, export_png_sequence, frame_count};
use crate::playback::AudioPlayer;
use crate::project::{BACKGROUND_ID, DEFAULT_DURATION_SECS, default_project, spectrum_band_count};
use crate::project_file::{ensure_project_extension, load_project, save_project};

type AudioLoadResult = Result<(PathBuf, Arc<AudioData>), String>;
type ImageLoadResult = Result<(ImageId, PathBuf, ImageAsset), (ImageId, String)>;

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
    spectrum: Option<SpectrumSource>,
}

struct LoadedProject {
    path: PathBuf,
    project: Project,
    audio: Option<Arc<AudioData>>,
    images: Vec<(ImageId, ImageAsset)>,
    image_errors: HashMap<ImageId, String>,
    audio_error: Option<String>,
}

struct ProjectLoadPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<LoadedProject, String>>,
}

struct ProjectSavePending {
    rx: mpsc::Receiver<Result<PathBuf, String>>,
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
    audio_player: Option<AudioPlayer>,
    audio_error: Option<String>,
    project_path: Option<PathBuf>,
    image_errors: HashMap<ImageId, String>,
    status: String,
    audio_pending: Option<AudioPending>,
    image_pending: Option<ImagePending>,
    preview_pending: Option<PreviewPending>,
    preview_dirty: bool,
    preview_revision: u64,
    preview_texture: Option<egui::TextureHandle>,
    preview_render_ms: f64,
    preview_report: RenderReport,
    preview_spectrum: Option<SpectrumSource>,
    export_pending: Option<ExportPending>,
    last_export: Option<ExportOutcome>,
    editor_state: EditorState,
    project_load_pending: Option<ProjectLoadPending>,
    project_save_pending: Option<ProjectSavePending>,
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
            audio_player: None,
            audio_error: None,
            project_path: None,
            image_errors: HashMap::new(),
            status,
            audio_pending: None,
            image_pending: None,
            preview_pending: None,
            preview_dirty: true,
            preview_revision: 1,
            preview_texture: None,
            preview_render_ms: 0.0,
            preview_report: RenderReport::default(),
            preview_spectrum: None,
            export_pending: None,
            last_export: None,
            editor_state: EditorState::default(),
            project_load_pending: None,
            project_save_pending: None,
        }
    }

    fn mark_preview_dirty(&mut self) {
        self.preview_revision = self.preview_revision.wrapping_add(1);
        self.preview_dirty = true;
        if let Some(pending) = &self.preview_pending {
            pending.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn schedule_playback_preview(&mut self) {
        self.preview_revision = self.preview_revision.wrapping_add(1);
        self.preview_dirty = true;
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

    fn start_image_load(&mut self, id: ImageId, path: PathBuf, ctx: &egui::Context) {
        if let Some(pending) = self.image_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let worker_path = path.clone();
        let worker_id = id.clone();
        let worker_ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-image-load".to_owned())
            .spawn(move || {
                let result = load_image(&worker_path)
                    .map(|image| (worker_id.clone(), worker_path, image))
                    .map_err(|error| (worker_id, error));
                if !worker_cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(result);
                    worker_ctx.request_repaint();
                }
            });
        match spawned {
            Ok(_) => {
                self.status = format!("画像を読み込んでいます: {}", path.display());
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
                self.project.audio = Some(AudioRef { path });
                self.audio_error = None;
                self.preview_spectrum = None;
                self.audio_player = match AudioPlayer::new(Arc::clone(&audio)) {
                    Ok(player) => Some(player),
                    Err(error) => {
                        self.status =
                            format!("音声は読み込みましたが再生を開始できません: {error}");
                        None
                    }
                };
                self.audio = Some(audio);
                if self.audio_player.is_some() {
                    self.status = "音声を読み込みました".to_owned();
                }
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
            Some(Ok(Ok((id, path, image)))) => {
                self.image_pending = None;
                self.assets.insert_image(id.clone(), image);
                if let Some(reference) = self.project.images.iter_mut().find(|item| item.id == id) {
                    reference.path = path;
                } else {
                    self.project.images.push(ImageRef {
                        id: id.clone(),
                        path,
                    });
                }
                self.image_errors.remove(&id);
                self.status = "画像を読み込みました".to_owned();
                self.mark_preview_dirty();
            }
            Some(Ok(Err((id, error)))) => {
                self.image_pending = None;
                self.image_errors.insert(id, error.clone());
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
        let previous_spectrum = self.preview_spectrum.take();
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
                let spectrum_frame = match audio {
                    Some(audio) => (|| {
                        let mut source = match previous_spectrum
                            .filter(|source| source.band_count() == band_count)
                        {
                            Some(source) => source,
                            None => SpectrumSource::new(
                                audio.sample_rate,
                                band_count,
                                Smoothing::Average { factor: 0.7 },
                            )
                            .map_err(|error| format!("スペクトラム解析に失敗しました: {error}"))?,
                        };
                        let frame = source
                            .full_frame_at(&audio.stereo_pcm, position_secs, 0.0)
                            .clone();
                        Ok((frame, Some(source)))
                    })(),
                    None => Ok((SpectrumFrame::new(vec![0.0; band_count]), None)),
                };
                let result = spectrum_frame.and_then(|(spectrum_frame, spectrum)| {
                    if worker_cancel.load(Ordering::Relaxed) {
                        return Err("プレビューをキャンセルしました".to_owned());
                    }
                    let scene = evaluate(&project, position_secs, &spectrum_frame);
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
                        spectrum,
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
                    self.preview_spectrum = result.spectrum;
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

    fn start_project_load(&mut self, path: PathBuf, ctx: &egui::Context) {
        if let Some(pending) = self.project_load_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_ctx = ctx.clone();
        let worker_path = path.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-project-load".to_owned())
            .spawn(move || {
                let result = load_project_bundle(worker_path, &worker_cancel);
                if !worker_cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(result);
                    worker_ctx.request_repaint();
                }
            });
        match spawned {
            Ok(_) => {
                self.status = format!("プロジェクトを読み込んでいます: {}", path.display());
                self.project_load_pending = Some(ProjectLoadPending { cancel, rx });
            }
            Err(error) => {
                self.status = format!("プロジェクト読込ワーカーを開始できません: {error}");
            }
        }
    }

    fn start_project_save(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.project_save_pending.is_some() {
            return;
        }
        let project = self.project.clone();
        let worker_ctx = ctx.clone();
        let worker_path = path.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-project-save".to_owned())
            .spawn(move || {
                let result = save_project(&worker_path, &project).map(|()| worker_path);
                let _ = tx.send(result);
                worker_ctx.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.status = format!("プロジェクトを保存しています: {}", path.display());
                self.project_save_pending = Some(ProjectSavePending { rx });
            }
            Err(error) => {
                self.status = format!("プロジェクト保存ワーカーを開始できません: {error}");
            }
        }
    }

    fn poll_projects(&mut self) {
        let load_result = self
            .project_load_pending
            .as_ref()
            .map(|pending| pending.rx.try_recv());
        match load_result {
            Some(Ok(Ok(loaded))) => {
                self.project_load_pending = None;
                if let Some(pending) = self.audio_pending.take() {
                    pending.cancel.store(true, Ordering::Relaxed);
                }
                if let Some(pending) = self.image_pending.take() {
                    pending.cancel.store(true, Ordering::Relaxed);
                }
                if let Some(player) = &self.audio_player {
                    player.pause();
                }
                for image in self.project.images.iter().chain(&loaded.project.images) {
                    self.assets.remove_image(&image.id);
                }
                for (id, image) in loaded.images {
                    self.assets.insert_image(id, image);
                }
                self.project = loaded.project;
                self.project_path = Some(loaded.path);
                self.position_secs = 0.0;
                self.audio = loaded.audio;
                self.audio_error = loaded.audio_error.clone();
                self.preview_spectrum = None;
                self.image_errors = loaded.image_errors;
                self.audio_player = self
                    .audio
                    .clone()
                    .map(AudioPlayer::new)
                    .transpose()
                    .unwrap_or_else(|error| {
                        self.status =
                            format!("プロジェクトは開きましたが音声を再生できません: {error}");
                        None
                    });
                self.editor_state.reconcile(&self.project);
                self.preview_report = RenderReport::default();
                let issue_count =
                    self.image_errors.len() + usize::from(loaded.audio_error.is_some());
                if let Some(error) = loaded.audio_error {
                    self.status = format!("プロジェクトを開きました。音声素材の問題: {error}");
                } else if issue_count > 0 {
                    self.status =
                        format!("プロジェクトを開きました。未解決素材が {issue_count} 件あります");
                } else if self.audio_player.is_some() || self.audio.is_none() {
                    self.status = "プロジェクトを開きました".to_owned();
                }
                self.mark_preview_dirty();
            }
            Some(Ok(Err(error))) => {
                self.project_load_pending = None;
                self.status = error;
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.project_load_pending = None;
                self.status = "プロジェクト読込ワーカーが終了しました".to_owned();
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }

        let save_result = self
            .project_save_pending
            .as_ref()
            .map(|pending| pending.rx.try_recv());
        match save_result {
            Some(Ok(Ok(path))) => {
                self.project_save_pending = None;
                self.project_path = Some(path.clone());
                self.status = format!("プロジェクトを保存しました: {}", path.display());
            }
            Some(Ok(Err(error))) => {
                self.project_save_pending = None;
                self.status = error;
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.project_save_pending = None;
                self.status = "プロジェクト保存ワーカーが終了しました".to_owned();
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
    }
}

fn load_project_bundle(path: PathBuf, cancel: &AtomicBool) -> Result<LoadedProject, String> {
    let project = load_project(&path)?;
    let mut images = Vec::new();
    let mut image_errors = HashMap::new();
    for reference in &project.images {
        if cancel.load(Ordering::Relaxed) {
            return Err("プロジェクトの読み込みをキャンセルしました".to_owned());
        }
        match load_image(&reference.path) {
            Ok(image) => images.push((reference.id.clone(), image)),
            Err(error) => {
                image_errors.insert(reference.id.clone(), error);
            }
        }
    }
    let (audio, audio_error) = match &project.audio {
        Some(reference) => match decode_audio(&reference.path, cancel) {
            Ok(audio) => (Some(Arc::new(audio)), None),
            Err(error) => (None, Some(error)),
        },
        None => (None, None),
    };
    Ok(LoadedProject {
        path,
        project,
        audio,
        images,
        image_errors,
        audio_error,
    })
}

fn load_image(path: &std::path::Path) -> Result<ImageAsset, String> {
    let image = image::open(path)
        .map_err(|error| format!("画像を読み込めません ({}): {error}", path.display()))?
        .to_rgba8();
    ImageAsset::new(image.width(), image.height(), image.into_raw())
        .map_err(|error| format!("画像を保持できません ({}): {error}", path.display()))
}

fn unresolved_image_notices(
    project: &Project,
    report: &RenderReport,
    image_errors: &HashMap<ImageId, String>,
) -> Vec<MissingImageNotice> {
    let mut notices = Vec::new();
    for asset in &report.unresolved_assets {
        let UnresolvedAsset::Image(id) = asset else {
            continue;
        };
        let path = project
            .images
            .iter()
            .find(|reference| &reference.id == id)
            .map(|reference| reference.path.clone());
        let reason = image_errors
            .get(id)
            .cloned()
            .unwrap_or_else(|| "画像を解決できません".to_owned());
        for layer in &project.layers {
            if matches!(&layer.kind, LayerKind::Image { image, .. } if image == id) {
                notices.push(MissingImageNotice {
                    id: id.clone(),
                    layer_name: layer.name.clone(),
                    path: path.clone(),
                    reason: reason.clone(),
                });
            }
        }
    }
    notices
}

impl eframe::App for MusicVideoLabApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_loads();
        self.poll_preview(ctx);
        self.poll_export();
        self.poll_projects();

        let playback = self
            .audio_player
            .as_ref()
            .map(AudioPlayer::snapshot)
            .unwrap_or_default();
        if self.audio_player.is_some()
            && (playback.position_secs - self.position_secs).abs() > 0.000_001
        {
            self.position_secs = playback.position_secs.min(self.project.duration_secs);
            self.schedule_playback_preview();
        }
        let mut replacement_request = None;

        egui::TopBottomPanel::top("musicvideo-controls").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.project_load_pending.is_none()
                            && self.export_pending.is_none()
                            && self.audio_pending.is_none()
                            && self.image_pending.is_none(),
                        egui::Button::new("プロジェクトを開く"),
                    )
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("mIV 音楽ビデオ", &["json"])
                        .pick_file()
                {
                    self.start_project_load(path, ctx);
                }
                if ui
                    .add_enabled(
                        self.project_save_pending.is_none()
                            && self.project_load_pending.is_none()
                            && self.audio_pending.is_none()
                            && self.image_pending.is_none(),
                        egui::Button::new("プロジェクトを保存"),
                    )
                    .clicked()
                {
                    let path = self.project_path.clone().or_else(|| {
                        rfd::FileDialog::new()
                            .add_filter("mIV 音楽ビデオ", &["json"])
                            .set_file_name("project.mivmv.json")
                            .save_file()
                    });
                    if let Some(path) = path {
                        self.start_project_save(ensure_project_extension(path), ctx);
                    }
                }
                if ui
                    .add_enabled(
                        self.project_save_pending.is_none()
                            && self.project_load_pending.is_none()
                            && self.audio_pending.is_none()
                            && self.image_pending.is_none(),
                        egui::Button::new("名前を付けて保存"),
                    )
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("mIV 音楽ビデオ", &["json"])
                        .set_file_name("project.mivmv.json")
                        .save_file()
                {
                    self.start_project_save(ensure_project_extension(path), ctx);
                }
                if ui
                    .add_enabled(
                        self.audio_pending.is_none()
                            && self.project_load_pending.is_none()
                            && self.export_pending.is_none(),
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
                        self.image_pending.is_none()
                            && self.project_load_pending.is_none()
                            && self.export_pending.is_none(),
                        egui::Button::new("背景画像を開く"),
                    )
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("画像", &["png", "jpg", "jpeg", "webp", "bmp"])
                        .pick_file()
                {
                    self.start_image_load(ImageId::new(BACKGROUND_ID), path, ctx);
                }

                let ready_to_export = self.audio.is_some()
                    && self.assets.has_image(&ImageId::new(BACKGROUND_ID))
                    && self.project_load_pending.is_none()
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
                let notices = unresolved_image_notices(
                    &self.project,
                    &self.preview_report,
                    &self.image_errors,
                );
                replacement_request = draw_missing_images(ui, &notices);
                for asset in &self.preview_report.unresolved_assets {
                    if let UnresolvedAsset::Font(key) = asset {
                        ui.colored_label(
                            egui::Color32::LIGHT_RED,
                            format!("未解決素材: フォント {key}"),
                        );
                    }
                }
            }
            if let Some(path) = self.project_path.as_ref() {
                ui.label(format!("プロジェクト: {}", path.display()));
            }
            if let Some(audio) = &self.project.audio {
                ui.label(format!("音声: {}", audio.path.display()));
            }
            if let Some(error) = &self.audio_error {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    format!("未解決の音声素材: {error}"),
                );
            }
            for image in &self.project.images {
                ui.label(format!("画像 {}: {}", image.id.0, image.path.display()));
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
                if ui
                    .add_enabled(
                        self.audio_player.is_some(),
                        egui::Button::new(if playback.playing { "一時停止" } else { "再生" }),
                    )
                    .clicked()
                    && let Some(player) = &self.audio_player
                {
                    let snapshot = player.toggle();
                    self.position_secs = snapshot.position_secs;
                    self.schedule_playback_preview();
                }
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
                    if let Some(player) = &self.audio_player {
                        self.position_secs = player.seek(self.position_secs).position_secs;
                    }
                    self.preview_spectrum = None;
                    self.mark_preview_dirty();
                }
                ui.label(format!("合成 {:.2} ms", self.preview_render_ms));
            });
            ui.small(
                "シーク時はスペクトラムの履歴をリセットします。連続再生中は書き出しと同じ解析順です。",
            );
        });

        if let Some(id) = replacement_request
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("画像", &["png", "jpg", "jpeg", "webp", "bmp"])
                .pick_file()
        {
            self.start_image_load(id, path, ctx);
        }

        egui::SidePanel::right("musicvideo-properties")
            .default_width(330.0)
            .show(ctx, |ui| {
                let response = ui
                    .add_enabled_ui(self.project_load_pending.is_none(), |ui| {
                        draw_editor(ui, &mut self.project, &mut self.editor_state)
                    })
                    .inner;
                if response.project_changed {
                    self.mark_preview_dirty();
                }
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                draw_preview(ui, self.preview_texture.as_ref(), &mut self.editor_state);
            });

        self.start_preview_if_needed(ctx);
        if self.audio_pending.is_some()
            || self.image_pending.is_some()
            || self.preview_pending.is_some()
            || self.export_pending.is_some()
            || self.project_load_pending.is_some()
            || self.project_save_pending.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if playback.playing {
            ctx.request_repaint_after(Duration::from_millis(16));
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
        if let Some(pending) = &self.project_load_pending {
            pending.cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unresolved_image_notice_names_the_layer_and_saved_path() {
        let mut project = default_project(1.0);
        project.images.push(ImageRef {
            id: ImageId::new(BACKGROUND_ID),
            path: PathBuf::from("media").join("missing.png"),
        });
        let mut report = RenderReport::default();
        report
            .unresolved_assets
            .push(UnresolvedAsset::Image(ImageId::new(BACKGROUND_ID)));
        let errors = HashMap::from([(
            ImageId::new(BACKGROUND_ID),
            "ファイルがありません".to_owned(),
        )]);

        let notices = unresolved_image_notices(&project, &report, &errors);
        assert_eq!(
            notices,
            vec![MissingImageNotice {
                id: ImageId::new(BACKGROUND_ID),
                layer_name: "背景画像".to_owned(),
                path: Some(PathBuf::from("media").join("missing.png")),
                reason: "ファイルがありません".to_owned(),
            }]
        );
    }

    #[test]
    fn project_bundle_restores_image_pixels_from_the_saved_reference() {
        let temp = tempfile::tempdir().unwrap();
        let image_path = temp.path().join("media").join("cover.png");
        std::fs::create_dir_all(image_path.parent().unwrap()).unwrap();
        image::RgbaImage::from_pixel(3, 2, image::Rgba([12, 34, 56, 255]))
            .save(&image_path)
            .unwrap();
        let project_path = temp.path().join("project.mivmv.json");
        let mut project = default_project(1.0);
        project.images.push(ImageRef {
            id: ImageId::new(BACKGROUND_ID),
            path: image_path.clone(),
        });
        let original_assets = Assets::default();
        original_assets.insert_image(
            ImageId::new(BACKGROUND_ID),
            load_image(&image_path).unwrap(),
        );
        let spectrum = SpectrumFrame::new(vec![0.25; spectrum_band_count(&project)]);
        let scene = evaluate(&project, 0.5, &spectrum);
        let mut expected = Frame::new(project.output.width, project.output.height).unwrap();
        let _ = render_frame(&scene, &original_assets, &mut expected).unwrap();
        save_project(&project_path, &project).unwrap();

        let loaded =
            load_project_bundle(project_path, &AtomicBool::new(false)).expect("project loads");
        assert_eq!(loaded.project, project);
        assert!(loaded.image_errors.is_empty());
        assert_eq!(loaded.images.len(), 1);
        assert_eq!(loaded.images[0].1.rgba, [12, 34, 56, 255].repeat(6));

        let restored_assets = Assets::default();
        for (id, image) in loaded.images {
            restored_assets.insert_image(id, image);
        }
        let scene = evaluate(&loaded.project, 0.5, &spectrum);
        let mut actual =
            Frame::new(loaded.project.output.width, loaded.project.output.height).unwrap();
        let _ = render_frame(&scene, &restored_assets, &mut actual).unwrap();
        assert_eq!(actual.rgba, expected.rgba);
    }

    #[test]
    fn project_bundle_keeps_missing_image_as_an_explicit_error() {
        let temp = tempfile::tempdir().unwrap();
        let project_path = temp.path().join("project.mivmv.json");
        let mut project = default_project(1.0);
        project.images.push(ImageRef {
            id: ImageId::new(BACKGROUND_ID),
            path: temp.path().join("missing.png"),
        });
        save_project(&project_path, &project).unwrap();

        let loaded =
            load_project_bundle(project_path, &AtomicBool::new(false)).expect("project loads");
        assert!(loaded.images.is_empty());
        assert!(
            loaded
                .image_errors
                .get(&ImageId::new(BACKGROUND_ID))
                .is_some_and(|error| error.contains("missing.png"))
        );
    }
}
