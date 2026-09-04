use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;
use music_core::{AnalysisConfig, analyze_stereo_waveform_cancellable};
use musicvideo_core::{
    Assets, AudioRef, Frame, ImageAsset, ImageId, ImageRef, LayerKind, Project, RenderReport,
    Smoothing, SpectrumFrame, SpectrumSource, UnresolvedAsset, evaluate, new_image_layer,
    render_frame,
};
use musicvideo_ui::{
    EditorState, MissingImageNotice, TimelineWaveform, draw_editor, draw_missing_images,
    draw_preview, draw_timeline,
};

use crate::decode::{AudioData, decode_audio};
use crate::export::{ExportOutcome, export_png_sequence, frame_count};
use crate::playback::AudioPlayer;
use crate::project::{BACKGROUND_ID, DEFAULT_DURATION_SECS, default_project, spectrum_band_count};
use crate::project_file::{ensure_project_extension, load_project, save_project};

type AudioLoadResult = Result<(PathBuf, Arc<AudioData>, TimelineWaveform), String>;
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
    timings: PreviewWorkerTimings,
}

#[derive(Debug, Clone, Copy, Default)]
struct PreviewWorkerTimings {
    analysis_ms: f64,
    scene_composition_ms: f64,
    rgba_to_egui_ms: f64,
    total_ms: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct PreviewPerfSample {
    worker: PreviewWorkerTimings,
    texture_update_ms: f64,
    egui_layout_ms: f64,
    audio_sync_ms: f64,
    ui_other_ms: f64,
    display_interval_ms: f64,
}

#[derive(Default)]
struct PreviewPerfTracker {
    samples: VecDeque<PreviewPerfSample>,
    last_displayed_at: Option<Instant>,
    total_samples: usize,
}

#[derive(Debug, Clone, Copy)]
struct PreviewPerfSummary {
    samples: usize,
    fps: f64,
    display_interval_ms: f64,
    analysis_ms: f64,
    scene_composition_ms: f64,
    rgba_to_egui_ms: f64,
    worker_other_ms: f64,
    texture_update_ms: f64,
    egui_layout_ms: f64,
    audio_sync_ms: f64,
    ui_other_ms: f64,
    outside_worker_ms: f64,
}

impl PreviewPerfTracker {
    const WINDOW: usize = 300;

    fn reset(&mut self) {
        self.samples.clear();
        self.last_displayed_at = None;
        self.total_samples = 0;
    }

    fn displayed_interval(&mut self, now: Instant) -> Option<f64> {
        let interval = self
            .last_displayed_at
            .replace(now)
            .map(|previous| now.duration_since(previous).as_secs_f64() * 1000.0);
        interval
    }

    fn push(&mut self, sample: PreviewPerfSample) {
        if self.samples.len() == Self::WINDOW {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
        self.total_samples = self.total_samples.saturating_add(1);
    }

    fn summary(&self) -> Option<PreviewPerfSummary> {
        if self.samples.is_empty() {
            return None;
        }
        let mean = |value: fn(&PreviewPerfSample) -> f64| {
            self.samples.iter().map(value).sum::<f64>() / self.samples.len() as f64
        };
        let display_interval_ms = mean(|sample| sample.display_interval_ms);
        let worker_total_ms = mean(|sample| sample.worker.total_ms);
        Some(PreviewPerfSummary {
            samples: self.samples.len(),
            fps: 1000.0 / display_interval_ms.max(f64::EPSILON),
            display_interval_ms,
            analysis_ms: mean(|sample| sample.worker.analysis_ms),
            scene_composition_ms: mean(|sample| sample.worker.scene_composition_ms),
            rgba_to_egui_ms: mean(|sample| sample.worker.rgba_to_egui_ms),
            worker_other_ms: mean(|sample| {
                (sample.worker.total_ms
                    - sample.worker.analysis_ms
                    - sample.worker.scene_composition_ms
                    - sample.worker.rgba_to_egui_ms)
                    .max(0.0)
            }),
            texture_update_ms: mean(|sample| sample.texture_update_ms),
            egui_layout_ms: mean(|sample| sample.egui_layout_ms),
            audio_sync_ms: mean(|sample| sample.audio_sync_ms),
            ui_other_ms: mean(|sample| sample.ui_other_ms),
            outside_worker_ms: (display_interval_ms - worker_total_ms).max(0.0),
        })
    }
}

struct LoadedProject {
    path: PathBuf,
    project: Project,
    audio: Option<Arc<AudioData>>,
    waveform: Option<TimelineWaveform>,
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
    waveform: Option<TimelineWaveform>,
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
    preview_perf: PreviewPerfTracker,
    export_pending: Option<ExportPending>,
    last_export: Option<ExportOutcome>,
    editor_state: EditorState,
    project_load_pending: Option<ProjectLoadPending>,
    project_save_pending: Option<ProjectSavePending>,
    preview_benchmark_target: Option<usize>,
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
            waveform: None,
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
            preview_perf: PreviewPerfTracker::default(),
            export_pending: None,
            last_export: None,
            editor_state: EditorState::default(),
            project_load_pending: None,
            project_save_pending: None,
            preview_benchmark_target: None,
        }
    }

    pub fn new_preview_benchmark(
        assets: Arc<Assets>,
        measured_frames: usize,
    ) -> Result<Self, String> {
        const SAMPLE_RATE: u32 = 48_000;
        const DURATION_SECS: f64 = 30.0;

        let mut app = Self::new(assets);
        app.project = default_project(DURATION_SECS);
        app.project
            .layers
            .push(crate::project::new_pitch_keyboard_layer(&app.project));
        let audio = Arc::new(AudioData {
            stereo_pcm: vec![0.0; SAMPLE_RATE as usize * DURATION_SECS as usize * 2],
            sample_rate: SAMPLE_RATE,
            duration_secs: DURATION_SECS,
        });
        let player = AudioPlayer::new(Arc::clone(&audio))?;
        let _ = player.toggle();
        app.audio = Some(audio);
        app.audio_player = Some(player);
        app.status = "鍵盤入りプレビューを計測しています".to_owned();
        app.preview_benchmark_target = Some(measured_frames.max(1));
        Ok(app)
    }

    fn mark_preview_dirty(&mut self) {
        self.preview_revision = self.preview_revision.wrapping_add(1);
        self.preview_dirty = true;
        self.preview_perf.reset();
        if let Some(pending) = &self.preview_pending {
            pending.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn schedule_playback_preview(&mut self) {
        // Playback time advances while the current worker is rendering. Keep that completed
        // frame, then render the newest audio position next. Revisions are reserved for project
        // edits and seeks that really invalidate an in-flight frame.
        self.preview_dirty = true;
    }

    fn seek_to(&mut self, position_secs: f64) {
        let requested = position_secs.clamp(0.0, self.project.duration_secs.max(0.0));
        self.position_secs = if let Some(player) = &self.audio_player {
            player.seek(requested).position_secs
        } else {
            requested
        };
        self.preview_spectrum = None;
        self.mark_preview_dirty();
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
                let result = decode_audio(&worker_path, &worker_cancel).and_then(|audio| {
                    let waveform = analyze_waveform(&audio, &worker_cancel)?;
                    Ok((worker_path, Arc::new(audio), waveform))
                });
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
            Some(Ok(Ok((path, audio, waveform)))) => {
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
                self.waveform = Some(waveform);
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
                let worker_started = Instant::now();
                let band_count = spectrum_band_count(&project);
                let analysis_started = Instant::now();
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
                            .full_frame_at(
                                &audio.stereo_pcm,
                                position_secs,
                                project.display_gain_db,
                            )
                            .clone();
                        Ok((frame, Some(source)))
                    })(),
                    None => Ok((SpectrumFrame::new(vec![0.0; band_count]), None)),
                };
                let analysis_ms = analysis_started.elapsed().as_secs_f64() * 1000.0;
                let result = spectrum_frame.and_then(|(spectrum_frame, spectrum)| {
                    if worker_cancel.load(Ordering::Relaxed) {
                        return Err("プレビューをキャンセルしました".to_owned());
                    }
                    let scene_composition_started = Instant::now();
                    let scene = evaluate(&project, position_secs, &spectrum_frame);
                    let mut frame = Frame::new(project.output.width, project.output.height)
                        .map_err(|error| format!("プレビューを確保できません: {error}"))?;
                    let started = Instant::now();
                    let report = render_frame(&scene, &assets, &mut frame)
                        .map_err(|error| format!("プレビュー合成に失敗しました: {error}"))?;
                    let render_ms = started.elapsed().as_secs_f64() * 1000.0;
                    let scene_composition_ms =
                        scene_composition_started.elapsed().as_secs_f64() * 1000.0;
                    let rgba_to_egui_started = Instant::now();
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [frame.width as usize, frame.height as usize],
                        &frame.rgba,
                    );
                    let rgba_to_egui_ms = rgba_to_egui_started.elapsed().as_secs_f64() * 1000.0;
                    let total_ms = worker_started.elapsed().as_secs_f64() * 1000.0;
                    Ok(PreviewResult {
                        revision,
                        image,
                        render_ms,
                        report,
                        spectrum,
                        timings: PreviewWorkerTimings {
                            analysis_ms,
                            scene_composition_ms,
                            rgba_to_egui_ms,
                            total_ms,
                        },
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

    fn poll_preview(&mut self, ctx: &egui::Context) -> Option<PreviewPerfSample> {
        let mut displayed = None;
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
                    let texture_started = Instant::now();
                    if let Some(texture) = &mut self.preview_texture {
                        texture.set(result.image, egui::TextureOptions::LINEAR);
                    } else {
                        self.preview_texture = Some(ctx.load_texture(
                            "musicvideo-preview",
                            result.image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                    let texture_update_ms = texture_started.elapsed().as_secs_f64() * 1000.0;
                    if let Some(display_interval_ms) =
                        self.preview_perf.displayed_interval(Instant::now())
                    {
                        displayed = Some(PreviewPerfSample {
                            worker: result.timings,
                            texture_update_ms,
                            display_interval_ms,
                            ..PreviewPerfSample::default()
                        });
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
        displayed
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
                self.waveform = loaded.waveform;
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
    let (audio, waveform, audio_error) = match &project.audio {
        Some(reference) => match decode_audio(&reference.path, cancel) {
            Ok(audio) => match analyze_waveform(&audio, cancel) {
                Ok(waveform) => (Some(Arc::new(audio)), Some(waveform), None),
                Err(error) => (None, None, Some(error)),
            },
            Err(error) => (None, None, Some(error)),
        },
        None => (None, None, None),
    };
    Ok(LoadedProject {
        path,
        project,
        audio,
        waveform,
        images,
        image_errors,
        audio_error,
    })
}

fn analyze_waveform(audio: &AudioData, cancel: &AtomicBool) -> Result<TimelineWaveform, String> {
    let analysis = analyze_stereo_waveform_cancellable(
        &audio.stereo_pcm,
        audio.sample_rate,
        AnalysisConfig::default(),
        &|| cancel.load(Ordering::Relaxed),
    )
    .ok_or_else(|| "音声波形の解析をキャンセルしました".to_owned())?;
    let peaks = analysis
        .bins
        .iter()
        .map(|bin| [bin.peak_l, bin.peak_r])
        .collect();
    Ok(TimelineWaveform::new(audio.duration_secs, peaks))
}

fn next_image_id(project: &Project) -> ImageId {
    for number in 1_u64.. {
        let candidate = ImageId::new(format!("image-{number}"));
        let in_references = project.images.iter().any(|image| image.id == candidate);
        let in_layers = project.layers.iter().any(
            |layer| matches!(&layer.kind, LayerKind::Image { image, .. } if image == &candidate),
        );
        if !in_references && !in_layers {
            return candidate;
        }
    }
    unreachable!("u64 image ID space is exhausted")
}

fn image_layer_insertion_index(project: &Project) -> usize {
    project
        .layers
        .iter()
        .rposition(|layer| matches!(&layer.kind, LayerKind::Image { .. }))
        .map_or(0, |index| index + 1)
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
        let update_started = Instant::now();
        self.poll_loads();
        let mut displayed_perf = self.poll_preview(ctx);
        self.poll_export();
        self.poll_projects();

        let audio_sync_started = Instant::now();
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
        let audio_sync_ms = audio_sync_started.elapsed().as_secs_f64() * 1000.0;
        let mut replacement_request = None;
        let mut add_image_layer_requested = false;
        let mut reset_preview_perf = false;

        let egui_layout_started = Instant::now();
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
                    reset_preview_perf = true;
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
                    self.seek_to(self.position_secs);
                }
                ui.label(format!("合成 {:.2} ms", self.preview_render_ms));
                if ui.small_button("計測をリセット").clicked() {
                    reset_preview_perf = true;
                }
            });
            ui.small(
                "シーク時はスペクトラムの履歴をリセットします。連続再生中は書き出しと同じ解析順です。",
            );
            if let Some(perf) = self.preview_perf.summary() {
                ui.small(format!(
                    "プレビュー実測 (直近 {}): {:.1} fps / 表示間隔 {:.2} ms",
                    perf.samples, perf.fps, perf.display_interval_ms
                ));
                ui.small(format!(
                    "worker: 解析 {:.2} / シーン評価+合成 {:.2} / RGBA→egui {:.2} / その他 {:.2} ms",
                    perf.analysis_ms,
                    perf.scene_composition_ms,
                    perf.rgba_to_egui_ms,
                    perf.worker_other_ms
                ));
                ui.small(format!(
                    "UI: texture.set {:.2} / egui layout+draw {:.2} / 音声同期 {:.3} / その他 {:.2} ms",
                    perf.texture_update_ms,
                    perf.egui_layout_ms,
                    perf.audio_sync_ms,
                    perf.ui_other_ms
                ));
                ui.small(format!(
                    "表示間隔−worker (eframe描画/GPU upload/待機、並行区間を含む): {:.2} ms",
                    perf.outside_worker_ms
                ));
            }
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
                        draw_editor(
                            ui,
                            &mut self.project,
                            &mut self.editor_state,
                            self.position_secs,
                        )
                    })
                    .inner;
                add_image_layer_requested = response.add_image_layer_requested;
                if response.project_changed {
                    self.mark_preview_dirty();
                }
            });

        if add_image_layer_requested
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("画像", &["png", "jpg", "jpeg", "webp", "bmp"])
                .pick_file()
        {
            let image_id = next_image_id(&self.project);
            let mut layer = new_image_layer(&self.project, image_id.clone());
            if let Some(name) = path.file_stem().and_then(|name| name.to_str()) {
                layer.name = name.to_owned();
            }
            let insertion_index = image_layer_insertion_index(&self.project);
            self.project.layers.insert(insertion_index, layer);
            self.editor_state.select_layer(insertion_index);
            self.mark_preview_dirty();
            self.start_image_load(image_id, path, ctx);
        }

        let timeline_response = egui::TopBottomPanel::bottom("musicvideo-timeline")
            .resizable(true)
            .default_height(320.0)
            .min_height(150.0)
            .show(ctx, |ui| {
                draw_timeline(
                    ui,
                    &mut self.project,
                    &mut self.editor_state,
                    self.position_secs,
                    self.waveform.as_ref(),
                )
            })
            .inner;
        if let Some(position_secs) = timeline_response.seek_to {
            self.seek_to(position_secs);
        } else if timeline_response.project_changed {
            self.mark_preview_dirty();
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                draw_preview(ui, self.preview_texture.as_ref(), &mut self.editor_state);
            });
        let egui_layout_ms = egui_layout_started.elapsed().as_secs_f64() * 1000.0;

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
        if reset_preview_perf {
            self.preview_perf.reset();
            displayed_perf = None;
        }
        if let Some(mut sample) = displayed_perf {
            sample.audio_sync_ms = audio_sync_ms;
            sample.egui_layout_ms = egui_layout_ms;
            let update_ms = update_started.elapsed().as_secs_f64() * 1000.0;
            sample.ui_other_ms = (update_ms
                - sample.texture_update_ms
                - sample.audio_sync_ms
                - sample.egui_layout_ms)
                .max(0.0);
            self.preview_perf.push(sample);
        }
        if self
            .preview_benchmark_target
            .is_some_and(|target| self.preview_perf.total_samples >= target)
        {
            if let Some(perf) = self.preview_perf.summary() {
                println!(
                    "musicvideo preview benchmark: frames={} resolution={}x{} fps={:.3} interval_ms={:.3} analysis_ms={:.3} scene_composition_ms={:.3} rgba_to_egui_ms={:.3} worker_other_ms={:.3} texture_update_ms={:.3} egui_layout_ms={:.3} audio_sync_ms={:.3} ui_other_ms={:.3} outside_worker_ms={:.3}",
                    self.preview_perf.total_samples,
                    self.project.output.width,
                    self.project.output.height,
                    perf.fps,
                    perf.display_interval_ms,
                    perf.analysis_ms,
                    perf.scene_composition_ms,
                    perf.rgba_to_egui_ms,
                    perf.worker_other_ms,
                    perf.texture_update_ms,
                    perf.egui_layout_ms,
                    perf.audio_sync_ms,
                    perf.ui_other_ms,
                    perf.outside_worker_ms,
                );
            }
            self.preview_benchmark_target = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
    fn playback_ticks_do_not_invalidate_an_in_flight_preview() {
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let revision = app.preview_revision;
        app.preview_dirty = false;

        app.schedule_playback_preview();
        assert!(app.preview_dirty);
        assert_eq!(app.preview_revision, revision);

        app.mark_preview_dirty();
        assert_eq!(app.preview_revision, revision.wrapping_add(1));
    }

    #[test]
    fn waveform_comes_from_the_shared_music_core_analysis() {
        let audio = AudioData {
            stereo_pcm: vec![0.25; 4_800 * 2],
            sample_rate: 48_000,
            duration_secs: 0.1,
        };
        let cancel = AtomicBool::new(false);
        let waveform = analyze_waveform(&audio, &cancel).unwrap();
        assert_eq!(waveform.duration_secs, 0.1);
        assert_eq!(waveform.stereo_peaks.len(), 4);
        assert!(
            waveform
                .stereo_peaks
                .iter()
                .all(|peak| *peak == [0.25, 0.25])
        );
    }

    #[test]
    fn added_image_ids_do_not_collide_with_references_or_layers() {
        let mut project = default_project(1.0);
        project.images.push(ImageRef {
            id: ImageId::new("image-1"),
            path: PathBuf::from("first.png"),
        });
        project
            .layers
            .push(new_image_layer(&project, ImageId::new("image-2")));
        assert_eq!(next_image_id(&project), ImageId::new("image-3"));
    }

    #[test]
    fn added_images_stay_behind_visualization_layers() {
        let mut project = default_project(10.0);
        assert_eq!(image_layer_insertion_index(&project), 1);
        let layer = new_image_layer(&project, ImageId::new("image-1"));
        let index = image_layer_insertion_index(&project);
        project.layers.insert(index, layer);
        assert_eq!(image_layer_insertion_index(&project), 2);
        assert!(matches!(&project.layers[2].kind, LayerKind::Spectrum(_)));
    }

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
