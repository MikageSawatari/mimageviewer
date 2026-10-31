mod chrome;
#[cfg(test)]
mod chrome_tests;
#[cfg(test)]
mod diagnostics_tests;
mod document_intent;
#[cfg(test)]
mod document_intent_tests;
mod mp4;
#[cfg(test)]
mod mp4_tests;
#[cfg(test)]
mod parity_tests;
mod startup;
#[cfg(test)]
mod startup_tests;

use chrome::{ChromeAction, ToolbarState};
use document_intent::{DocumentIntent, DocumentIntentState};

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;
use music_core::{AnalysisConfig, analyze_stereo_waveform_cancellable};
use musicvideo_core::{
    AnalysisKey, AnalysisTimeline, Assets, AudioRef, Frame, FrameGrid, ImageAsset, ImageId,
    ImageRef, LayerId, LayerKind, LrcParseReport, LyricLine, ModulatorCalibration,
    ModulatorCalibrationRequest, ModulatorValue, Project, RenderReport, Smoothing, SpectrumFrame,
    UnresolvedAsset, calibrate_modulators_view, evaluate, modulator_calibration_requests,
    new_image_layer, parse_lrc, pcm_view, render_frame, write_lrc,
};
use musicvideo_ui::{
    DocumentSession, DocumentSessionId, EditGeneration, EditTargetLifetime, EditorCommand,
    EditorResponse, EditorState, InspectorTab, MissingImageNotice, PreviewFrame, ResourceLease,
    ResourceManifest, ResourceRevision, SaveCheckpoint, TimelineWaveform, draw_missing_images,
    draw_preview,
};

use crate::decode::{AudioData, decode_audio};
use crate::export::{ExportOutcome, export_png_sequence, frame_count};
use crate::playback::{AudioPlayer, PlaybackSnapshot};
use crate::project::{BACKGROUND_ID, DEFAULT_DURATION_SECS, default_project, spectrum_band_count};
use crate::project_file::{ensure_project_extension, load_project, save_project};
use crate::resource_blobs::{ResourceBlobStore, RestoredResources, restore_manifest};

type AudioLoadResult = Result<(PathBuf, Arc<AudioData>, TimelineWaveform, ResourceLease), String>;
type ImageLoadResult = Result<(PathBuf, Arc<ImageAsset>, ResourceLease), String>;

struct AudioPending {
    request: AudioLoadRequest,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<AudioLoadResult>,
    queued: Option<AudioLoadRequest>,
}

struct AudioLoadRequest {
    session: DocumentSessionId,
    expected: Option<ResourceRevision>,
    path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ImageLoadTarget {
    Replace(ImageId),
    AddLayer,
}

struct ImagePending {
    request: ImageLoadRequest,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<ImageLoadResult>,
    queued: Option<ImageLoadRequest>,
}

struct ImageLoadRequest {
    session: DocumentSessionId,
    target: ImageLoadTarget,
    target_lifetime: Option<EditTargetLifetime>,
    path: PathBuf,
}

struct PreviewResult {
    revision: u64,
    project: Project,
    layer_lifetimes: HashMap<LayerId, EditTargetLifetime>,
    time_secs: f64,
    image: egui::ColorImage,
    /// Test observation of the actual worker output, before egui conversion.
    #[cfg(test)]
    rgba: Vec<u8>,
    render_ms: f64,
    report: RenderReport,
    analysis_key: Option<AnalysisKey<ResourceRevision>>,
    frame_index: u64,
    modulator_values: Vec<ModulatorValue>,
    timings: PreviewWorkerTimings,
}

/// Texture and evaluation inputs are published/cleared as one displayed frame.
struct DisplayedPreview {
    texture: egui::TextureHandle,
    project: Project,
    layer_lifetimes: HashMap<LayerId, EditTargetLifetime>,
    time_secs: f64,
    modulator_values: Vec<ModulatorValue>,
}

impl DisplayedPreview {
    fn frame(&self) -> PreviewFrame<'_> {
        PreviewFrame {
            texture: &self.texture,
            project: &self.project,
            layer_lifetimes: &self.layer_lifetimes,
            time_secs: self.time_secs,
            modulator_values: &self.modulator_values,
        }
    }
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

pub(crate) struct LoadedProject {
    pub(crate) path: PathBuf,
    pub(crate) project: Project,
    pub(crate) audio: Option<Arc<AudioData>>,
    waveform: Option<TimelineWaveform>,
    pub(crate) images: Vec<(ImageId, Arc<ImageAsset>)>,
    pub(crate) resources: ResourceManifest,
    pub(crate) image_errors: HashMap<ImageId, String>,
    pub(crate) audio_error: Option<String>,
}

struct ProjectLoadPending {
    started_at: EditGeneration,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<LoadedProject, String>>,
    queued: Option<PathBuf>,
}

enum ProjectLoadState {
    Idle,
    Loading(ProjectLoadPending),
    Ready(LoadedProject),
}

impl ProjectLoadState {
    fn is_loading(&self) -> bool {
        matches!(self, Self::Loading(_))
    }
}

impl Default for ProjectLoadState {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeResourceState {
    Ready(ResourceManifest),
    Restoring {
        manifest: ResourceManifest,
        generation: u64,
    },
    Unresolved {
        manifest: ResourceManifest,
        error: String,
    },
}

struct RestoreRequest {
    generation: u64,
    session: DocumentSessionId,
    manifest: ResourceManifest,
    cancel: Arc<AtomicBool>,
}

struct RestoreCompletion {
    generation: u64,
    session: DocumentSessionId,
    manifest: ResourceManifest,
    result: Result<(RestoredResources, Option<TimelineWaveform>), String>,
}

#[derive(Default)]
struct RestoreQueueState {
    latest: Option<(RestoreRequest, egui::Context)>,
    shutdown: bool,
}

#[derive(Default)]
struct RestoreQueue {
    state: Mutex<RestoreQueueState>,
    ready: Condvar,
}

struct ResourceRestoreWorker {
    queue: Arc<RestoreQueue>,
    rx: mpsc::Receiver<RestoreCompletion>,
    next_generation: u64,
    active_cancel: Option<Arc<AtomicBool>>,
}

impl ResourceRestoreWorker {
    fn new(store: ResourceBlobStore) -> Result<Self, String> {
        let queue = Arc::new(RestoreQueue::default());
        let worker_queue = Arc::clone(&queue);
        let (result_tx, result_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("musicvideo-resource-restore".to_owned())
            .spawn(move || {
                loop {
                    let next = {
                        let mut state = worker_queue
                            .state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        while state.latest.is_none() && !state.shutdown {
                            state = worker_queue
                                .ready
                                .wait(state)
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                        }
                        if state.shutdown {
                            None
                        } else {
                            state.latest.take()
                        }
                    };
                    let Some((request, ctx)) = next else {
                        break;
                    };
                    let result =
                        restore_manifest(&store, request.manifest.clone(), &request.cancel)
                            .and_then(|resources| {
                                let waveform = resources
                                    .audio
                                    .as_deref()
                                    .map(|audio| analyze_waveform(audio, &request.cancel))
                                    .transpose()?;
                                Ok((resources, waveform))
                            });
                    let _ = result_tx.send(RestoreCompletion {
                        generation: request.generation,
                        session: request.session,
                        manifest: request.manifest,
                        result,
                    });
                    ctx.request_repaint();
                }
            })
            .map_err(|error| format!("素材履歴の復元ワーカーを開始できません: {error}"))?;
        Ok(Self {
            queue,
            rx: result_rx,
            next_generation: 1,
            active_cancel: None,
        })
    }

    fn request(
        &mut self,
        session: DocumentSessionId,
        manifest: ResourceManifest,
        ctx: &egui::Context,
    ) -> u64 {
        self.cancel();
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut state = self
            .queue
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.latest = Some((
            RestoreRequest {
                generation,
                session,
                manifest,
                cancel: Arc::clone(&cancel),
            },
            ctx.clone(),
        ));
        drop(state);
        self.active_cancel = Some(cancel);
        self.queue.ready.notify_one();
        generation
    }

    fn cancel(&mut self) {
        if let Some(cancel) = self.active_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Drop for ResourceRestoreWorker {
    fn drop(&mut self) {
        self.cancel();
        let mut state = self
            .queue
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.latest = None;
        state.shutdown = true;
        drop(state);
        self.queue.ready.notify_one();
    }
}

struct ProjectSaveWrite {
    checkpoint: SaveCheckpoint,
    rx: mpsc::Receiver<Result<(), String>>,
}

enum ProjectSavePending {
    Choosing {
        session: DocumentSessionId,
        rx: mpsc::Receiver<Option<PathBuf>>,
    },
    Writing(ProjectSaveWrite),
}

struct LrcLoadOwner {
    session: DocumentSessionId,
    layer: musicvideo_core::LayerId,
    target_lifetime: EditTargetLifetime,
    baseline: Vec<LyricLine>,
}

enum LrcLoadState {
    Idle,
    Loading {
        owner: LrcLoadOwner,
        rx: mpsc::Receiver<Result<(PathBuf, LrcParseReport), String>>,
    },
    Ready {
        owner: LrcLoadOwner,
        path: PathBuf,
        report: LrcParseReport,
    },
}

impl LrcLoadState {
    fn is_active(&self) -> bool {
        !matches!(self, Self::Idle)
    }
}

impl Default for LrcLoadState {
    fn default() -> Self {
        Self::Idle
    }
}

struct LrcSavePending {
    rx: mpsc::Receiver<Result<(PathBuf, usize), String>>,
}

struct PreviewPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<PreviewResult, String>>,
}

/// Calibration depends on the immutable PCM identity and the requested output interval.
/// Float fields use bits, matching the analysis timeline key contract.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CalibrationKey {
    pcm_id: ResourceRevision,
    duration_bits: u64,
    requests: Vec<Vec<u64>>,
}

impl CalibrationKey {
    fn new(
        pcm_id: ResourceRevision,
        duration: f64,
        requests: &[ModulatorCalibrationRequest],
    ) -> Self {
        let requests = requests
            .iter()
            .map(|request| {
                let range = match request.range {
                    musicvideo_core::BandRange::Hz { low_hz, high_hz } => {
                        [0, u64::from(low_hz.to_bits()), u64::from(high_hz.to_bits())]
                    }
                    musicvideo_core::BandRange::Semitone {
                        low_midi,
                        high_midi,
                    } => [1, u64::from(low_midi), u64::from(high_midi)],
                };
                vec![
                    request.id.0,
                    request.feature as u64,
                    range[0],
                    range[1],
                    range[2],
                    request.weighting as u64,
                    u64::from(request.gain_db.to_bits()),
                    u64::from(request.low_percentile.to_bits()),
                    u64::from(request.high_percentile.to_bits()),
                ]
            })
            .collect();
        Self {
            pcm_id,
            duration_bits: duration.to_bits(),
            requests,
        }
    }
}

/// Only the worker acquires this lock. The session retains ownership through
/// cancellation, render failures, and discarded UI results.
fn analyze_preview(
    timeline: &Mutex<AnalysisTimeline<ResourceRevision>>,
    key: &AnalysisKey<ResourceRevision>,
    pcm: music_core::PcmView<'_>,
    n: u64,
    cancel: &AtomicBool,
) -> Result<SpectrumFrame, String> {
    let mut state = match timeline.lock() {
        Ok(state) => state,
        Err(poisoned) => {
            *poisoned.into_inner() = AnalysisTimeline::default();
            timeline.clear_poison();
            eprintln!("musicvideo preview: poisoned analysis timeline was reset");
            return Err("解析ワーカーの異常により解析状態を作り直しました".to_owned());
        }
    };
    // Check after waiting for the lock and before inspecting or replacing a key.
    if cancel.load(Ordering::Relaxed) {
        return Err("プレビューをキャンセルしました".to_owned());
    }
    state
        .frame(key, pcm, n, &|| cancel.load(Ordering::Relaxed))
        .cloned()
        .map_err(|error| error.to_string())
}

struct CalibrationPending {
    cancel: Arc<AtomicBool>,
    key: CalibrationKey,
    rx: mpsc::Receiver<Result<Vec<ModulatorCalibration>, String>>,
}

enum ExportCompletion {
    Png(ExportOutcome),
    Mp4(
        Result<
            mimageviewer::video::mvexport::ExportSummary,
            mimageviewer::video::mvexport::ExportError,
        >,
    ),
}

enum ExportKind {
    Png,
    Mp4 {
        stage: Arc<AtomicUsize>,
        output_path: PathBuf,
    },
}

struct ExportPending {
    kind: ExportKind,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    total_frames: usize,
    rx: mpsc::Receiver<Result<ExportCompletion, String>>,
}

#[derive(Default)]
enum ExportState {
    #[default]
    Idle,
    Settings(crate::mp4::ExportSettings),
    Choosing {
        settings: crate::mp4::ExportSettings,
        rx: mpsc::Receiver<Option<(PathBuf, bool)>>,
    },
    Running(ExportPending),
}

impl ExportState {
    fn pending(&self) -> Option<&ExportPending> {
        if let Self::Running(pending) = self {
            Some(pending)
        } else {
            None
        }
    }

    fn setup_open(&self) -> bool {
        matches!(self, Self::Settings(_) | Self::Choosing { .. })
    }

    fn take_worker(&mut self) -> Option<ExportPending> {
        if let Self::Running(pending) = std::mem::take(self) {
            Some(pending)
        } else {
            None
        }
    }
}

pub struct MusicVideoLabApp {
    host: startup::Host,
    assets: Arc<Assets>,
    runtime_resources: RuntimeResourceState,
    resource_store: Option<ResourceBlobStore>,
    restore_worker: Option<ResourceRestoreWorker>,
    project: Project,
    position_secs: f64,
    audio: Option<Arc<AudioData>>,
    waveform: Option<TimelineWaveform>,
    audio_player: Option<AudioPlayer>,
    #[cfg(test)]
    audio_player_factory: fn(Arc<AudioData>, ResourceRevision, f64) -> Result<AudioPlayer, String>,
    playback_error: Option<String>,
    audio_error: Option<String>,
    document: DocumentSession,
    image_errors: HashMap<ImageId, String>,
    status: String,
    show_missing_resources: bool,
    show_diagnostics: bool,
    last_sent_title: String,
    audio_pending: Option<AudioPending>,
    image_pending: Option<ImagePending>,
    preview_pending: Option<PreviewPending>,
    preview_dirty: bool,
    preview_revision: u64,
    displayed_preview: Option<DisplayedPreview>,
    preview_render_ms: f64,
    preview_report: RenderReport,
    preview_timeline: Arc<Mutex<AnalysisTimeline<ResourceRevision>>>,
    modulator_calibrations: Vec<ModulatorCalibration>,
    calibration_key: Option<CalibrationKey>,
    calibration_pending: Option<CalibrationPending>,
    preview_perf: PreviewPerfTracker,
    export_state: ExportState,
    last_export: Option<ExportOutcome>,
    editor_state: EditorState,
    project_load: ProjectLoadState,
    project_save_pending: Option<ProjectSavePending>,
    document_intent: DocumentIntentState,
    lrc_load: LrcLoadState,
    lrc_save_pending: Option<LrcSavePending>,
    preview_benchmark_target: Option<usize>,
    #[cfg(test)]
    drop_cleanup_observer: Option<Box<dyn FnOnce()>>,
}

impl MusicVideoLabApp {
    pub fn new(assets: Arc<Assets>) -> Self {
        let mut status = if assets.fonts.is_empty() {
            "日本語フォントが見つからないため、時間表示は描画されません".to_owned()
        } else {
            "音声と背景画像を選んでください".to_owned()
        };
        let resource_store = ResourceBlobStore::new();
        let (resource_store, restore_worker) = match resource_store {
            Ok(store) => match ResourceRestoreWorker::new(store.clone()) {
                Ok(worker) => (Some(store), Some(worker)),
                Err(error) => {
                    status = error;
                    (None, None)
                }
            },
            Err(error) => {
                status = error;
                (None, None)
            }
        };
        Self {
            host: startup::Host::default(),
            assets,
            runtime_resources: RuntimeResourceState::Ready(ResourceManifest::default()),
            resource_store,
            restore_worker,
            project: default_project(DEFAULT_DURATION_SECS),
            position_secs: 0.0,
            audio: None,
            waveform: None,
            audio_player: None,
            #[cfg(test)]
            audio_player_factory: AudioPlayer::new_headless,
            playback_error: None,
            audio_error: None,
            document: DocumentSession::default(),
            image_errors: HashMap::new(),
            status,
            show_missing_resources: false,
            show_diagnostics: false,
            last_sent_title: String::new(),
            audio_pending: None,
            image_pending: None,
            preview_pending: None,
            preview_dirty: true,
            preview_revision: 1,
            displayed_preview: None,
            preview_render_ms: 0.0,
            preview_report: RenderReport::default(),
            preview_timeline: Arc::new(Mutex::new(AnalysisTimeline::default())),
            modulator_calibrations: Vec::new(),
            calibration_key: None,
            calibration_pending: None,
            preview_perf: PreviewPerfTracker::default(),
            export_state: ExportState::Idle,
            last_export: None,
            editor_state: EditorState::default(),
            project_load: ProjectLoadState::Idle,
            project_save_pending: None,
            document_intent: DocumentIntentState::Idle,
            lrc_load: LrcLoadState::Idle,
            lrc_save_pending: None,
            preview_benchmark_target: None,
            #[cfg(test)]
            drop_cleanup_observer: None,
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
        let mut manifest = ResourceManifest::default();
        manifest.set_audio(Some(ResourceLease::new(Arc::clone(&audio))));
        let mut player = AudioPlayer::new(
            Arc::clone(&audio),
            manifest.audio().unwrap().revision(),
            0.0,
        )?;
        player.toggle();
        app.editor_state
            .reset_for_project_with_resources(&app.project, manifest.clone());
        app.runtime_resources = RuntimeResourceState::Ready(manifest);
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

    fn schedule_project_edit_preview(&mut self) {
        // A slider can change faster than a full-resolution frame finishes.
        // Let the current complete frame reach the texture, then render the
        // newest project snapshot. Seek and asset replacement still use
        // mark_preview_dirty because their in-flight result is invalid.
        self.preview_dirty = true;
    }

    fn invalidate_modulator_calibration(&mut self) {
        if let Some(pending) = self.calibration_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        self.calibration_key = None;
        self.modulator_calibrations.clear();
    }

    fn current_calibration_key(&self) -> Option<CalibrationKey> {
        self.audio.as_ref()?;
        let pcm_id = self.editor_state.resource_manifest().audio()?.revision();
        Some(CalibrationKey::new(
            pcm_id,
            self.project.duration_secs,
            &modulator_calibration_requests(&self.project.modulators),
        ))
    }

    fn start_calibration_if_needed(&mut self, ctx: &egui::Context) {
        if self.mp4_export_running() {
            return;
        }
        let key = self.current_calibration_key();
        if key == self.calibration_key {
            return;
        }
        if let Some(pending) = self.calibration_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        self.calibration_key = key.clone();
        self.modulator_calibrations.clear();
        let Some(key) = key else {
            return;
        };
        if key.requests.is_empty() {
            return;
        }
        let Some(audio) = self.audio.clone() else {
            return;
        };
        let duration_secs = self.project.duration_secs;
        let modulators = self.project.modulators.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-modulator-calibration".to_owned())
            .spawn(move || {
                let result = pcm_view(&audio.stereo_pcm, audio.sample_rate, duration_secs)
                    .map_err(|error| error.to_string())
                    .and_then(|view| {
                        calibrate_modulators_view(
                            view,
                            audio.sample_rate,
                            duration_secs,
                            audio.stereo_pcm.len() as f64 / 2.0 / audio.sample_rate as f64,
                            &modulators,
                            &|| worker_cancel.load(Ordering::Relaxed),
                        )
                        .map_err(|error| format!("曲全体の反応範囲を計算できません: {error}"))
                    });
                if !worker_cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(result);
                    worker_ctx.request_repaint();
                }
            });
        match spawned {
            Ok(_) => {
                self.status = "曲全体からモジュレータの反応範囲を計算しています".to_owned();
                self.calibration_pending = Some(CalibrationPending { cancel, key, rx });
            }
            Err(error) => {
                self.status = format!("自動調整ワーカーを開始できません: {error}");
            }
        }
    }

    fn poll_calibration(&mut self) {
        let result = self
            .calibration_pending
            .as_ref()
            .map(|pending| pending.rx.try_recv());
        match result {
            Some(Ok(Ok(calibrations))) => {
                let Some(pending) = self.calibration_pending.take() else {
                    return;
                };
                if Some(pending.key) == self.current_calibration_key() {
                    self.modulator_calibrations = calibrations;
                    self.status = "曲全体に合わせて反応範囲を自動調整しました".to_owned();
                    self.mark_preview_dirty();
                }
            }
            Some(Ok(Err(error))) => {
                self.calibration_pending = None;
                self.status = error;
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.calibration_pending = None;
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
    }

    fn calibration_ready(&self) -> bool {
        self.audio.is_none()
            || self.current_calibration_key().is_none_or(|key| {
                key.requests.is_empty()
                    || (self.calibration_key.as_ref() == Some(&key)
                        && self.modulator_calibrations.len() == key.requests.len())
            })
    }

    fn schedule_playback_preview(&mut self) {
        // Playback time advances while the current worker is rendering. Keep that completed
        // frame, then render the newest audio position next. Revisions are reserved for project
        // state changes and seeks that really invalidate an in-flight frame.
        self.preview_dirty = true;
    }

    fn seek_to(&mut self, position_secs: f64) {
        let requested = position_secs.clamp(0.0, self.project.duration_secs.max(0.0));
        self.position_secs = if self.audio_transport_ready() {
            self.audio.as_ref().map_or(requested, |audio| {
                (requested * f64::from(audio.sample_rate))
                    .round()
                    .clamp(0.0, (audio.stereo_pcm.len() / 2) as f64)
                    / f64::from(audio.sample_rate.max(1))
            })
        } else {
            requested
        };
        if self.audio_transport_ready()
            && let Some(player) = &mut self.audio_player
        {
            player.seek(self.position_secs);
        }
        self.mark_preview_dirty();
    }

    fn start_audio_load(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.document.note_edit();
        let request = AudioLoadRequest {
            session: self.document.id(),
            expected: self
                .editor_state
                .resource_manifest()
                .audio()
                .map(ResourceLease::revision),
            path,
        };
        if let Some(pending) = self.audio_pending.as_mut() {
            pending.cancel.store(true, Ordering::Relaxed);
            pending.queued = Some(request);
            self.status = format!(
                "現在の音声読込を取り消して、次の音声を待っています: {}",
                pending.queued.as_ref().unwrap().path.display()
            );
            return;
        }
        self.spawn_audio_load(request, ctx);
    }

    fn spawn_audio_load(&mut self, request: AudioLoadRequest, ctx: &egui::Context) {
        let Some(store) = self.resource_store.clone() else {
            self.status = "素材履歴を利用できないため音声を読み込めません".to_owned();
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let worker_path = request.path.clone();
        let worker_ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-audio-load".to_owned())
            .spawn(move || {
                let result = decode_audio(&worker_path, &worker_cancel)
                    .map_err(|error| {
                        format!("音声を開けません ({}): {error}", worker_path.display())
                    })
                    .and_then(|audio| {
                        let waveform = analyze_waveform(&audio, &worker_cancel)?;
                        let lease = store.store_audio(worker_path.clone(), &audio)?;
                        Ok((worker_path, Arc::new(audio), waveform, lease))
                    });
                if !worker_cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(result);
                    worker_ctx.request_repaint();
                }
            });
        match spawned {
            Ok(_) => {
                self.status = format!("音声を読み込んでいます: {}", request.path.display());
                self.audio_pending = Some(AudioPending {
                    request,
                    cancel,
                    rx,
                    queued: None,
                });
            }
            Err(error) => self.status = format!("音声読み込みワーカーを開始できません: {error}"),
        }
    }

    fn start_image_load(&mut self, id: ImageId, path: PathBuf, ctx: &egui::Context) {
        self.start_image_request(ImageLoadTarget::Replace(id), path, ctx);
    }

    fn start_image_request(&mut self, target: ImageLoadTarget, path: PathBuf, ctx: &egui::Context) {
        self.editor_state.reconcile(&self.project);
        self.document.note_edit();
        let request = ImageLoadRequest {
            session: self.document.id(),
            target_lifetime: match &target {
                ImageLoadTarget::Replace(id) => self.editor_state.image_target_lifetime(id),
                ImageLoadTarget::AddLayer => None,
            },
            target,
            path,
        };
        if matches!(&request.target, ImageLoadTarget::Replace(_))
            && request.target_lifetime.is_none()
        {
            self.status = "画像の差替対象が見つかりません".to_owned();
            return;
        }
        if let Some(pending) = self.image_pending.as_mut() {
            pending.cancel.store(true, Ordering::Relaxed);
            pending.queued = Some(request);
            self.status = format!(
                "現在の画像読込を取り消して、次の画像を待っています: {}",
                pending.queued.as_ref().unwrap().path.display()
            );
            return;
        }
        self.spawn_image_load(request, ctx);
    }

    fn spawn_image_load(&mut self, request: ImageLoadRequest, ctx: &egui::Context) {
        let Some(store) = self.resource_store.clone() else {
            self.status = "素材履歴を利用できないため画像を読み込めません".to_owned();
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let worker_path = request.path.clone();
        let worker_ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-image-load".to_owned())
            .spawn(move || {
                let result = load_image(&worker_path).and_then(|image| {
                    let lease = store.store_image(worker_path.clone(), &image)?;
                    Ok((worker_path, Arc::new(image), lease))
                });
                if !worker_cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(result);
                    worker_ctx.request_repaint();
                }
            });
        match spawned {
            Ok(_) => {
                self.status = match &request.target {
                    ImageLoadTarget::AddLayer => format!(
                        "画像レイヤーを準備しています（完了までは追加されません）: {}",
                        request.path.display()
                    ),
                    ImageLoadTarget::Replace(_) => {
                        format!("画像を読み込んでいます: {}", request.path.display())
                    }
                };
                self.image_pending = Some(ImagePending {
                    request,
                    cancel,
                    rx,
                    queued: None,
                });
            }
            Err(error) => self.status = format!("画像読み込みワーカーを開始できません: {error}"),
        }
    }

    fn poll_loads(&mut self, ctx: &egui::Context) {
        let audio_result = self
            .audio_pending
            .as_ref()
            .map(|pending| pending.rx.try_recv());
        match audio_result {
            Some(Ok(Ok((path, audio, waveform, lease)))) => {
                let pending = self.audio_pending.take().expect("audio request exists");
                let accepted = !pending.cancel.load(Ordering::Relaxed)
                    && self.document.is_current(pending.request.session)
                    && self
                        .editor_state
                        .resource_manifest()
                        .audio()
                        .map(ResourceLease::revision)
                        == pending.request.expected;
                if accepted {
                    self.commit_audio(path, audio, waveform, lease, ctx);
                } else if let Some(store) = &self.resource_store {
                    store.defer_drop((audio, waveform, lease));
                    if !pending.cancel.load(Ordering::Relaxed) {
                        self.status =
                            "対象が変更されたため音声の読込結果を反映しませんでした".to_owned();
                    }
                }
                self.start_queued_audio(pending.queued, ctx);
            }
            Some(Ok(Err(error))) => {
                let pending = self.audio_pending.take().expect("audio request exists");
                if !pending.cancel.load(Ordering::Relaxed) {
                    self.host.notices.push(error.clone());
                    self.status = error;
                }
                self.start_queued_audio(pending.queued, ctx);
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                let queued = self.audio_pending.take().and_then(|pending| pending.queued);
                self.start_queued_audio(queued, ctx);
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }

        let image_result = self
            .image_pending
            .as_ref()
            .map(|pending| pending.rx.try_recv());
        match image_result {
            Some(Ok(Ok((path, image, lease)))) => {
                let pending = self.image_pending.take().expect("image request exists");
                let expected_matches = match &pending.request.target {
                    ImageLoadTarget::Replace(id) => self
                        .editor_state
                        .image_target_lifetime(id)
                        .is_some_and(|lifetime| Some(lifetime) == pending.request.target_lifetime),
                    ImageLoadTarget::AddLayer => true,
                };
                let accepted = !pending.cancel.load(Ordering::Relaxed)
                    && self.document.is_current(pending.request.session)
                    && expected_matches;
                if accepted {
                    self.commit_image(pending.request.target.clone(), path, image, lease, ctx);
                } else if let Some(store) = &self.resource_store {
                    store.defer_drop((image, lease));
                    if !pending.cancel.load(Ordering::Relaxed) {
                        self.status =
                            "対象が変更されたため画像の読込結果を反映しませんでした".to_owned();
                    }
                }
                self.start_queued_image(pending.queued, ctx);
            }
            Some(Ok(Err(error))) => {
                let pending = self.image_pending.take().expect("image request exists");
                if !pending.cancel.load(Ordering::Relaxed) {
                    if let ImageLoadTarget::Replace(id) = &pending.request.target {
                        self.image_errors.insert(id.clone(), error.clone());
                    }
                    self.status = error;
                }
                self.start_queued_image(pending.queued, ctx);
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                let queued = self.image_pending.take().and_then(|pending| pending.queued);
                self.start_queued_image(queued, ctx);
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
    }

    fn start_queued_audio(&mut self, queued: Option<AudioLoadRequest>, ctx: &egui::Context) {
        if let Some(request) = queued {
            self.spawn_audio_load(request, ctx);
        }
    }

    fn start_queued_image(&mut self, queued: Option<ImageLoadRequest>, ctx: &egui::Context) {
        if let Some(request) = queued {
            self.spawn_image_load(request, ctx);
        }
    }

    fn commit_image(
        &mut self,
        target: ImageLoadTarget,
        path: PathBuf,
        image: Arc<ImageAsset>,
        lease: ResourceLease,
        ctx: &egui::Context,
    ) {
        let runtime_was_ready = self.runtime_resources_ready();
        let checkpoint = self.editor_state.begin_external_edit(&self.project);
        let id = match target {
            ImageLoadTarget::Replace(id) => {
                if let Some(reference) = self.project.images.iter_mut().find(|item| item.id == id) {
                    reference.path = path;
                }
                id
            }
            ImageLoadTarget::AddLayer => {
                let id = next_image_id(&self.project);
                let layer = new_image_layer(&self.project, id.clone());
                let insertion_index = image_layer_insertion_index(&self.project);
                self.project.images.push(ImageRef {
                    id: id.clone(),
                    path,
                });
                self.project.layers.insert(insertion_index, layer);
                self.editor_state
                    .select_layer(&self.project, insertion_index);
                self.editor_state.set_inspector_tab(InspectorTab::Layer);
                id
            }
        };
        let mut manifest = self.editor_state.resource_manifest().clone();
        manifest.set_image(id.clone(), lease);
        self.editor_state.finish_external_edit_with_resources(
            &self.project,
            checkpoint,
            manifest.clone(),
        );
        if runtime_was_ready {
            let retained = self
                .project
                .images
                .iter()
                .map(|reference| reference.id.clone())
                .collect::<Vec<_>>();
            let assets = Arc::new(self.assets.fork_retain_images(&retained));
            assets.insert_shared_image(id.clone(), image);
            let old_assets = std::mem::replace(&mut self.assets, assets);
            if let Some(store) = &self.resource_store {
                store.defer_drop(old_assets);
            }
            self.runtime_resources = RuntimeResourceState::Ready(manifest);
        } else {
            if let Some(store) = &self.resource_store {
                store.defer_drop(image);
            }
            self.sync_runtime_resources(ctx);
        }
        self.image_errors.remove(&id);
        self.document.note_edit();
        self.status = "画像を読み込みました".to_owned();
        self.mark_preview_dirty();
    }

    fn commit_audio(
        &mut self,
        path: PathBuf,
        audio: Arc<AudioData>,
        waveform: TimelineWaveform,
        lease: ResourceLease,
        ctx: &egui::Context,
    ) {
        let runtime_was_ready = self.runtime_resources_ready();
        let checkpoint = self.editor_state.begin_external_edit(&self.project);
        self.project.duration_secs = audio.duration_secs;
        self.position_secs = self.position_secs.min(audio.duration_secs);
        self.project.audio = Some(AudioRef { path });
        let mut manifest = self.editor_state.resource_manifest().clone();
        manifest.set_audio(Some(lease));
        self.editor_state.finish_external_edit_with_resources(
            &self.project,
            checkpoint,
            manifest.clone(),
        );
        if runtime_was_ready {
            self.runtime_resources = RuntimeResourceState::Ready(manifest);
            let playback_error = self.install_audio(Some(audio), Some(waveform));
            self.status = playback_error.map_or_else(
                || "音声を読み込みました".to_owned(),
                |error| format!("音声は読み込みましたが再生を開始できません: {error}"),
            );
        } else {
            if let Some(store) = &self.resource_store {
                store.defer_drop((audio, waveform));
            }
            self.sync_runtime_resources(ctx);
        }
        self.audio_error = None;
        self.document.note_edit();
        self.invalidate_modulator_calibration();
        self.mark_preview_dirty();
    }

    fn install_audio(
        &mut self,
        audio: Option<Arc<AudioData>>,
        waveform: Option<TimelineWaveform>,
    ) -> Option<String> {
        let mut playback_error = None;
        let revision = self
            .editor_state
            .resource_manifest()
            .audio()
            .map(ResourceLease::revision);
        match (&mut self.audio_player, &audio) {
            (Some(player), Some(audio)) => {
                player.replace_audio(
                    Some((
                        Arc::clone(audio),
                        revision.expect("adopted PCM has a revision"),
                    )),
                    self.position_secs,
                );
            }
            (None, Some(audio)) => match {
                #[cfg(test)]
                let create_player = self.audio_player_factory;
                #[cfg(not(test))]
                let create_player = AudioPlayer::new;
                create_player(
                    Arc::clone(audio),
                    revision.expect("adopted PCM has a revision"),
                    self.position_secs,
                )
            } {
                Ok(mut player) => {
                    player.set_diagnostics(self.host.persistence.as_ref().map(|p| p.tx.clone()));
                    self.audio_player = Some(player);
                }
                Err(error) => playback_error = Some(error),
            },
            (Some(player), None) => {
                player.replace_audio(None, 0.0);
            }
            (None, None) => {}
        }
        if let Some(old_audio) = std::mem::replace(&mut self.audio, audio)
            && let Some(store) = &self.resource_store
        {
            store.defer_drop(old_audio);
        }
        if let Some(old_waveform) = std::mem::replace(&mut self.waveform, waveform)
            && let Some(store) = &self.resource_store
        {
            store.defer_drop(old_waveform);
        }
        playback_error
    }

    fn cancel_pending_resource_loads(&mut self) {
        if let Some(pending) = self.audio_pending.as_mut() {
            pending.cancel.store(true, Ordering::Relaxed);
            pending.queued = None;
        }
        if let Some(pending) = self.image_pending.as_mut() {
            pending.cancel.store(true, Ordering::Relaxed);
            pending.queued = None;
        }
        self.lrc_load = LrcLoadState::Idle;
    }

    /// Shared production frame boundary: all input and note_edit calls must
    /// precede completion admission. Headless input tests use this same entry.
    fn process_editor_frame(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = &ui.ctx().clone();
        self.poll_host();
        if !self.editing_admitted() {
            egui::CentralPanel::default().show(ui, |ui| {
                if let crate::startup::RuntimeAdmission::Blocked(error) = &*self.host.runtime {
                    ui.heading("音楽ビデオエディタを開始できません");
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            });
            return false;
        }
        self.draw_host_notices(ui);
        self.handle_close_request(ctx);
        let input_enabled = !self.load_choice_open()
            && !self.intent_pending()
            && !self.choosing_project_save()
            && !self.mp4_export_running()
            && !self.export_state.setup_open();
        let mut actions = Vec::new();
        self.draw_status_bar(ui, input_enabled, &mut actions);
        let (response, toolbar_actions) = self.draw_editing_panels(ui, input_enabled);
        actions.extend(toolbar_actions);
        self.draw_tool_windows(ctx, input_enabled, &mut actions);
        actions.extend(self.draw_load_choice(ctx));
        self.draw_mp4_dialogs(ctx, &mut actions);
        self.draw_document_intent(ctx, &mut actions);
        let reset_preview_perf = self.apply_chrome_actions(ctx, actions);
        let add_image_layer_requested = response.add_image_layer_requested;
        let import_lrc_requested = response.import_lrc_requested;
        let export_lrc_requested = response.export_lrc_requested;

        if let Some(layer) = import_lrc_requested
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("時間入り歌詞", &["lrc", "txt"])
                .pick_file()
        {
            self.start_lrc_load(layer, path, ctx);
        }
        if let Some(layer) = export_lrc_requested
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("時間入り歌詞", &["lrc"])
                .set_file_name("lyrics.lrc")
                .save_file()
        {
            self.start_lrc_save(layer, ensure_lrc_extension(path), ctx);
        }

        if add_image_layer_requested
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("画像", &["png", "jpg", "jpeg", "webp", "bmp"])
                .pick_file()
        {
            self.start_image_request(ImageLoadTarget::AddLayer, path, ctx);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ui, |ui| {
                ui.add_enabled_ui(
                    !self.mp4_export_running()
                        && !self.export_state.setup_open()
                        && !self.intent_pending()
                        && !self.choosing_project_save(),
                    |ui| {
                        self.draw_preview(ui);
                    },
                );
            });

        // Project/LRC completion can replace the document or lyrics. Apply the current frame's
        // editor and timeline input first so that an asynchronous completion cannot skip its
        // generation/pending-edit conflict check.
        self.poll_projects(ctx);
        self.poll_lrc(ctx);
        self.poll_project_save_choice(ctx);
        self.poll_mp4_choice(ctx);
        self.sync_runtime_resources(ctx);
        self.finish_document_intent_frame(ctx);
        self.log_status();
        let title = format!("{} - mIV 音楽ビデオ", self.project_caption());
        if title != self.last_sent_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_sent_title = title;
        }
        reset_preview_perf
    }

    /// All editor input and generation updates precede asynchronous completion admission.
    fn draw_editing_panels(
        &mut self,
        ui: &mut egui::Ui,
        input_enabled: bool,
    ) -> (EditorResponse, Vec<ChromeAction>) {
        let ctx = &ui.ctx().clone();
        let toolbar = ToolbarState::from_app(self);
        let mut actions = Vec::new();
        let mut prepared =
            self.editor_state
                .begin_frame_with_input_enabled(ctx, &mut self.project, input_enabled);
        egui::Panel::top("musicvideo-controls").show(ui, |ui| {
            ui.add_enabled_ui(input_enabled, |ui| {
                ui.horizontal_wrapped(|ui| {
                    toolbar.draw(ui, &mut actions);
                    prepared.preview_controls(ui);
                    prepared.history_controls(ui);
                });
            });
        });
        // Toolbar seek input used to run before the timeline. Pass its requested
        // position to both editor regions, and admit any later timeline seek last.
        let input_position_secs = actions
            .iter()
            .rev()
            .find_map(|action| {
                if let ChromeAction::Seek(position) = action {
                    Some(*position)
                } else {
                    None
                }
            })
            .unwrap_or(self.position_secs);
        let timeline_response = egui::Panel::bottom("musicvideo-timeline")
            .resizable(true)
            .default_size(320.0)
            .min_size(150.0)
            .show(ui, |ui| {
                ui.add_enabled_ui(input_enabled, |ui| {
                    prepared.timeline(ui, input_position_secs, self.waveform.as_ref())
                })
                .inner
            })
            .inner;
        let mut frame = prepared.properties();
        egui::Panel::left("musicvideo-layers")
            .resizable(true)
            .default_size(220.0)
            .min_size(180.0)
            .show(ui, |ui| {
                ui.add_enabled_ui(input_enabled, |ui| frame.layer_list(ui));
            });
        egui::Panel::right("musicvideo-properties")
            .resizable(true)
            .default_size(360.0)
            .min_size(300.0)
            .show(ui, |ui| {
                ui.add_enabled_ui(input_enabled, |ui| {
                    frame.inspector_with_project_path(
                        ui,
                        input_position_secs,
                        self.displayed_preview
                            .as_ref()
                            .map_or(&[], |preview| preview.modulator_values.as_slice()),
                        &self.modulator_calibrations,
                        self.document.path(),
                    )
                });
            });
        let response = frame.finish();
        self.handle_editor_response(response);
        if timeline_response.project_changed {
            self.document.note_edit();
        }
        if let Some(position_secs) = timeline_response.seek_to {
            actions.push(ChromeAction::Seek(position_secs));
        } else if timeline_response.project_changed {
            self.schedule_project_edit_preview();
        }

        (response, actions)
    }

    fn handle_editor_response(&mut self, response: EditorResponse) {
        if response.history_changed {
            self.cancel_pending_resource_loads();
        }
        if response.project_changed {
            self.document.note_edit();
            self.schedule_project_edit_preview();
        }
    }

    fn runtime_resources_ready(&self) -> bool {
        matches!(
            &self.runtime_resources,
            RuntimeResourceState::Ready(manifest)
                if manifest == self.editor_state.resource_manifest()
        )
    }

    fn audio_transport_ready(&self) -> bool {
        self.audio.is_some() && self.audio_player.is_some() && self.runtime_resources_ready()
    }

    fn playback_snapshot_for_ui(&self) -> PlaybackSnapshot {
        playback_snapshot_for_ui(
            self.position_secs,
            self.audio_transport_ready(),
            self.audio_player.as_ref().map(AudioPlayer::snapshot),
            self.audio_player
                .as_ref()
                .map_or(0, AudioPlayer::last_operation),
            self.editor_state
                .resource_manifest()
                .audio()
                .map(ResourceLease::revision),
        )
    }

    fn poll_playback_error(&mut self, ctx: &egui::Context) {
        if let Some(player) = &self.audio_player {
            player.set_repaint_context(ctx);
            let error = player.snapshot().error;
            if error != self.playback_error {
                if let Some(error) = &error {
                    self.status = format!("音声を再生できません: {error}");
                }
                self.playback_error = error;
            }
        }
    }

    fn clear_decoded_resources(&mut self) {
        if let Some(player) = &mut self.audio_player {
            player.pause();
        }
        let _ = self.install_audio(None, None);
        let empty_assets = Arc::new(self.assets.fork_retain_images(&[]));
        let old_assets = std::mem::replace(&mut self.assets, empty_assets);
        if let Some(store) = &self.resource_store {
            store.defer_drop(old_assets);
        }
        self.preview_report = RenderReport::default();
        self.displayed_preview = None;
        self.mark_preview_dirty();
    }

    fn sync_runtime_resources(&mut self, ctx: &egui::Context) {
        self.position_secs = self.position_secs.min(self.project.duration_secs.max(0.0));
        let desired = self.editor_state.resource_manifest().clone();
        let already_current = match &self.runtime_resources {
            RuntimeResourceState::Ready(manifest)
            | RuntimeResourceState::Restoring { manifest, .. }
            | RuntimeResourceState::Unresolved { manifest, .. } => manifest == &desired,
        };
        if already_current {
            return;
        }
        self.clear_decoded_resources();
        if desired == ResourceManifest::default() {
            self.runtime_resources = RuntimeResourceState::Ready(desired);
            return;
        }
        let Some(worker) = &mut self.restore_worker else {
            self.runtime_resources = RuntimeResourceState::Unresolved {
                manifest: desired,
                error: "素材履歴の復元ワーカーを利用できません".to_owned(),
            };
            return;
        };
        let generation = worker.request(self.document.id(), desired.clone(), ctx);
        self.runtime_resources = RuntimeResourceState::Restoring {
            manifest: desired,
            generation,
        };
        self.status = "履歴の素材を復元しています".to_owned();
    }

    fn retry_runtime_restore(&mut self, ctx: &egui::Context) {
        let RuntimeResourceState::Unresolved { manifest, .. } = &self.runtime_resources else {
            return;
        };
        let manifest = manifest.clone();
        if self.restore_worker.is_none()
            && let Err(error) = self.restart_restore_worker()
        {
            self.status = error.clone();
            self.runtime_resources = RuntimeResourceState::Unresolved { manifest, error };
            return;
        }
        let Some(worker) = &mut self.restore_worker else {
            return;
        };
        let generation = worker.request(self.document.id(), manifest.clone(), ctx);
        self.runtime_resources = RuntimeResourceState::Restoring {
            manifest,
            generation,
        };
        self.status = "履歴の素材を再び復元しています".to_owned();
    }

    fn poll_resource_restore(&mut self) {
        let mut completions = Vec::new();
        let mut disconnected = false;
        if let Some(worker) = &self.restore_worker {
            loop {
                match worker.rx.try_recv() {
                    Ok(completion) => completions.push(completion),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        for completion in completions {
            self.apply_resource_restore_completion(completion);
        }
        if disconnected {
            let restart_error = self.restart_restore_worker().err();
            if let RuntimeResourceState::Restoring { manifest, .. } = &self.runtime_resources {
                let error = restart_error.unwrap_or_else(|| {
                    "素材履歴の復元ワーカーが終了しました。再試行できます".to_owned()
                });
                self.runtime_resources = RuntimeResourceState::Unresolved {
                    manifest: manifest.clone(),
                    error: error.clone(),
                };
                self.status = error;
            }
        }
    }

    fn restart_restore_worker(&mut self) -> Result<(), String> {
        let Some(store) = self.resource_store.clone() else {
            self.restore_worker = None;
            return Err("素材履歴を利用できないため復元ワーカーを再開できません".to_owned());
        };
        if let Some(worker) = self.restore_worker.take() {
            store.defer_drop(worker);
        }
        self.restore_worker = Some(ResourceRestoreWorker::new(store)?);
        Ok(())
    }

    fn apply_resource_restore_completion(&mut self, completion: RestoreCompletion) {
        let accepted = self.document.is_current(completion.session)
            && matches!(
                &self.runtime_resources,
                RuntimeResourceState::Restoring { manifest, generation }
                    if manifest == &completion.manifest && *generation == completion.generation
            )
            && self.editor_state.resource_manifest() == &completion.manifest;
        if !accepted {
            if let Ok((resources, waveform)) = completion.result
                && let Some(store) = &self.resource_store
            {
                store.defer_drop((resources, waveform));
            }
            return;
        }
        match completion.result {
            Ok((resources, waveform)) => {
                let assets = self.assets.fork_retain_images(&[]);
                for (id, image) in resources.images {
                    assets.insert_shared_image(id.clone(), image);
                    self.image_errors.remove(&id);
                }
                let old_assets = std::mem::replace(&mut self.assets, Arc::new(assets));
                if let Some(store) = &self.resource_store {
                    store.defer_drop(old_assets);
                }
                let playback_error = self.install_audio(resources.audio, waveform);
                if self.editor_state.resource_manifest().audio().is_some() {
                    self.audio_error = None;
                }
                self.runtime_resources = RuntimeResourceState::Ready(resources.manifest);
                self.status = playback_error.map_or_else(
                    || "履歴の素材を復元しました".to_owned(),
                    |error| format!("素材は復元しましたが音声を再生できません: {error}"),
                );
                self.invalidate_modulator_calibration();
                self.mark_preview_dirty();
            }
            Err(error) => {
                self.status = format!("履歴の素材を復元できません: {error}");
                self.runtime_resources = RuntimeResourceState::Unresolved {
                    manifest: completion.manifest,
                    error,
                };
            }
        }
    }

    fn draw_preview(&mut self, ui: &mut egui::Ui) {
        draw_preview(
            ui,
            self.displayed_preview.as_ref().map(DisplayedPreview::frame),
            &mut self.editor_state,
        );
    }

    fn start_preview_if_needed(&mut self, ctx: &egui::Context) {
        if self.mp4_export_running()
            || !self.preview_dirty
            || self.preview_pending.is_some()
            || !self.runtime_resources_ready()
            || !self.calibration_ready()
        {
            return;
        }
        self.preview_dirty = false;
        let revision = self.preview_revision;
        let project = self.project.clone();
        let position_secs = self.position_secs;
        let layer_lifetimes = project
            .layers
            .iter()
            .filter_map(|layer| {
                self.editor_state
                    .layer_target_lifetime(layer.id)
                    .map(|lifetime| (layer.id, lifetime))
            })
            .collect();
        let audio = self.audio.clone();
        let timeline = Arc::clone(&self.preview_timeline);
        let audio_revision = self
            .editor_state
            .resource_manifest()
            .audio()
            .map(ResourceLease::revision);
        let calibrations = self.modulator_calibrations.clone();
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
                let spectrum_frame = (|| {
                    let grid = FrameGrid::new(project.output.fps, project.duration_secs)
                        .map_err(|error| error.to_string())?;
                    let n = grid.index_at(position_secs);
                    let time = grid.time(n);
                    match audio {
                        Some(audio) => {
                            let pcm_id = audio_revision
                                .ok_or_else(|| "音声素材の識別情報がありません".to_owned())?;
                            let view = pcm_view(
                                &audio.stereo_pcm,
                                audio.sample_rate,
                                project.duration_secs,
                            )
                            .map_err(|error| error.to_string())?;
                            let key = AnalysisKey::new(
                                pcm_id,
                                audio.sample_rate,
                                view.logical_frames,
                                grid,
                                band_count,
                                Smoothing::Average { factor: 0.7 },
                                &project.modulators,
                                &calibrations,
                                project.display_gain_db,
                            )
                            .map_err(|error| error.to_string())?;
                            let frame = analyze_preview(&timeline, &key, view, n, &worker_cancel)?;
                            Ok((frame, Some(key), n, time))
                        }
                        None => Ok((SpectrumFrame::new(vec![0.0; band_count]), None, n, time)),
                    }
                })();
                let analysis_ms = analysis_started.elapsed().as_secs_f64() * 1000.0;
                let result = spectrum_frame.and_then(
                    |(spectrum_frame, analysis_key, frame_index, time_secs)| {
                        if worker_cancel.load(Ordering::Relaxed) {
                            return Err("プレビューをキャンセルしました".to_owned());
                        }
                        let scene_composition_started = Instant::now();
                        let modulator_values = spectrum_frame.modulators.clone();
                        let scene = evaluate(&project, time_secs, &spectrum_frame);
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
                            project,
                            layer_lifetimes,
                            time_secs,
                            image,
                            #[cfg(test)]
                            rgba: frame.rgba,
                            render_ms,
                            report,
                            analysis_key,
                            frame_index,
                            modulator_values,
                            timings: PreviewWorkerTimings {
                                analysis_ms,
                                scene_composition_ms,
                                rgba_to_egui_ms,
                                total_ms,
                            },
                        })
                    },
                );
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
                if !self.mp4_export_running() && result.revision == self.preview_revision {
                    let _analysis_identity = (&result.analysis_key, result.frame_index);
                    self.preview_render_ms = result.render_ms;
                    self.preview_report = result.report;
                    let texture_started = Instant::now();
                    let texture = if let Some(mut previous) = self.displayed_preview.take() {
                        previous
                            .texture
                            .set(result.image, egui::TextureOptions::LINEAR);
                        previous.texture
                    } else {
                        ctx.load_texture(
                            "musicvideo-preview",
                            result.image,
                            egui::TextureOptions::LINEAR,
                        )
                    };
                    self.displayed_preview = Some(DisplayedPreview {
                        texture,
                        project: result.project,
                        layer_lifetimes: result.layer_lifetimes,
                        time_secs: result.time_secs,
                        modulator_values: result.modulator_values,
                    });
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
                if !self.mp4_export_running() && !self.preview_dirty {
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
                let _ = tx.send(result.map(ExportCompletion::Png));
                worker_ctx.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.last_export = None;
                self.status = "連番 PNG を書き出しています".to_owned();
                self.export_state = ExportState::Running(ExportPending {
                    kind: ExportKind::Png,
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
            .export_state
            .pending()
            .map(|pending| pending.rx.try_recv());
        match result {
            Some(Ok(Ok(ExportCompletion::Png(outcome)))) => {
                self.export_state = ExportState::Idle;
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
            Some(Ok(Ok(ExportCompletion::Mp4(result)))) => {
                self.complete_mp4_export(result);
            }
            Some(Ok(Err(error))) => {
                self.export_state = ExportState::Idle;
                self.status = error;
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.export_state = ExportState::Idle;
                self.status = "書き出しワーカーが終了しました".to_owned();
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
    }

    fn start_project_load(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.intent_pending() {
            return;
        }
        if let ProjectLoadState::Loading(pending) = &mut self.project_load {
            pending.cancel.store(true, Ordering::Relaxed);
            pending.queued = Some(path.clone());
            self.status = format!(
                "現在のプロジェクト読込を取り消して、次の読込を待っています: {}",
                path.display()
            );
            return;
        }
        if let ProjectLoadState::Ready(loaded) =
            std::mem::replace(&mut self.project_load, ProjectLoadState::Idle)
        {
            self.defer_loaded_project(loaded);
        }
        self.spawn_project_load(path, ctx);
    }

    fn spawn_project_load(&mut self, path: PathBuf, ctx: &egui::Context) {
        let Some(store) = self.resource_store.clone() else {
            self.status = "素材履歴を利用できないためプロジェクトを開けません".to_owned();
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_ctx = ctx.clone();
        let worker_path = path.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-project-load".to_owned())
            .spawn(move || {
                let result = load_project_bundle(worker_path, &store, &worker_cancel);
                if !worker_cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(result);
                    worker_ctx.request_repaint();
                }
            });
        match spawned {
            Ok(_) => {
                self.status = format!("プロジェクトを読み込んでいます: {}", path.display());
                self.project_load = ProjectLoadState::Loading(ProjectLoadPending {
                    started_at: self.document.edit_generation(),
                    cancel,
                    rx,
                    queued: None,
                });
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
        let path = match crate::project_file::absolute_lexical(&path) {
            Ok(path) => path,
            Err(error) => {
                self.status = error;
                self.project_save_failed();
                return;
            }
        };
        let checkpoint = self.document.begin_save(path.clone(), &self.project);
        let project = checkpoint.project().clone();
        let worker_ctx = ctx.clone();
        let worker_path = path.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-project-save".to_owned())
            .spawn(move || {
                let result = save_project(&worker_path, &project);
                let _ = tx.send(result);
                worker_ctx.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.status = format!("プロジェクトを保存しています: {}", path.display());
                self.project_save_pending = Some(ProjectSavePending::Writing(ProjectSaveWrite {
                    checkpoint,
                    rx,
                }));
            }
            Err(error) => {
                self.status = format!("プロジェクト保存ワーカーを開始できません: {error}");
                self.project_save_failed();
            }
        }
    }

    fn poll_projects(&mut self, ctx: &egui::Context) {
        let load_result = match &self.project_load {
            ProjectLoadState::Loading(pending) => Some(pending.rx.try_recv()),
            ProjectLoadState::Idle | ProjectLoadState::Ready(_) => None,
        };
        match load_result {
            Some(Ok(Ok(loaded))) => {
                let pending = match std::mem::take(&mut self.project_load) {
                    ProjectLoadState::Loading(pending) => pending,
                    _ => unreachable!("project load completion requires an active request"),
                };
                if let Some(next_path) = pending.queued {
                    self.defer_loaded_project(loaded);
                    self.spawn_project_load(next_path, ctx);
                } else if project_load_requires_choice(
                    pending.started_at,
                    self.document.edit_generation(),
                    self.pending_project_edit_in_progress(),
                ) {
                    self.status =
                        "読込中に現在のプロジェクトが変更されました。切替方法を選んでください"
                            .to_owned();
                    self.project_load = ProjectLoadState::Ready(loaded);
                    ctx.request_repaint();
                } else {
                    self.apply_loaded_project(loaded);
                }
            }
            Some(Ok(Err(error))) => {
                let pending = match std::mem::take(&mut self.project_load) {
                    ProjectLoadState::Loading(pending) => pending,
                    _ => unreachable!("project load failure requires an active request"),
                };
                if let Some(next_path) = pending.queued {
                    self.spawn_project_load(next_path, ctx);
                } else if !pending.cancel.load(Ordering::Relaxed) {
                    self.host.notices.push(error.clone());
                    self.status = error;
                }
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                let pending = match std::mem::take(&mut self.project_load) {
                    ProjectLoadState::Loading(pending) => pending,
                    _ => unreachable!("project load disconnect requires an active request"),
                };
                if let Some(next_path) = pending.queued {
                    self.spawn_project_load(next_path, ctx);
                } else if !pending.cancel.load(Ordering::Relaxed) {
                    self.status = "プロジェクト読込ワーカーが終了しました".to_owned();
                }
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }

        let save_result = match &self.project_save_pending {
            Some(ProjectSavePending::Writing(pending)) => Some(pending.rx.try_recv()),
            _ => None,
        };
        match save_result {
            Some(Ok(Ok(()))) => {
                let ProjectSavePending::Writing(pending) = self
                    .project_save_pending
                    .take()
                    .expect("the completed save request still exists")
                else {
                    unreachable!()
                };
                let path = pending.checkpoint.path().to_path_buf();
                self.remember_project(path.clone());
                if self.document.complete_save(pending.checkpoint) {
                    self.status = format!("プロジェクトを保存しました: {}", path.display());
                } else {
                    self.status = format!(
                        "切替前のプロジェクトを保存しました（現在の保存先は変更していません）: {}",
                        path.display()
                    );
                }
            }
            Some(Ok(Err(error))) => {
                self.project_save_pending = None;
                self.status = error;
                self.project_save_failed();
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.project_save_pending = None;
                self.status = "プロジェクト保存ワーカーが終了しました".to_owned();
                self.project_save_failed();
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
    }

    fn pending_project_edit_in_progress(&self) -> bool {
        self.audio_pending.is_some()
            || self.image_pending.is_some()
            || self.lrc_load.is_active()
            || self.editor_state.has_pending_lyrics_edit()
    }

    fn apply_loaded_project(&mut self, loaded: LoadedProject) {
        self.remember_project(loaded.path.clone());
        self.document_intent = DocumentIntentState::Idle;
        self.cancel_pending_resource_loads();
        if let Some(worker) = &mut self.restore_worker {
            worker.cancel();
        }
        let assets = self.assets.fork_retain_images(&[]);
        for (id, image) in loaded.images {
            assets.insert_shared_image(id, image);
        }
        let old_assets = std::mem::replace(&mut self.assets, Arc::new(assets));
        if let Some(store) = &self.resource_store {
            store.defer_drop(old_assets);
        }
        self.displayed_preview = None;
        self.project = loaded.project;
        self.document
            .replace_with_loaded(loaded.path, &self.project);
        self.position_secs = 0.0;
        self.editor_state
            .reset_for_project_with_resources(&self.project, loaded.resources.clone());
        self.runtime_resources = RuntimeResourceState::Ready(loaded.resources);
        let playback_error = self.install_audio(loaded.audio, loaded.waveform);
        self.audio_error = loaded.audio_error.clone();
        self.image_errors = loaded.image_errors;
        self.preview_report = RenderReport::default();
        self.invalidate_modulator_calibration();
        let issue_count = self.image_errors.len() + usize::from(loaded.audio_error.is_some());
        if let Some(error) = loaded.audio_error {
            self.status = format!("プロジェクトを開きました。音声素材の問題: {error}");
        } else if issue_count > 0 {
            self.status =
                format!("プロジェクトを開きました。未解決素材が {issue_count} 件あります");
        } else if let Some(error) = playback_error {
            self.status = format!("プロジェクトは開きましたが音声を再生できません: {error}");
        } else if self.audio_player.is_some() || self.audio.is_none() {
            self.status = "プロジェクトを開きました".to_owned();
        }
        self.mark_preview_dirty();
    }

    fn defer_loaded_project(&self, loaded: LoadedProject) {
        if let Some(store) = &self.resource_store {
            store.defer_drop(loaded);
        }
    }

    fn start_lrc_load(
        &mut self,
        layer: musicvideo_core::LayerId,
        path: PathBuf,
        ctx: &egui::Context,
    ) {
        if self.lrc_save_pending.is_some() {
            self.status = "LRC の書出中は新しい LRC を読み込めません".to_owned();
            return;
        }
        self.editor_state.reconcile(&self.project);
        let Some(target_lifetime) = self.editor_state.layer_target_lifetime(layer) else {
            self.status = "LRC の読込対象が見つかりません".to_owned();
            return;
        };
        let baseline = self
            .project
            .layers
            .iter()
            .find(|candidate| candidate.id == layer)
            .and_then(|layer| match &layer.kind {
                LayerKind::Lyrics(lyrics) => Some(lyrics.lines.clone()),
                _ => None,
            })
            .unwrap_or_default();
        self.lrc_load = LrcLoadState::Idle;
        let worker_ctx = ctx.clone();
        let worker_path = path.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-lrc-load".to_owned())
            .spawn(move || {
                let result = std::fs::read(&worker_path)
                    .map_err(|error| {
                        format!("LRC を読み込めません ({}): {error}", worker_path.display())
                    })
                    .and_then(|bytes| {
                        parse_lrc(&bytes)
                            .map(|report| (worker_path.clone(), report))
                            .map_err(|error| {
                                format!("LRC を解釈できません ({}): {error}", worker_path.display())
                            })
                    });
                let _ = tx.send(result);
                worker_ctx.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.document.note_edit();
                self.status = format!("LRC を読み込んでいます: {}", path.display());
                self.lrc_load = LrcLoadState::Loading {
                    owner: LrcLoadOwner {
                        session: self.document.id(),
                        layer,
                        target_lifetime,
                        baseline,
                    },
                    rx,
                };
            }
            Err(error) => self.status = format!("LRC 読込ワーカーを開始できません: {error}"),
        }
    }

    fn start_lrc_save(
        &mut self,
        layer: musicvideo_core::LayerId,
        path: PathBuf,
        ctx: &egui::Context,
    ) {
        if self.lrc_load.is_active() || self.lrc_save_pending.is_some() {
            return;
        }
        let Some(lines) = self
            .project
            .layers
            .iter()
            .find(|candidate| candidate.id == layer)
            .and_then(|layer| match &layer.kind {
                LayerKind::Lyrics(lyrics) => Some(lyrics.lines.clone()),
                _ => None,
            })
        else {
            self.status = "LRC の書き出し対象が見つかりません".to_owned();
            return;
        };
        let worker_ctx = ctx.clone();
        let worker_path = path.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-lrc-save".to_owned())
            .spawn(move || {
                let (text, report) = write_lrc(&lines);
                let result = std::fs::write(&worker_path, text.as_bytes())
                    .map(|()| (worker_path.clone(), report.skipped_unset_lines))
                    .map_err(|error| {
                        format!("LRC を書き出せません ({}): {error}", worker_path.display())
                    });
                let _ = tx.send(result);
                worker_ctx.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.status = format!("LRC を書き出しています: {}", path.display());
                self.lrc_save_pending = Some(LrcSavePending { rx });
            }
            Err(error) => self.status = format!("LRC 書出ワーカーを開始できません: {error}"),
        }
    }

    fn current_lyrics(&self, layer: musicvideo_core::LayerId) -> Option<Vec<LyricLine>> {
        self.project
            .layers
            .iter()
            .find(|candidate| candidate.id == layer)
            .and_then(|layer| match &layer.kind {
                LayerKind::Lyrics(lyrics) => Some(lyrics.lines.clone()),
                _ => None,
            })
    }

    fn apply_lrc_result(
        &mut self,
        owner: LrcLoadOwner,
        path: PathBuf,
        report: LrcParseReport,
        ctx: &egui::Context,
    ) {
        if !self.document.is_current(owner.session)
            || self.editor_state.layer_target_lifetime(owner.layer) != Some(owner.target_lifetime)
        {
            self.status =
                "対象レイヤーが変更されたため LRC の読込結果を反映しませんでした".to_owned();
            return;
        }
        let notice = format!(
            "{} として読込 / {} 行 / 読み飛ばし: メタ情報 {}、未対応 {}、行内時刻 {} / offset {:+} ms",
            report.encoding.label(),
            report.lines.len(),
            report.skipped_metadata_tags,
            report.skipped_unsupported_tags,
            report.inline_time_tags_removed,
            report.offset_millis,
        );
        let command = EditorCommand::ReplaceLyrics {
            layer: owner.layer,
            lines: report.lines,
        };
        match self.editor_state.try_apply_external_command_with_context(
            ctx,
            &mut self.project,
            &command,
        ) {
            Ok(true) => {
                self.document.note_edit();
                self.editor_state
                    .set_lyrics_notice(owner.layer, notice.clone());
                self.status = format!("LRC を読み込みました ({}): {notice}", path.display());
                self.schedule_project_edit_preview();
            }
            Ok(false) => {
                self.status = "LRC の内容は現在の歌詞と同じです".to_owned();
            }
            Err(error) => self.status = format!("LRC を反映できません: {error}"),
        }
    }

    fn poll_lrc(&mut self, ctx: &egui::Context) {
        let load_result = match &self.lrc_load {
            LrcLoadState::Loading { rx, .. } => Some(rx.try_recv()),
            LrcLoadState::Idle | LrcLoadState::Ready { .. } => None,
        };
        match load_result {
            Some(Ok(Ok((path, report)))) => {
                let owner = match std::mem::take(&mut self.lrc_load) {
                    LrcLoadState::Loading { owner, .. } => owner,
                    _ => unreachable!("LRC completion requires an active request"),
                };
                if !self.document.is_current(owner.session)
                    || self.editor_state.layer_target_lifetime(owner.layer)
                        != Some(owner.target_lifetime)
                {
                    self.status = "対象レイヤーが変更されたため LRC の読込結果を反映しませんでした"
                        .to_owned();
                    return;
                }
                if self.current_lyrics(owner.layer).as_ref() == Some(&owner.baseline)
                    && !self.editor_state.layer_has_pending_lyrics_edit(owner.layer)
                {
                    self.apply_lrc_result(owner, path, report, ctx);
                } else {
                    self.status =
                        "LRC 読込中に対象の歌詞が変更されました。反映方法を選んでください"
                            .to_owned();
                    self.lrc_load = LrcLoadState::Ready {
                        owner,
                        path,
                        report,
                    };
                    ctx.request_repaint();
                }
            }
            Some(Ok(Err(error))) => {
                self.lrc_load = LrcLoadState::Idle;
                self.status = error;
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.lrc_load = LrcLoadState::Idle;
                self.status = "LRC 読込ワーカーが終了しました".to_owned();
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }

        let save_result = self
            .lrc_save_pending
            .as_ref()
            .map(|pending| pending.rx.try_recv());
        match save_result {
            Some(Ok(Ok((path, skipped)))) => {
                self.lrc_save_pending = None;
                self.status = format!(
                    "LRC を UTF-8 で書き出しました: {} (時刻未設定 {} 行は出力しませんでした)",
                    path.display(),
                    skipped
                );
            }
            Some(Ok(Err(error))) => {
                self.lrc_save_pending = None;
                self.status = error;
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.lrc_save_pending = None;
                self.status = "LRC 書出ワーカーが終了しました".to_owned();
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
    }
}

pub(crate) fn load_project_bundle(
    path: PathBuf,
    store: &ResourceBlobStore,
    cancel: &AtomicBool,
) -> Result<LoadedProject, String> {
    let path = crate::project_file::absolute_lexical(&path)?;
    let project = load_project(&path)?;
    let mut images = Vec::new();
    let mut resources = ResourceManifest::default();
    let mut image_errors = HashMap::new();
    for reference in &project.images {
        if cancel.load(Ordering::Relaxed) {
            return Err("プロジェクトの読み込みをキャンセルしました".to_owned());
        }
        match load_image(&reference.path).and_then(|image| {
            let lease = store.store_image(reference.path.clone(), &image)?;
            Ok((Arc::new(image), lease))
        }) {
            Ok((image, lease)) => {
                resources.set_image(reference.id.clone(), lease);
                images.push((reference.id.clone(), image));
            }
            Err(error) => {
                image_errors.insert(reference.id.clone(), error);
            }
        }
    }
    let (audio, waveform, audio_error, audio_lease) = match &project.audio {
        Some(reference) => match decode_audio(&reference.path, cancel) {
            Ok(audio) => match analyze_waveform(&audio, cancel) {
                Ok(waveform) => match store.store_audio(reference.path.clone(), &audio) {
                    Ok(lease) => (Some(Arc::new(audio)), Some(waveform), None, Some(lease)),
                    Err(error) => (None, None, Some(error), None),
                },
                Err(error) => (None, None, Some(error), None),
            },
            Err(error) => (None, None, Some(error), None),
        },
        None => (None, None, None, None),
    };
    resources.set_audio(audio_lease);
    Ok(LoadedProject {
        path,
        project,
        audio,
        waveform,
        images,
        resources,
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

fn ensure_lrc_extension(mut path: PathBuf) -> PathBuf {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lrc"))
    {
        path.set_extension("lrc");
    }
    path
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        if !self.editing_admitted() {
            self.process_editor_frame(ui);
            return;
        }
        let update_started = Instant::now();
        self.poll_loads(ctx);
        self.poll_resource_restore();
        self.poll_calibration();
        let mut displayed_perf = self.poll_preview(ctx);
        self.poll_export();

        let audio_sync_started = Instant::now();
        self.poll_playback_error(ctx);
        let playback = self.playback_snapshot_for_ui();
        if self.audio_transport_ready()
            && (playback.position_secs - self.position_secs).abs() > 0.000_001
        {
            self.position_secs = playback.position_secs.min(self.project.duration_secs);
            self.schedule_playback_preview();
        }
        let audio_sync_ms = audio_sync_started.elapsed().as_secs_f64() * 1000.0;
        let egui_layout_started = Instant::now();
        let reset_preview_perf = self.process_editor_frame(ui);

        let egui_layout_ms = egui_layout_started.elapsed().as_secs_f64() * 1000.0;

        self.start_calibration_if_needed(ctx);
        self.start_preview_if_needed(ctx);
        if self.audio_pending.is_some()
            || self.image_pending.is_some()
            || self.preview_pending.is_some()
            || self.export_state.pending().is_some()
            || self.project_load.is_loading()
            || self.project_save_pending.is_some()
            || self.lrc_load.is_active()
            || self.lrc_save_pending.is_some()
            || self.calibration_pending.is_some()
            || matches!(
                self.runtime_resources,
                RuntimeResourceState::Restoring { .. }
            )
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
        self.finish_preview_benchmark(ctx);
    }
}

impl MusicVideoLabApp {
    fn finish_preview_benchmark(&mut self, ctx: &egui::Context) {
        if self
            .preview_benchmark_target
            .is_some_and(|target| self.preview_perf.total_samples >= target)
        {
            if let Some(perf) = self.preview_perf.summary() {
                let text = format!(
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
                self.send_host(crate::persistence::Command::Print(text));
            }
            self.preview_benchmark_target = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn playback_snapshot_for_ui(
    current_position_secs: f64,
    audio_ready: bool,
    player_snapshot: Option<PlaybackSnapshot>,
    last_operation: u64,
    revision: Option<ResourceRevision>,
) -> PlaybackSnapshot {
    if audio_ready
        && let Some(snapshot) = player_snapshot
        && snapshot.applied_operation >= last_operation
        && snapshot.resource_revision == revision
    {
        snapshot
    } else {
        PlaybackSnapshot {
            position_secs: current_position_secs,
            playing: false,
            ..Default::default()
        }
    }
}

fn project_load_requires_choice(
    started_at: EditGeneration,
    current: EditGeneration,
    has_pending_edit: bool,
) -> bool {
    current != started_at || has_pending_edit
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
        if let Some(pending) = &self.calibration_pending {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(pending) = self.export_state.pending() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        if let ProjectLoadState::Loading(pending) = &self.project_load {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(worker) = &mut self.restore_worker {
            worker.cancel();
        }
        if let Some(store) = self.resource_store.clone() {
            if let Some(pending) = self.audio_pending.take() {
                store.defer_drop(pending);
            }
            if let Some(pending) = self.image_pending.take() {
                store.defer_drop(pending);
            }
            if let Some(pending) = self.preview_pending.take() {
                store.defer_drop(pending);
            }
            if let Some(pending) = self.calibration_pending.take() {
                store.defer_drop(pending);
            }
            if let Some(worker) = self.restore_worker.take() {
                store.defer_drop(worker);
            }
            let _ = self.install_audio(None, None);
            let empty_assets = Arc::new(self.assets.fork_retain_images(&[]));
            let old_assets = std::mem::replace(&mut self.assets, empty_assets);
            let old_editor = std::mem::take(&mut self.editor_state);
            let old_timeline = std::mem::replace(
                &mut self.preview_timeline,
                Arc::new(Mutex::new(AnalysisTimeline::default())),
            );
            let old_runtime = std::mem::replace(
                &mut self.runtime_resources,
                RuntimeResourceState::Ready(ResourceManifest::default()),
            );
            match std::mem::take(&mut self.project_load) {
                ProjectLoadState::Loading(pending) => store.defer_drop(pending),
                ProjectLoadState::Ready(loaded) => store.defer_drop(loaded),
                ProjectLoadState::Idle => {}
            }
            store.defer_drop((old_assets, old_editor, old_runtime, old_timeline));
        }
        #[cfg(test)]
        if let Some(observer) = self.drop_cleanup_observer.take() {
            observer();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn h3a_app() -> MusicVideoLabApp {
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        app.project = default_project(0.5);
        app.project.output.width = 16;
        app.project.output.height = 16;
        app.project.output.fps = 60.0;
        app.project.layers.clear();
        app.project.audio = Some(AudioRef {
            path: PathBuf::from("immutable-test-pcm.wav"),
        });
        let audio = Arc::new(AudioData {
            stereo_pcm: vec![0.1; 8_000],
            sample_rate: 8_000,
            duration_secs: 0.5,
        });
        let mut manifest = ResourceManifest::default();
        manifest.set_audio(Some(ResourceLease::new(Arc::clone(&audio))));
        app.editor_state
            .reset_for_project_with_resources(&app.project, manifest.clone());
        app.runtime_resources = RuntimeResourceState::Ready(manifest);
        app.audio = Some(audio);
        app
    }

    fn h3a_request(app: &mut MusicVideoLabApp, ctx: &egui::Context) -> PreviewResult {
        app.preview_dirty = true;
        app.start_preview_if_needed(ctx);
        let result = app
            .preview_pending
            .as_ref()
            .expect("admitted preview")
            .rx
            .recv_timeout(Duration::from_secs(20))
            .unwrap()
            .unwrap();
        app.preview_pending = None;
        result
    }

    #[test]
    fn output_fps_edit_and_undo_replace_preview_analysis_key() {
        let ctx = egui::Context::default();
        let mut app = h3a_app();
        app.position_secs = 0.205;
        let initial = h3a_request(&mut app, &ctx);
        let initial_key = initial.analysis_key.unwrap();
        assert_eq!(initial.frame_index, 12);
        for fps in [24.0, 30.0] {
            let checkpoint = app.editor_state.begin_external_edit(&app.project);
            app.project.output.fps = fps;
            assert!(
                app.editor_state
                    .finish_external_edit(&app.project, checkpoint)
            );
            app.handle_editor_response(EditorResponse {
                project_changed: true,
                ..Default::default()
            });
            let changed = h3a_request(&mut app, &ctx);
            let key = changed.analysis_key.unwrap();
            assert!(key != initial_key);
            let grid = FrameGrid::new(fps, 0.5).unwrap();
            assert_eq!(changed.frame_index, grid.index_at(0.205));
            assert_eq!(changed.time_secs, grid.time(changed.frame_index));
            assert!(app.preview_timeline.lock().unwrap().key() == Some(&key));
            assert!(app.editor_state.undo(&mut app.project));
            app.handle_editor_response(EditorResponse {
                project_changed: true,
                history_changed: true,
                ..Default::default()
            });
            let restored = h3a_request(&mut app, &ctx);
            assert!(restored.analysis_key.as_ref() == Some(&initial_key));
            assert_eq!(restored.frame_index, 12);
        }
    }

    fn h3a_spectrum_bits(frame: &SpectrumFrame) -> Vec<u32> {
        let mut bits = vec![
            u32::from(frame.note_min_midi),
            u32::from(frame.keyboard.note_min_midi),
        ];
        bits.extend(frame.bands.iter().chain(&frame.notes).map(|v| v.to_bits()));
        for level in &frame.keyboard.levels {
            bits.extend([
                level.center.to_bits(),
                level.even.to_bits(),
                level.odd.to_bits(),
            ]);
        }
        for band in &frame.miv_spectrum.bands {
            bits.extend([
                band.value.to_bits(),
                band.trail.to_bits(),
                band.onset.to_bits(),
            ]);
        }
        for value in &frame.modulators {
            bits.extend([
                value.id.0 as u32,
                (value.id.0 >> 32) as u32,
                value.value.to_bits(),
            ]);
        }
        bits
    }

    #[test]
    fn h3a_drop_moves_last_timeline_owner_to_cleanup_before_fields_drop() {
        struct CleanupProbe {
            original: std::sync::Weak<Mutex<AnalysisTimeline<ResourceRevision>>>,
            tx: mpsc::Sender<(usize, String)>,
        }
        impl Drop for CleanupProbe {
            fn drop(&mut self) {
                self.tx
                    .send((
                        self.original.strong_count(),
                        std::thread::current().name().unwrap_or("").to_owned(),
                    ))
                    .unwrap();
            }
        }

        let mut app = h3a_app();
        // Populate the original allocation synchronously, with no worker Arc.
        let audio = app.audio.as_ref().unwrap();
        let pcm = pcm_view(&audio.stereo_pcm, audio.sample_rate, 0.5).unwrap();
        let key = AnalysisKey::new(
            app.editor_state
                .resource_manifest()
                .audio()
                .unwrap()
                .revision(),
            audio.sample_rate,
            pcm.logical_frames,
            FrameGrid::new(60.0, 0.5).unwrap(),
            16,
            Smoothing::default(),
            &[],
            &[],
            0.0,
        )
        .unwrap();
        app.preview_timeline
            .lock()
            .unwrap()
            .frame(&key, pcm, 29, &|| false)
            .unwrap();
        assert_eq!(Arc::strong_count(&app.preview_timeline), 1);
        let original = Arc::downgrade(&app.preview_timeline);
        let store = app.resource_store.clone().unwrap();
        app.drop_cleanup_observer = Some(Box::new(move || {
            // FIFO cleanup completes while Drop still holds all app fields.
            // With a cloned owner, the original field remains the last Arc here.
            let (tx, rx) = mpsc::channel();
            store.defer_drop(CleanupProbe { original, tx });
            let (remaining, thread) = rx.recv_timeout(Duration::from_secs(20)).unwrap();
            assert_eq!(thread, "musicvideo-resource-cleanup");
            assert_eq!(
                remaining, 0,
                "the original allocation must be released by cleanup before app fields drop"
            );
        }));
        drop(app);
    }

    #[test]
    fn h3a_audio_commit_undo_restores_through_host_handlers_and_rejects_stale_completion() {
        let ctx = egui::Context::default();
        let mut app = h3a_app();
        app.audio_player_factory = AudioPlayer::new_headless;
        let store = app.resource_store.clone().unwrap();
        let initial_audio = app.audio.clone().unwrap();
        let mut initial_manifest = ResourceManifest::default();
        initial_manifest.set_audio(Some(
            store
                .store_audio(PathBuf::from("original.wav"), &initial_audio)
                .unwrap(),
        ));
        app.editor_state
            .reset_for_project_with_resources(&app.project, initial_manifest.clone());
        app.runtime_resources = RuntimeResourceState::Ready(initial_manifest.clone());

        // Control only completion scheduling; request(), cancel(), and polling
        // remain the real resource-restore host path.
        let (restore_tx, restore_rx) = mpsc::channel();
        app.restore_worker = Some(ResourceRestoreWorker {
            queue: Arc::new(RestoreQueue::default()),
            rx: restore_rx,
            next_generation: 1,
            active_cancel: None,
        });
        let take_request = |app: &mut MusicVideoLabApp| {
            app.restore_worker
                .as_ref()
                .unwrap()
                .queue
                .state
                .lock()
                .unwrap()
                .latest
                .take()
                .unwrap()
                .0
        };
        // Model a completed decode whose delivery can be delayed past cancellation.
        let complete = |request: RestoreRequest| RestoreCompletion {
            generation: request.generation,
            session: request.session,
            manifest: request.manifest.clone(),
            result: Ok((
                restore_manifest(&store, request.manifest, &AtomicBool::new(false)).unwrap(),
                None,
            )),
        };
        let owner = Arc::clone(&app.preview_timeline);
        app.position_secs = 0.205;
        let initial = h3a_request(&mut app, &ctx);
        let initial_key = initial.analysis_key.clone().unwrap();
        let initial_pcm = pcm_view(
            &initial_audio.stereo_pcm,
            initial_audio.sample_rate,
            app.project.duration_secs,
        )
        .unwrap();
        let original_bits = h3a_spectrum_bits(
            owner
                .lock()
                .unwrap()
                .frame(&initial_key, initial_pcm, initial.frame_index, &|| false)
                .unwrap(),
        );

        let replacement = Arc::new(AudioData {
            stereo_pcm: (0..8_000)
                .map(|n| ((n / 2) as f32 * 0.19).sin() * 0.7)
                .collect(),
            sample_rate: 8_000,
            duration_secs: 0.5,
        });
        let lease = store
            .store_audio(PathBuf::from("replacement.wav"), &replacement)
            .unwrap();
        app.commit_audio(
            PathBuf::from("replacement.wav"),
            replacement.clone(),
            TimelineWaveform::new(0.5, vec![[0.7, 0.7]]),
            lease,
            &ctx,
        );
        assert!(app.runtime_resources_ready());
        assert!(Arc::ptr_eq(app.audio.as_ref().unwrap(), &replacement));
        assert!(app.audio_player.is_some());
        let replacement_playback = app.audio_player.as_ref().unwrap().wait_applied();
        assert_eq!(
            replacement_playback.resource_revision,
            app.editor_state
                .resource_manifest()
                .audio()
                .map(ResourceLease::revision)
        );
        assert!(!replacement_playback.playing);
        assert_eq!(replacement_playback.position_secs, app.position_secs);
        assert!(Arc::ptr_eq(&owner, &app.preview_timeline));
        let replacement_result = h3a_request(&mut app, &ctx);
        let replacement_key = replacement_result.analysis_key.clone().unwrap();
        assert!(
            replacement_key != initial_key,
            "new PCM must reject the old analysis key"
        );
        assert!(owner.lock().unwrap().key() == Some(&replacement_key));
        let replacement_pcm =
            pcm_view(&replacement.stereo_pcm, replacement.sample_rate, 0.5).unwrap();
        assert_ne!(
            original_bits,
            h3a_spectrum_bits(
                owner
                    .lock()
                    .unwrap()
                    .frame(&replacement_key, replacement_pcm, 12, &|| false)
                    .unwrap()
            )
        );

        assert!(app.editor_state.undo(&mut app.project));
        app.handle_editor_response(EditorResponse {
            project_changed: true,
            history_changed: true,
            ..EditorResponse::default()
        });
        app.sync_runtime_resources(&ctx);
        assert!(matches!(
            app.runtime_resources,
            RuntimeResourceState::Restoring { .. }
        ));
        assert!(
            app.audio.is_none(),
            "Undo must clear the replaced decoded PCM"
        );
        assert!(
            Arc::ptr_eq(&owner, &app.preview_timeline),
            "Undo resource clearing must preserve the session owner"
        );
        assert!(owner.lock().unwrap().key() == Some(&replacement_key));
        let stale_request = take_request(&mut app);

        // Redo then Undo supersedes an in-flight restore for the same manifest.
        assert!(app.editor_state.redo(&mut app.project));
        app.handle_editor_response(EditorResponse {
            project_changed: true,
            history_changed: true,
            ..EditorResponse::default()
        });
        app.sync_runtime_resources(&ctx);
        let superseded = take_request(&mut app);
        assert!(stale_request.cancel.load(Ordering::Relaxed));
        assert!(app.editor_state.undo(&mut app.project));
        app.handle_editor_response(EditorResponse {
            project_changed: true,
            history_changed: true,
            ..EditorResponse::default()
        });
        app.sync_runtime_resources(&ctx);
        let current_request = take_request(&mut app);
        let current_generation = current_request.generation;
        assert!(superseded.cancel.load(Ordering::Relaxed));
        assert_eq!(current_request.manifest, initial_manifest);
        restore_tx.send(complete(stale_request)).unwrap();
        app.poll_resource_restore();
        assert!(
            matches!(app.runtime_resources, RuntimeResourceState::Restoring { generation, .. } if generation == current_generation)
        );
        assert!(
            app.audio.is_none(),
            "stale same-manifest completion must not install PCM"
        );
        restore_tx.send(complete(current_request)).unwrap();
        app.poll_resource_restore();
        assert!(app.runtime_resources_ready());
        assert_eq!(app.editor_state.resource_manifest(), &initial_manifest);
        let restored_playback = app.audio_player.as_ref().unwrap().wait_applied();
        assert_eq!(
            restored_playback.resource_revision,
            initial_manifest.audio().map(ResourceLease::revision)
        );
        assert!(!restored_playback.playing);
        assert!(restored_playback.applied_operation > replacement_playback.applied_operation);
        assert!(Arc::ptr_eq(&owner, &app.preview_timeline));
        assert!(
            owner.lock().unwrap().key() == Some(&replacement_key),
            "restore completion must leave analysis rebuilding to the next request"
        );
        selection_deliver(&mut app, &ctx, replacement_result);
        assert!(
            app.displayed_preview.is_none(),
            "old PCM preview must not be adopted after Undo"
        );
        let restored = h3a_request(&mut app, &ctx);
        assert!(restored.analysis_key.as_ref() == Some(&initial_key));
        let export_request = crate::mp4::snapshot(
            &app.project,
            &app.assets,
            Arc::clone(app.audio.as_ref().unwrap()),
            app.editor_state
                .resource_manifest()
                .audio()
                .unwrap()
                .revision(),
            PathBuf::from("unused-undo-export.mp4"),
            crate::mp4::ExportSettings::default(),
            false,
        );
        assert_eq!(
            export_request.pcm_id,
            initial_manifest.audio().unwrap().revision()
        );
        assert_eq!(export_request.audio.stereo_pcm, initial_audio.stereo_pcm);
        assert!(Arc::ptr_eq(
            &export_request.audio,
            app.audio.as_ref().unwrap()
        ));
        assert!(owner.lock().unwrap().key() != Some(&replacement_key));
        let restored_bits = h3a_spectrum_bits(
            owner
                .lock()
                .unwrap()
                .frame(&initial_key, initial_pcm, restored.frame_index, &|| false)
                .unwrap(),
        );
        let mut reference = AnalysisTimeline::default();
        let mut reference_bits = Vec::new();
        for n in 0..=restored.frame_index {
            reference_bits = h3a_spectrum_bits(
                reference
                    .frame(&initial_key, initial_pcm, n, &|| false)
                    .unwrap(),
            );
        }
        assert_eq!(restored_bits, original_bits);
        assert_eq!(restored_bits, reference_bits);
        assert_eq!(restored.image.pixels, initial.image.pixels);
    }

    #[test]
    fn h3a_cancelled_worker_keeps_admission_until_disconnect() {
        let ctx = egui::Context::default();
        let mut app = h3a_app();
        app.position_secs = 0.14;
        h3a_request(&mut app, &ctx);
        assert_eq!(
            app.preview_timeline.lock().unwrap().processed_frame(),
            Some(8)
        );
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        app.preview_pending = Some(PreviewPending {
            cancel: Arc::clone(&cancel),
            rx,
        });
        app.mark_preview_dirty();
        app.start_preview_if_needed(&ctx);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(app.preview_pending.as_ref().unwrap().rx.try_recv().is_err());
        app.poll_preview(&ctx);
        assert!(
            app.preview_pending.is_some(),
            "cancel notification is not completion"
        );
        assert_eq!(
            app.preview_timeline.lock().unwrap().processed_frame(),
            Some(8)
        );
        app.start_preview_if_needed(&ctx);
        assert!(app.preview_dirty, "new worker must remain coalesced");
        drop(tx);
        app.poll_preview(&ctx);
        assert!(app.preview_pending.is_none());
        app.position_secs = 0.0;
        let result = h3a_request(&mut app, &ctx);
        assert_eq!(result.frame_index, 0);
    }

    #[test]
    fn h3a_cancel_after_lock_wait_does_not_replace_key_or_progress() {
        let app = h3a_app();
        let audio = app.audio.as_ref().unwrap();
        let pcm = pcm_view(
            &audio.stereo_pcm,
            audio.sample_rate,
            app.project.duration_secs,
        )
        .unwrap();
        let id = app
            .editor_state
            .resource_manifest()
            .audio()
            .unwrap()
            .revision();
        let key = AnalysisKey::new(
            id,
            audio.sample_rate,
            pcm.logical_frames,
            FrameGrid::new(60.0, 0.5).unwrap(),
            16,
            Smoothing::None,
            &[],
            &[],
            0.0,
        )
        .unwrap();
        let newer_id = ResourceLease::new(Arc::new(())).revision();
        let newer_key = AnalysisKey::new(
            newer_id,
            audio.sample_rate,
            pcm.logical_frames,
            FrameGrid::new(60.0, 0.5).unwrap(),
            16,
            Smoothing::None,
            &[],
            &[],
            0.0,
        )
        .unwrap();
        let mut state = app.preview_timeline.lock().unwrap();
        state.frame(&key, pcm, 8, &|| false).unwrap();
        let cancel = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let (ready_tx, ready_rx) = mpsc::sync_channel(0);
            let timeline = &app.preview_timeline;
            let worker_cancel = &cancel;
            let worker = scope.spawn(move || {
                ready_tx.send(()).unwrap();
                analyze_preview(timeline, &newer_key, pcm, 1, worker_cancel)
            });
            ready_rx.recv().unwrap();
            cancel.store(true, Ordering::Relaxed);
            drop(state);
            assert!(worker.join().unwrap().is_err());
        });
        let state = app.preview_timeline.lock().unwrap();
        assert!(state.key() == Some(&key));
        assert_eq!(state.processed_frame(), Some(8));
    }

    #[test]
    fn h3a_preview_state_survives_error_stale_result_seek_and_project_switch() {
        let ctx = egui::Context::default();
        let mut app = h3a_app();
        let owner = Arc::clone(&app.preview_timeline);
        app.position_secs = 0.205;
        let result = h3a_request(&mut app, &ctx);
        assert_eq!(result.frame_index, 12);
        assert_eq!(result.time_secs, 0.2);
        let first_key = result.analysis_key.clone().unwrap();
        // A completed analysis is retained even if composition fails afterward.
        app.project.output.width = u32::MAX;
        app.project.output.height = u32::MAX;
        app.preview_dirty = true;
        app.start_preview_if_needed(&ctx);
        let failed = app
            .preview_pending
            .take()
            .unwrap()
            .rx
            .recv_timeout(Duration::from_secs(20))
            .unwrap();
        assert!(
            failed.is_err(),
            "the real worker must reach its render allocation error"
        );
        app.project.output.width = 16;
        app.project.output.height = 16;
        let (tx, rx) = mpsc::channel();
        app.preview_pending = Some(PreviewPending {
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
        });
        tx.send(failed).unwrap();
        app.poll_preview(&ctx);
        assert!(Arc::ptr_eq(&owner, &app.preview_timeline));
        assert_eq!(owner.lock().unwrap().processed_frame(), Some(12));
        app.mark_preview_dirty();
        selection_deliver(&mut app, &ctx, result);
        assert!(
            app.displayed_preview.is_none(),
            "revision mismatch discards only display adoption"
        );
        assert!(owner.lock().unwrap().key() == Some(&first_key));
        app.seek_to(0.05);
        assert_eq!(owner.lock().unwrap().processed_frame(), Some(12));
        assert_eq!(h3a_request(&mut app, &ctx).frame_index, 3);
        let checkpoint = app.editor_state.begin_external_edit(&app.project);
        app.project.display_gain_db = 6.0;
        app.editor_state
            .finish_external_edit(&app.project, checkpoint);
        assert!(h3a_request(&mut app, &ctx).analysis_key.as_ref() != Some(&first_key));
        assert!(app.editor_state.undo(&mut app.project));
        app.handle_editor_response(EditorResponse {
            project_changed: true,
            history_changed: true,
            ..EditorResponse::default()
        });
        app.sync_runtime_resources(&ctx);
        assert!(h3a_request(&mut app, &ctx).analysis_key.as_ref() == Some(&first_key));
        // A project replacement does not replace the session timeline owner.
        app.apply_loaded_project(LoadedProject {
            path: PathBuf::from("switch.mivmv.json"),
            project: default_project(0.5),
            audio: None,
            waveform: None,
            audio_error: None,
            images: vec![],
            image_errors: HashMap::new(),
            resources: ResourceManifest::default(),
        });
        assert!(Arc::ptr_eq(&owner, &app.preview_timeline));
        assert!(owner.lock().unwrap().key() == Some(&first_key));
    }

    #[test]
    fn h3a_preview_pixels_match_png_at_containing_frame_time() {
        use musicvideo_core::{Animatable, Interp, Keyframe, NormRect, Track};

        let ctx = egui::Context::default();
        let mut app = h3a_app();
        let mut layer = musicvideo_core::new_solid_layer(&app.project);
        layer.rect = Animatable::Const(NormRect::new(0.0, 0.0, 1.0, 1.0));
        // The transition lies inside frame 12. Evaluating the raw playback
        // position would show frame 13's appearance instead of frame 12's.
        layer.opacity = Animatable::Track(
            Track::new(vec![
                Keyframe {
                    t: 0.0,
                    value: 0.0,
                    interp: Interp::Step,
                },
                Keyframe {
                    t: 0.205,
                    value: 1.0,
                    interp: Interp::Step,
                },
            ])
            .unwrap(),
        );
        app.project.layers.push(layer);
        app.position_secs = 0.205;
        let preview = h3a_request(&mut app, &ctx);
        let output = tempfile::tempdir().unwrap();
        let outcome = export_png_sequence(
            app.project.clone(),
            Arc::clone(&app.assets),
            app.audio.clone().unwrap(),
            output.path(),
            &AtomicBool::new(false),
            &AtomicUsize::new(0),
            || {},
        )
        .unwrap();
        let ExportOutcome::Completed { output_dir, stats } = outcome else {
            panic!("PNG sequence unexpectedly canceled");
        };
        assert_eq!(stats.frames, 30);
        let same_frame = image::open(output_dir.join("frame_000012.png"))
            .unwrap()
            .to_rgba8();
        let next_frame = image::open(output_dir.join("frame_000013.png"))
            .unwrap()
            .to_rgba8();
        assert_ne!(
            same_frame, next_frame,
            "animated opacity must distinguish the two times"
        );
        assert_eq!(preview.frame_index, 12);
        assert_eq!(
            preview.image.pixels,
            egui::ColorImage::from_rgba_unmultiplied([16, 16], same_frame.as_raw()).pixels
        );
    }

    #[test]
    fn h3a_calibration_key_tracks_duration_identity_and_request_bits() {
        let mut app = h3a_app();
        app.project
            .modulators
            .push(musicvideo_core::new_band_energy_modulator(&app.project));
        let first = app.current_calibration_key().unwrap();
        app.project.duration_secs = 0.25;
        assert_ne!(app.current_calibration_key().unwrap(), first);
        app.project.duration_secs = 0.5;
        let mut manifest = app.editor_state.resource_manifest().clone();
        manifest.set_audio(Some(ResourceLease::new(Arc::clone(
            app.audio.as_ref().unwrap(),
        ))));
        app.editor_state
            .reset_for_project_with_resources(&app.project, manifest);
        assert_ne!(app.current_calibration_key().unwrap(), first);
        let id = first.pcm_id;
        let mut requests = modulator_calibration_requests(&app.project.modulators);
        requests[0].gain_db = 0.0;
        let positive = CalibrationKey::new(id, 0.5, &requests);
        requests[0].gain_db = -0.0;
        assert_ne!(positive, CalibrationKey::new(id, 0.5, &requests));
    }

    #[test]
    fn h3a_calibration_discards_completed_old_duration() {
        let ctx = egui::Context::default();
        let mut app = h3a_app();
        app.project
            .modulators
            .push(musicvideo_core::new_band_energy_modulator(&app.project));
        let key = app.current_calibration_key().unwrap();
        let audio = app.audio.as_ref().unwrap();
        let view = pcm_view(&audio.stereo_pcm, audio.sample_rate, 0.5).unwrap();
        let calibrations = calibrate_modulators_view(
            view,
            audio.sample_rate,
            0.5,
            0.5,
            &app.project.modulators,
            &|| false,
        )
        .unwrap();
        let (tx, rx) = mpsc::channel();
        app.calibration_pending = Some(CalibrationPending {
            key: key.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
        });
        app.calibration_key = Some(key);
        app.project.duration_secs = 0.25;
        tx.send(Ok(calibrations)).unwrap();
        app.poll_calibration();
        assert!(app.modulator_calibrations.is_empty());
        assert!(!app.calibration_ready());
        app.start_calibration_if_needed(&ctx);
        let pending = app.calibration_pending.take().unwrap();
        let result = pending.rx.recv_timeout(Duration::from_secs(20)).unwrap();
        let (tx, rx) = mpsc::channel();
        tx.send(result).unwrap();
        app.calibration_pending = Some(CalibrationPending { rx, ..pending });
        app.poll_calibration();
        assert!(app.calibration_ready());
        assert_eq!(app.modulator_calibrations[0].sample_count, 15);
    }

    fn selection_result(
        app: &MusicVideoLabApp,
        project: Project,
        revision: u64,
        time_secs: f64,
        color: egui::Color32,
    ) -> PreviewResult {
        let layer_lifetimes = project
            .layers
            .iter()
            .filter_map(|layer| {
                app.editor_state
                    .layer_target_lifetime(layer.id)
                    .map(|lifetime| (layer.id, lifetime))
            })
            .collect();
        PreviewResult {
            project,
            layer_lifetimes,
            revision,
            time_secs,
            image: egui::ColorImage::filled([160, 90], color),
            rgba: Vec::new(),
            render_ms: 0.0,
            report: RenderReport::default(),
            analysis_key: None,
            frame_index: 0,
            modulator_values: vec![ModulatorValue {
                id: musicvideo_core::ModulatorId(1),
                value: 0.5,
            }],
            timings: PreviewWorkerTimings::default(),
        }
    }

    fn selection_deliver(app: &mut MusicVideoLabApp, ctx: &egui::Context, result: PreviewResult) {
        let (tx, rx) = mpsc::channel();
        app.preview_pending = Some(PreviewPending {
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
        });
        tx.send(Ok(result)).unwrap();
        app.poll_preview(ctx);
    }

    #[test]
    fn selection_displayed_frame_uses_result_time_modulation_and_project_in_real_draw() {
        use musicvideo_core::{
            Animatable, Interp, Keyframe, ModulationBinding, ModulationCurve, ModulationTarget,
            ModulatorId, NormRect, Track,
        };
        let ctx = crate::project::test_context();
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        app.project.layers[0].name = "displayed-box".into();
        app.project.layers[0].rect = Animatable::Track(
            Track::new(vec![
                Keyframe {
                    t: 0.0,
                    value: NormRect::new(0.0, 0.2, 0.2, 0.2),
                    interp: Interp::Linear,
                },
                Keyframe {
                    t: 10.0,
                    value: NormRect::new(1.0, 0.2, 0.2, 0.2),
                    interp: Interp::Linear,
                },
            ])
            .unwrap(),
        );
        app.project.layers[0].modulations.push(ModulationBinding {
            target: ModulationTarget::Scale,
            modulator: ModulatorId(1),
            amount: 1.0,
            curve: ModulationCurve::Linear,
        });
        app.editor_state.reset_for_project(&app.project);
        app.editor_state.select_layer(&app.project, 0);
        let project_snapshot = app.project.clone();
        app.position_secs = 9.0;
        // Ordinary edits deliberately do not reject an already-composed frame.
        app.project.layers[0].rect = Animatable::Const(NormRect::new(0.9, 0.9, 0.1, 0.1));
        app.project.layers[0].name = "latest-box".into();
        let result = selection_result(
            &app,
            project_snapshot,
            app.preview_revision,
            2.0,
            egui::Color32::BLACK,
        );
        selection_deliver(&mut app, &ctx, result);
        let displayed = app.displayed_preview.as_ref().unwrap();
        let bounds = displayed
            .frame()
            .selected_bounds(&app.editor_state)
            .unwrap();
        assert!((bounds.rect.x - 0.15).abs() < 1e-6);
        assert!((bounds.rect.width - 0.3).abs() < 1e-6);
        assert_eq!(displayed.time_secs, 2.0);
        let output = selection_draw_output(&mut app, &ctx);
        assert!(output.shapes.iter().any(
            |s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "displayed-box")
        ));
        assert!(
            !output.shapes.iter().any(
                |s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "latest-box")
            )
        );
        let image = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Mesh(mesh) => Some(mesh.calc_bounds()),
                _ => None,
            })
            .unwrap();
        let path = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Path(path) if path.closed && s.clip_rect.min.y > 20.0 => Some(path),
                _ => None,
            })
            .unwrap();
        assert!((path.points[0].x - (image.left() + image.width() * 0.15)).abs() < 1e-4);
        assert!((path.points[0].y - (image.top() + image.height() * 0.15)).abs() < 1e-4);
    }

    fn selection_assert_texture_color(
        delta: &mut egui::TexturesDelta,
        texture: egui::TextureId,
        color: egui::Color32,
    ) {
        let updates = delta.set.get(&texture).cloned();
        let freed = delta.free.contains(&texture);
        delta.clear();
        let updates = updates.expect("texture upload");
        assert_eq!(updates.len(), 1);
        let egui::ImageData::Color(image) = &updates[0].image;
        assert_eq!(image.size, [160, 90]);
        assert!(image.pixels.iter().all(|pixel| *pixel == color));
        assert!(!freed);
    }

    #[test]
    fn selection_stale_preview_keeps_all_displayed_inputs_and_clear_removes_them() {
        let ctx = crate::project::test_context();
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        app.editor_state.reset_for_project(&app.project);
        let first_revision = app.preview_revision;
        let first = selection_result(
            &app,
            default_project(10.0),
            first_revision,
            2.0,
            egui::Color32::BLACK,
        );
        selection_deliver(&mut app, &ctx, first);
        let texture_id = app.displayed_preview.as_ref().unwrap().texture.id();
        // Consume the initial upload before examining whether a rejected result
        // publishes another upload. TextureHandle::set preserves the same ID.
        let mut initial_delta = ctx.tex_manager().write().take_delta();
        selection_assert_texture_color(&mut initial_delta, texture_id, egui::Color32::BLACK);
        app.mark_preview_dirty();
        let mut stale = selection_result(
            &app,
            default_project(20.0),
            first_revision,
            9.0,
            egui::Color32::RED,
        );
        stale.modulator_values[0].value = 0.9;
        selection_deliver(&mut app, &ctx, stale);
        let mut stale_delta = ctx.tex_manager().write().take_delta();
        let stale_upload = stale_delta.set.contains_key(&texture_id);
        let stale_free = stale_delta.free.contains(&texture_id);
        stale_delta.clear();
        assert!(
            !stale_upload,
            "stale result uploaded pixels to the displayed texture"
        );
        assert!(!stale_free);
        let displayed = app.displayed_preview.as_ref().unwrap();
        assert_eq!(displayed.texture.id(), texture_id);
        assert_eq!(displayed.time_secs, 2.0);
        assert_eq!(displayed.project.duration_secs, 10.0);
        assert_eq!(displayed.modulator_values[0].value, 0.5);
        let revision = app.preview_revision;
        let latest = selection_result(
            &app,
            default_project(30.0),
            revision,
            4.0,
            egui::Color32::GREEN,
        );
        selection_deliver(&mut app, &ctx, latest);
        let mut latest_delta = ctx.tex_manager().write().take_delta();
        selection_assert_texture_color(&mut latest_delta, texture_id, egui::Color32::GREEN);
        let displayed = app.displayed_preview.as_ref().unwrap();
        assert_eq!(displayed.time_secs, 4.0);
        assert_eq!(displayed.project.duration_secs, 30.0);
        assert_eq!(displayed.texture.id(), texture_id);
        app.clear_decoded_resources();
        assert!(app.displayed_preview.is_none());
        let revision = app.preview_revision;
        let next = selection_result(
            &app,
            default_project(30.0),
            revision,
            4.0,
            egui::Color32::BLUE,
        );
        selection_deliver(&mut app, &ctx, next);
        app.apply_loaded_project(loaded_project("new.mivmv", 12.0));
        assert!(app.displayed_preview.is_none());
    }

    #[test]
    fn selection_worker_result_carries_its_captured_time_and_project() {
        let ctx = crate::project::test_context();
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        app.project.output.width = 32;
        app.project.output.height = 18;
        app.position_secs = 2.0;
        app.editor_state.reset_for_project(&app.project);
        let project = app.project.clone();
        app.start_preview_if_needed(&ctx);
        app.position_secs = 9.0;
        app.project.duration_secs = 99.0;
        let pending = app.preview_pending.take().unwrap();
        let result = pending
            .rx
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .unwrap();
        assert_eq!(result.time_secs, 2.0);
        assert_eq!(result.project, project);
        for layer in &result.project.layers {
            assert_eq!(
                result.layer_lifetimes.get(&layer.id).copied(),
                app.editor_state.layer_target_lifetime(layer.id)
            );
        }
    }

    fn selection_request_frame(app: &mut MusicVideoLabApp, ctx: &egui::Context) -> PreviewResult {
        app.schedule_project_edit_preview();
        app.start_preview_if_needed(ctx);
        app.preview_pending
            .take()
            .unwrap()
            .rx
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .unwrap()
    }

    fn selection_draw_output(app: &mut MusicVideoLabApp, ctx: &egui::Context) -> egui::FullOutput {
        let mut output = egui::FullOutput::default();
        for _ in 0..2 {
            output = crate::project::headless_paint_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::Vec2::new(640.0, 480.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::Panel::top("preview-controls")
                        .show(ctx, |ui| app.editor_state.preview_controls(ui));
                    egui::CentralPanel::default().show(ctx, |ui| app.draw_preview(ui));
                },
            ));
        }
        output
    }

    fn selection_assert_outline(
        app: &mut MusicVideoLabApp,
        ctx: &egui::Context,
        name: Option<&str>,
    ) {
        let output = selection_draw_output(app, ctx);
        let paths = output
            .shapes
            .iter()
            .filter(|s| {
                s.clip_rect.min.y > 20.0
                    && matches!(&s.shape, egui::Shape::Path(path) if path.closed)
            })
            .count();
        assert_eq!(paths, if name.is_some() { 2 } else { 0 });
        for label in ["deleted-box", "replacement-box"] {
            let shown = output.shapes.iter().any(|s| {
                matches!(&s.shape,
                egui::Shape::Text(text) if text.galley.text() == label)
            });
            assert_eq!(
                shown,
                name == Some(label),
                "unexpected preview label: {label}"
            );
        }
    }

    fn selection_add_test_layer(app: &mut MusicVideoLabApp, replacement: bool) -> LayerId {
        let checkpoint = app.editor_state.begin_external_edit(&app.project);
        let mut layer = if replacement {
            musicvideo_core::new_progress_bar_layer(&app.project)
        } else {
            musicvideo_core::new_solid_layer(&app.project)
        };
        layer.name = if replacement {
            "replacement-box"
        } else {
            "deleted-box"
        }
        .into();
        let id = layer.id;
        app.project.layers.push(layer);
        assert!(
            app.editor_state
                .finish_external_edit(&app.project, checkpoint)
        );
        app.editor_state
            .select_layer(&app.project, app.project.layers.len() - 1);
        id
    }

    fn selection_check_reused_layer_id(undo_add: bool) {
        let ctx = crate::project::test_context();
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        app.project.output.width = 32;
        app.project.output.height = 18;
        app.editor_state.reset_for_project(&app.project);
        let original_id = selection_add_test_layer(&mut app, false);
        let original_lifetime = app.editor_state.layer_target_lifetime(original_id).unwrap();
        let first = selection_request_frame(&mut app, &ctx);
        assert_eq!(
            first.layer_lifetimes.get(&original_id),
            Some(&original_lifetime)
        );
        selection_deliver(&mut app, &ctx, first);
        selection_assert_outline(&mut app, &ctx, Some("deleted-box"));
        // Keep a second result from the old lifetime in flight across editing.
        let in_flight = selection_request_frame(&mut app, &ctx);
        let revision = app.preview_revision;
        if undo_add {
            assert!(app.editor_state.undo(&mut app.project));
        } else {
            assert!(app.editor_state.apply_external_command(
                &mut app.project,
                &EditorCommand::DeleteLayer { layer: original_id },
            ));
        }
        assert!(
            app.editor_state
                .layer_target_lifetime(original_id)
                .is_none()
        );
        let replacement_id = selection_add_test_layer(&mut app, true);
        assert_eq!(replacement_id, original_id);
        let replacement_lifetime = app
            .editor_state
            .layer_target_lifetime(replacement_id)
            .unwrap();
        assert_ne!(replacement_lifetime, original_lifetime);
        app.schedule_project_edit_preview();
        assert_eq!(app.preview_revision, revision);
        // The old displayed frame and the accepted old-lifetime result must
        // neither draw a box nor publish the deleted layer's name for this ID.
        selection_assert_outline(&mut app, &ctx, None);
        assert_eq!(in_flight.revision, revision);
        selection_deliver(&mut app, &ctx, in_flight);
        selection_assert_outline(&mut app, &ctx, None);
        assert_eq!(
            app.displayed_preview
                .as_ref()
                .unwrap()
                .layer_lifetimes
                .get(&original_id),
            Some(&original_lifetime)
        );
        // Another layer still owns its lifetime: don't suppress all outlines
        // just because this project has been edited or the texture is old.
        app.editor_state.select_layer(&app.project, 0);
        assert!(
            app.displayed_preview
                .as_ref()
                .unwrap()
                .frame()
                .selected_bounds(&app.editor_state)
                .is_some()
        );
        app.editor_state
            .select_layer(&app.project, app.project.layers.len() - 1);
        let latest = selection_request_frame(&mut app, &ctx);
        assert_eq!(
            latest.layer_lifetimes.get(&original_id),
            Some(&replacement_lifetime)
        );
        selection_deliver(&mut app, &ctx, latest);
        selection_assert_outline(&mut app, &ctx, Some("replacement-box"));
    }

    #[test]
    fn selection_reused_layer_id_after_delete_does_not_draw_old_lifetime() {
        selection_check_reused_layer_id(false);
    }

    #[test]
    fn selection_reused_layer_id_after_undo_and_other_layer_add_does_not_draw_old_lifetime() {
        selection_check_reused_layer_id(true);
    }

    #[test]
    fn playback_and_project_edits_do_not_cancel_a_complete_in_flight_frame() {
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let revision = app.preview_revision;
        app.preview_dirty = false;
        let cancel = Arc::new(AtomicBool::new(false));
        let (_tx, rx) = mpsc::channel();
        app.preview_pending = Some(PreviewPending {
            cancel: Arc::clone(&cancel),
            rx,
        });

        app.schedule_playback_preview();
        assert!(app.preview_dirty);
        assert_eq!(app.preview_revision, revision);
        assert!(!cancel.load(Ordering::Relaxed));

        app.preview_dirty = false;
        app.schedule_project_edit_preview();
        assert!(app.preview_dirty);
        assert_eq!(app.preview_revision, revision);
        assert!(!cancel.load(Ordering::Relaxed));

        app.mark_preview_dirty();
        assert_eq!(app.preview_revision, revision.wrapping_add(1));
        assert!(cancel.load(Ordering::Relaxed));
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
    fn delayed_save_completion_does_not_rebind_a_loaded_document() {
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let checkpoint = app
            .document
            .begin_save(PathBuf::from("a.mivmv.json"), &app.project);
        let (tx, rx) = mpsc::channel();
        app.project_save_pending = Some(ProjectSavePending::Writing(ProjectSaveWrite {
            checkpoint,
            rx,
        }));

        app.project = default_project(20.0);
        app.document
            .replace_with_loaded(PathBuf::from("b.mivmv.json"), &app.project);
        tx.send(Ok(())).unwrap();
        app.poll_projects(&crate::project::test_context());

        assert_eq!(
            app.document.path(),
            Some(std::path::Path::new("b.mivmv.json"))
        );
        assert!(!app.document.is_dirty(&app.project));
        assert!(app.status.contains("現在の保存先は変更していません"));
    }

    #[test]
    fn save_completion_adopts_its_path_without_hiding_later_edits() {
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let checkpoint = app
            .document
            .begin_save(PathBuf::from("a.mivmv.json"), &app.project);
        let (tx, rx) = mpsc::channel();
        app.project_save_pending = Some(ProjectSavePending::Writing(ProjectSaveWrite {
            checkpoint,
            rx,
        }));
        app.project.display_gain_db = 6.0;
        app.document.note_edit();

        tx.send(Ok(())).unwrap();
        app.poll_projects(&crate::project::test_context());

        assert_eq!(
            app.document.path(),
            Some(std::path::Path::new("a.mivmv.json"))
        );
        assert!(app.document.is_dirty(&app.project));
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

        let store = ResourceBlobStore::new().unwrap();
        let loaded = load_project_bundle(project_path, &store, &AtomicBool::new(false))
            .expect("project loads");
        assert_eq!(loaded.project, project);
        assert!(loaded.image_errors.is_empty());
        assert_eq!(loaded.images.len(), 1);
        assert_eq!(loaded.images[0].1.rgba, [12, 34, 56, 255].repeat(6));

        let restored_assets = Assets::default();
        for (id, image) in loaded.images {
            restored_assets.insert_shared_image(id, image);
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

        let loaded = load_project_bundle(
            project_path,
            &ResourceBlobStore::new().unwrap(),
            &AtomicBool::new(false),
        )
        .expect("project loads");
        assert!(loaded.images.is_empty());
        assert!(
            loaded
                .image_errors
                .get(&ImageId::new(BACKGROUND_ID))
                .is_some_and(|error| error.contains("missing.png"))
        );
    }

    pub(super) fn loaded_project(path: &str, duration_secs: f64) -> LoadedProject {
        LoadedProject {
            path: PathBuf::from(path),
            project: default_project(duration_secs),
            audio: None,
            waveform: None,
            images: Vec::new(),
            resources: ResourceManifest::default(),
            image_errors: HashMap::new(),
            audio_error: None,
        }
    }

    pub(super) fn editor_input(
        app: &mut MusicVideoLabApp,
        context: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        crate::project::headless_paint_output(context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 1200.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                app.process_editor_frame(ctx);
            },
        ))
    }

    pub(super) fn pointer_input(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    pub(super) fn rendered_editor_text(
        app: &mut MusicVideoLabApp,
        context: &egui::Context,
        text: &str,
        exact: bool,
    ) -> egui::Pos2 {
        editor_input(app, context, vec![]);
        let output = editor_input(app, context, vec![]);
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(shape)
                    if if exact {
                        shape.galley.text() == text
                    } else {
                        shape.galley.text().starts_with(text)
                    } =>
                {
                    Some(shape.pos + shape.galley.rect.center().to_vec2())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing panel text: {text}"))
    }

    fn selection_check_ui_add_captures_lifetimes_in_the_same_frame(
        preset: &str,
        added_count: usize,
    ) {
        let ctx = crate::project::test_context();
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        app.project.output.width = 32;
        app.project.output.height = 18;
        app.editor_state.set_inspector_tab(InspectorTab::Project);
        assert!(app.audio_player.is_none());
        assert_eq!(app.position_secs, 0.0);
        let before = app.project.clone();
        let menu = rendered_editor_text(&mut app, &ctx, "＋ レイヤーを追加", true);
        editor_input(&mut app, &ctx, pointer_input(menu, true));
        editor_input(&mut app, &ctx, pointer_input(menu, false));
        let item = rendered_editor_text(&mut app, &ctx, preset, true);
        editor_input(&mut app, &ctx, pointer_input(item, true));
        // Release adds through the real menu. As in update, create the request
        // after process_editor_frame in THIS pass, before another begin_frame.
        let _ = crate::project::headless_paint_output(ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 1200.0),
                )),
                events: pointer_input(item, false),
                ..Default::default()
            },
            |ctx| {
                app.process_editor_frame(ctx);
                app.start_calibration_if_needed(ctx);
                app.start_preview_if_needed(ctx);
            },
        ));
        assert_eq!(app.project.layers.len(), before.layers.len() + added_count);
        assert_eq!(app.editor_state.history_depths(), (1, 0));
        assert_eq!(app.position_secs, 0.0);
        let result = app
            .preview_pending
            .take()
            .expect("same-frame preview request")
            .rx
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .unwrap();
        assert_eq!(result.project, app.project);
        assert_eq!(result.time_secs, 0.0);
        assert_eq!(
            result.layer_lifetimes.len(),
            result.project.layers.len(),
            "the first request must contain every layer's lifetime"
        );
        for layer in &result.project.layers {
            let captured = result
                .layer_lifetimes
                .get(&layer.id)
                .copied()
                .expect("captured lifetime");
            assert_eq!(
                Some(captured),
                app.editor_state.layer_target_lifetime(layer.id)
            );
        }
        let selected = app.project.layers[app.editor_state.selected_layer(&app.project)].clone();
        selection_deliver(&mut app, &ctx, result);
        assert!(
            app.displayed_preview
                .as_ref()
                .unwrap()
                .frame()
                .selected_bounds(&app.editor_state)
                .is_some()
        );
        // Rendering the first completion does not run editor preparation.
        let output = selection_draw_output(&mut app, &ctx);
        assert_eq!(
            output
                .shapes
                .iter()
                .filter(|s| s.clip_rect.min.y > 20.0
                    && matches!(&s.shape, egui::Shape::Path(path) if path.closed))
                .count(),
            2
        );
        assert!(output.shapes.iter().any(
            |s| matches!(&s.shape, egui::Shape::Text(text) if text.galley.text() == selected.name)
        ));
        assert!(!app.preview_dirty);
        assert!(app.preview_pending.is_none());
        assert_eq!(app.editor_state.history_depths(), (1, 0));
        let added = app.project.clone();
        assert!(app.editor_state.undo(&mut app.project));
        assert_eq!(app.project, before);
        assert!(
            app.editor_state
                .layer_target_lifetime(selected.id)
                .is_none()
        );
        assert!(app.editor_state.redo(&mut app.project));
        assert_eq!(app.project, added);
        assert!(
            app.editor_state
                .layer_target_lifetime(selected.id)
                .is_some()
        );
    }

    #[test]
    fn selection_ui_add_solid_captures_lifetimes_and_draws_the_first_stopped_frame() {
        selection_check_ui_add_captures_lifetimes_in_the_same_frame("単色", 1);
    }

    #[test]
    fn selection_ui_add_two_layers_captures_lifetimes_and_draws_the_first_stopped_frame() {
        selection_check_ui_add_captures_lifetimes_in_the_same_frame("mIV スペクトラム + 鍵盤", 2);
    }

    #[test]
    fn editor_panels_keep_timeline_full_width_and_inspector_tabs_separate() {
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let context = crate::project::test_context();
        for tab in [
            InspectorTab::Layer,
            InspectorTab::Modulators,
            InspectorTab::Project,
        ] {
            app.editor_state.set_inspector_tab(tab);
            editor_input(&mut app, &context, vec![]);
            let output = editor_input(&mut app, &context, vec![]);
            let panel = |id| {
                egui::containers::panel::PanelState::load(
                    &context,
                    context.viewport_id().root_ui_id().with(id),
                )
                .unwrap()
                .outer_rect
            };
            let timeline = panel("musicvideo-timeline");
            let left = panel("musicvideo-layers");
            let right = panel("musicvideo-properties");
            assert_eq!(timeline.left(), 0.0);
            assert_eq!(timeline.right(), 1400.0);
            assert!(left.bottom() <= timeline.top());
            assert!(right.bottom() <= timeline.top());
            assert!((left.width() - 220.0).abs() < 2.0);
            assert!((right.width() - 360.0).abs() < 2.0);
            let inspector_text = |text: &str| {
                output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(shape) if shape.galley.text().starts_with(text) && right.contains(shape.pos)))
            };
            assert_eq!(inspector_text("不透明度"), tab == InspectorTab::Layer);
            assert_eq!(
                inspector_text("＋ キックに合わせる動きを追加"),
                tab == InspectorTab::Modulators
            );
            assert_eq!(inspector_text("表示ゲイン"), tab == InspectorTab::Project);
        }
    }

    #[test]
    fn project_load_completion_after_same_frame_input_waits_for_a_choice() {
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let started_at = app.document.edit_generation();
        let (tx, rx) = mpsc::channel();
        app.project_load = ProjectLoadState::Loading(ProjectLoadPending {
            started_at,
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
            queued: None,
        });
        let context = crate::project::test_context();
        app.editor_state.set_inspector_tab(InspectorTab::Project);
        let pos = rendered_editor_text(&mut app, &context, "表示ゲイン", false);
        editor_input(&mut app, &context, pointer_input(pos, true));
        tx.send(Ok(loaded_project("loaded.mivmv.json", 25.0)))
            .unwrap();

        // Completion is already available while this frame's panel changes the document.
        editor_input(
            &mut app,
            &context,
            vec![egui::Event::PointerMoved(pos + egui::vec2(40.0, 0.0))],
        );
        let gain = app.project.display_gain_db;
        assert_ne!(gain, 0.0);
        assert_ne!(app.document.edit_generation(), started_at);

        assert_eq!(app.project.display_gain_db, gain);
        assert!(matches!(app.project_load, ProjectLoadState::Ready(_)));
        let ProjectLoadState::Ready(loaded) = std::mem::take(&mut app.project_load) else {
            unreachable!();
        };
        app.apply_loaded_project(loaded);
        assert_eq!(app.project.duration_secs, 25.0);
        assert_eq!(
            app.document.path(),
            Some(std::path::Path::new("loaded.mivmv.json"))
        );
    }

    #[test]
    fn project_load_switches_automatically_when_the_old_document_is_unchanged() {
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let (tx, rx) = mpsc::channel();
        app.project_load = ProjectLoadState::Loading(ProjectLoadPending {
            started_at: app.document.edit_generation(),
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
            queued: None,
        });
        tx.send(Ok(loaded_project("loaded.mivmv.json", 18.0)))
            .unwrap();

        app.poll_projects(&crate::project::test_context());

        assert_eq!(app.project.duration_secs, 18.0);
        assert!(matches!(app.project_load, ProjectLoadState::Idle));
    }

    #[test]
    fn project_load_requires_a_choice_for_a_pending_lyrics_draft() {
        let app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let unchanged_generation = app.document.edit_generation();

        assert!(project_load_requires_choice(
            unchanged_generation,
            unchanged_generation,
            true,
        ));
    }

    #[test]
    fn undo_moves_history_and_cancels_pending_import_without_dropping_its_receiver() {
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let before = app.project.display_gain_db;
        let checkpoint = app.editor_state.begin_external_edit(&app.project);
        app.project.display_gain_db = 7.0;
        assert!(
            app.editor_state
                .finish_external_edit(&app.project, checkpoint)
        );
        let (sender, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        app.image_pending = Some(ImagePending {
            request: ImageLoadRequest {
                session: app.document.id(),
                target: ImageLoadTarget::AddLayer,
                target_lifetime: None,
                path: PathBuf::from("first.png"),
            },
            cancel: Arc::clone(&cancel),
            rx,
            queued: Some(ImageLoadRequest {
                session: app.document.id(),
                target: ImageLoadTarget::AddLayer,
                target_lifetime: None,
                path: PathBuf::from("queued.png"),
            }),
        });

        assert!(app.editor_state.undo(&mut app.project));
        app.handle_editor_response(EditorResponse {
            project_changed: true,
            history_changed: true,
            ..EditorResponse::default()
        });

        assert_eq!(app.project.display_gain_db, before);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(app.image_pending.is_some());
        assert!(app.image_pending.as_ref().unwrap().queued.is_none());
        assert!(app.editor_state.redo(&mut app.project));
        assert_eq!(app.project.display_gain_db, 7.0);
        drop(sender);
    }

    #[test]
    fn image_completion_cannot_attach_to_a_recreated_target_with_the_same_ids() {
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let image_id = ImageId::new("replace-me");
        app.project.images.push(ImageRef {
            id: image_id.clone(),
            path: PathBuf::from("original.png"),
        });
        let layer = new_image_layer(&app.project, image_id.clone());
        let layer_id = layer.id;
        app.project.layers.push(layer.clone());
        app.editor_state.reset_for_project(&app.project);
        let target_lifetime = app.editor_state.image_target_lifetime(&image_id).unwrap();
        app.assets.insert_image(
            image_id.clone(),
            ImageAsset::new(1, 1, vec![1, 0, 0, 255]).unwrap(),
        );
        let store = app.resource_store.clone().unwrap();
        let replacement = Arc::new(ImageAsset::new(1, 1, vec![2, 0, 0, 255]).unwrap());
        let lease = store
            .store_image(PathBuf::from("late.png"), &replacement)
            .unwrap();
        let (tx, rx) = mpsc::channel();
        app.image_pending = Some(ImagePending {
            request: ImageLoadRequest {
                session: app.document.id(),
                target: ImageLoadTarget::Replace(image_id.clone()),
                target_lifetime: Some(target_lifetime),
                path: PathBuf::from("late.png"),
            },
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
            queued: None,
        });
        assert!(app.editor_state.apply_external_command(
            &mut app.project,
            &EditorCommand::DeleteLayer { layer: layer_id },
        ));
        let checkpoint = app.editor_state.begin_external_edit(&app.project);
        app.project.images.push(ImageRef {
            id: image_id.clone(),
            path: PathBuf::from("new-owner.png"),
        });
        app.project.layers.push(layer);
        assert!(
            app.editor_state
                .finish_external_edit(&app.project, checkpoint)
        );
        tx.send(Ok((PathBuf::from("late.png"), replacement, lease)))
            .unwrap();

        app.poll_loads(&crate::project::test_context());

        assert_eq!(app.project.images[0].path, PathBuf::from("new-owner.png"));
        assert_eq!(app.assets.image(&image_id).unwrap().rgba[0], 1);
        assert!(app.status.contains("対象が変更された"));
    }

    #[test]
    fn stale_restore_completion_is_drained_before_the_latest_completion() {
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let manifest = ResourceManifest::default();
        app.runtime_resources = RuntimeResourceState::Restoring {
            manifest: manifest.clone(),
            generation: 2,
        };
        let (tx, rx) = mpsc::channel();
        app.restore_worker = Some(ResourceRestoreWorker {
            queue: Arc::new(RestoreQueue::default()),
            rx,
            next_generation: 3,
            active_cancel: None,
        });
        tx.send(RestoreCompletion {
            generation: 1,
            session: app.document.id(),
            manifest: manifest.clone(),
            result: Err("stale".to_owned()),
        })
        .unwrap();
        tx.send(RestoreCompletion {
            generation: 2,
            session: app.document.id(),
            manifest: manifest.clone(),
            result: Err("latest".to_owned()),
        })
        .unwrap();

        app.poll_resource_restore();

        assert!(matches!(
            &app.runtime_resources,
            RuntimeResourceState::Unresolved { error, .. } if error == "latest"
        ));
    }

    #[test]
    fn disconnected_restore_worker_is_replaced_before_retry() {
        let context = crate::project::test_context();
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let manifest = ResourceManifest::default();
        app.runtime_resources = RuntimeResourceState::Restoring {
            manifest: manifest.clone(),
            generation: 1,
        };
        let (_tx, rx) = mpsc::channel::<RestoreCompletion>();
        drop(_tx);
        app.restore_worker = Some(ResourceRestoreWorker {
            queue: Arc::new(RestoreQueue::default()),
            rx,
            next_generation: 2,
            active_cancel: None,
        });

        app.poll_resource_restore();

        assert!(matches!(
            &app.runtime_resources,
            RuntimeResourceState::Unresolved { error, .. }
                if error.contains("再試行できます")
        ));
        assert!(app.restore_worker.is_some());
        app.retry_runtime_restore(&context);
        assert!(matches!(
            app.runtime_resources,
            RuntimeResourceState::Restoring { .. }
        ));
    }

    #[test]
    fn restore_completion_keeps_property_edits_made_after_the_request() {
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let id = ImageId::new("restored");
        app.project.images.push(ImageRef {
            id: id.clone(),
            path: PathBuf::from("restored.png"),
        });
        app.project
            .layers
            .push(new_image_layer(&app.project, id.clone()));
        let image = Arc::new(ImageAsset::new(1, 1, vec![9, 8, 7, 255]).unwrap());
        let lease = app
            .resource_store
            .as_ref()
            .unwrap()
            .store_image(PathBuf::from("restored.png"), &image)
            .unwrap();
        let mut manifest = ResourceManifest::default();
        manifest.set_image(id.clone(), lease);
        app.editor_state
            .reset_for_project_with_resources(&app.project, manifest.clone());
        app.runtime_resources = RuntimeResourceState::Restoring {
            manifest: manifest.clone(),
            generation: 4,
        };
        app.project.display_gain_db = 3.0;
        app.document.note_edit();

        app.apply_resource_restore_completion(RestoreCompletion {
            generation: 4,
            session: app.document.id(),
            manifest: manifest.clone(),
            result: Ok((
                RestoredResources {
                    manifest,
                    audio: None,
                    images: vec![(id.clone(), image)],
                },
                None,
            )),
        });

        assert_eq!(app.project.display_gain_db, 3.0);
        assert_eq!(app.assets.image(&id).unwrap().rgba[0], 9);
        assert!(matches!(
            app.runtime_resources,
            RuntimeResourceState::Ready(_)
        ));
    }

    #[test]
    fn restore_failure_detaches_old_pixels_and_keeps_logical_history_retryable() {
        let context = crate::project::test_context();
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let id = ImageId::new("broken-history");
        app.project.images.push(ImageRef {
            id: id.clone(),
            path: PathBuf::from("missing.png"),
        });
        let layer = new_image_layer(&app.project, id.clone());
        app.project.layers.push(layer);
        let mut manifest = ResourceManifest::default();
        manifest.set_image(id.clone(), ResourceLease::new(Arc::new(())));
        app.editor_state
            .reset_for_project_with_resources(&app.project, manifest);
        let checkpoint = app.editor_state.begin_external_edit(&app.project);
        app.project.display_gain_db = 2.0;
        assert!(
            app.editor_state
                .finish_external_edit(&app.project, checkpoint)
        );
        app.assets.insert_image(
            id.clone(),
            ImageAsset::new(1, 1, vec![1, 2, 3, 255]).unwrap(),
        );
        app.displayed_preview = Some(DisplayedPreview {
            project: app.project.clone(),
            layer_lifetimes: HashMap::new(),
            time_secs: 0.0,
            modulator_values: vec![],
            texture: context.load_texture(
                "old-preview",
                egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
                egui::TextureOptions::LINEAR,
            ),
        });

        app.sync_runtime_resources(&context);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !matches!(
            app.runtime_resources,
            RuntimeResourceState::Unresolved { .. }
        ) && Instant::now() < deadline
        {
            std::thread::yield_now();
            app.poll_resource_restore();
        }

        assert!(matches!(
            app.runtime_resources,
            RuntimeResourceState::Unresolved { .. }
        ));
        assert!(!app.assets.has_image(&id));
        assert!(app.displayed_preview.is_none());
        assert!(app.editor_state.can_undo());
        app.retry_runtime_restore(&context);
        assert!(matches!(
            app.runtime_resources,
            RuntimeResourceState::Restoring { .. }
        ));
    }

    #[test]
    fn imports_during_restore_request_a_complete_latest_bundle() {
        let context = crate::project::test_context();
        let mut image_app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let image_id = ImageId::new("image");
        image_app.project.images.push(ImageRef {
            id: image_id.clone(),
            path: PathBuf::from("old.png"),
        });
        image_app
            .project
            .layers
            .push(new_image_layer(&image_app.project, image_id.clone()));
        image_app.project.audio = Some(AudioRef {
            path: PathBuf::from("old.wav"),
        });
        let store = image_app.resource_store.clone().unwrap();
        let old_image = ImageAsset::new(1, 1, vec![1, 0, 0, 255]).unwrap();
        let old_audio = AudioData {
            stereo_pcm: vec![0.25, -0.25],
            sample_rate: 48_000,
            duration_secs: 1.0 / 48_000.0,
        };
        let mut old_manifest = ResourceManifest::default();
        old_manifest.set_image(
            image_id.clone(),
            store
                .store_image(PathBuf::from("old.png"), &old_image)
                .unwrap(),
        );
        old_manifest.set_audio(Some(
            store
                .store_audio(PathBuf::from("old.wav"), &old_audio)
                .unwrap(),
        ));
        image_app
            .editor_state
            .reset_for_project_with_resources(&image_app.project, old_manifest.clone());
        image_app.runtime_resources = RuntimeResourceState::Restoring {
            manifest: old_manifest,
            generation: 1,
        };
        let new_image = Arc::new(ImageAsset::new(1, 1, vec![2, 0, 0, 255]).unwrap());
        let new_image_lease = store
            .store_image(PathBuf::from("new.png"), &new_image)
            .unwrap();
        image_app.commit_image(
            ImageLoadTarget::Replace(image_id.clone()),
            PathBuf::from("new.png"),
            new_image,
            new_image_lease.clone(),
            &context,
        );
        let RuntimeResourceState::Restoring { manifest, .. } = &image_app.runtime_resources else {
            panic!("partial image commit must request the complete bundle");
        };
        assert_eq!(
            manifest.image(&image_id).map(ResourceLease::revision),
            Some(new_image_lease.revision())
        );
        assert!(manifest.audio().is_some());
        assert!(image_app.audio.is_none());
        assert!(!image_app.assets.has_image(&image_id));

        let mut audio_app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        audio_app.project.images.push(ImageRef {
            id: image_id.clone(),
            path: PathBuf::from("old.png"),
        });
        audio_app
            .project
            .layers
            .push(new_image_layer(&audio_app.project, image_id.clone()));
        let audio_store = audio_app.resource_store.clone().unwrap();
        let image_lease = audio_store
            .store_image(PathBuf::from("old.png"), &old_image)
            .unwrap();
        let mut manifest = ResourceManifest::default();
        manifest.set_image(image_id.clone(), image_lease);
        audio_app
            .editor_state
            .reset_for_project_with_resources(&audio_app.project, manifest.clone());
        audio_app.runtime_resources = RuntimeResourceState::Restoring {
            manifest,
            generation: 1,
        };
        let new_audio = Arc::new(AudioData {
            stereo_pcm: vec![0.5, -0.5],
            sample_rate: 48_000,
            duration_secs: 1.0 / 48_000.0,
        });
        let new_audio_lease = audio_store
            .store_audio(PathBuf::from("new.wav"), &new_audio)
            .unwrap();
        audio_app.commit_audio(
            PathBuf::from("new.wav"),
            new_audio,
            TimelineWaveform::new(1.0 / 48_000.0, vec![[0.5, 0.5]]),
            new_audio_lease.clone(),
            &context,
        );
        let RuntimeResourceState::Restoring { manifest, .. } = &audio_app.runtime_resources else {
            panic!("partial audio commit must request the complete bundle");
        };
        assert!(manifest.image(&image_id).is_some());
        assert_eq!(
            manifest.audio().map(ResourceLease::revision),
            Some(new_audio_lease.revision())
        );
        assert!(audio_app.audio.is_none());
        assert!(!audio_app.assets.has_image(&image_id));
    }

    #[test]
    fn lrc_completion_after_same_frame_input_waits_for_a_choice() {
        let context = crate::project::test_context();
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        let mut layer = musicvideo_core::new_lyrics_layer(&app.project);
        let layer_id = layer.id;
        let LayerKind::Lyrics(lyrics) = &mut layer.kind else {
            unreachable!();
        };
        lyrics.lines = vec![LyricLine {
            text: "original".to_owned(),
            time_secs: Some(1.0),
        }];
        app.project.layers.push(layer);
        app.editor_state.reset_for_project(&app.project);
        let owner = LrcLoadOwner {
            session: app.document.id(),
            layer: layer_id,
            target_lifetime: app.editor_state.layer_target_lifetime(layer_id).unwrap(),
            baseline: app.current_lyrics(layer_id).unwrap(),
        };
        let (tx, rx) = mpsc::channel();
        app.lrc_load = LrcLoadState::Loading { owner, rx };
        let report = parse_lrc(b"[00:02.00]loaded\n").unwrap();
        app.editor_state
            .select_layer(&app.project, app.project.layers.len() - 1);
        let pos = rendered_editor_text(&mut app, &context, "original", true);
        editor_input(&mut app, &context, pointer_input(pos, true));
        editor_input(&mut app, &context, pointer_input(pos, false));
        tx.send(Ok((PathBuf::from("loaded.lrc"), report))).unwrap();
        editor_input(
            &mut app,
            &context,
            vec![
                egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
                },
                egui::Event::Text("edited while loading".to_owned()),
            ],
        );
        assert_eq!(
            app.current_lyrics(layer_id).unwrap()[0].text,
            "edited while loading"
        );

        assert_eq!(
            app.current_lyrics(layer_id).unwrap()[0].text,
            "edited while loading"
        );
        let LrcLoadState::Ready {
            owner,
            path,
            report,
        } = std::mem::take(&mut app.lrc_load)
        else {
            panic!("lyrics conflict must wait for an explicit choice");
        };
        app.apply_lrc_result(owner, path, report, &context);
        assert_eq!(app.current_lyrics(layer_id).unwrap()[0].text, "loaded");
    }

    #[test]
    fn dormant_audio_stream_does_not_override_a_timeline_position() {
        let dormant = PlaybackSnapshot {
            position_secs: 0.0,
            playing: true,
            ..Default::default()
        };
        assert_eq!(
            playback_snapshot_for_ui(7.5, false, Some(dormant), 0, None),
            PlaybackSnapshot {
                position_secs: 7.5,
                playing: false,
                ..Default::default()
            }
        );
    }

    fn acknowledged_snapshot(
        operation: u64,
        revision: ResourceRevision,
        position: f64,
    ) -> PlaybackSnapshot {
        PlaybackSnapshot {
            applied_operation: operation,
            resource_revision: Some(revision),
            position_secs: position,
            playing: true,
            ..Default::default()
        }
    }

    #[test]
    fn p4b2_replacement_rejects_old_position_until_its_operation_is_applied() {
        let old = ResourceLease::new(Arc::new(())).revision();
        let new = ResourceLease::new(Arc::new(())).revision();
        let stale = acknowledged_snapshot(3, old, 0.9);
        assert_eq!(
            playback_snapshot_for_ui(0.25, true, Some(stale), 4, Some(new)).position_secs,
            0.25
        );
        let fresh = acknowledged_snapshot(4, new, 0.3);
        assert_eq!(
            playback_snapshot_for_ui(0.25, true, Some(fresh), 4, Some(new)).position_secs,
            0.3
        );
    }

    #[test]
    fn p4b2_undo_rejects_a_previously_applied_snapshot_of_the_same_pcm() {
        let restored = ResourceLease::new(Arc::new(())).revision();
        // Revision equality alone is insufficient when Undo returns to this PCM.
        let stale = acknowledged_snapshot(1, restored, 0.9);
        assert_eq!(
            playback_snapshot_for_ui(0.25, true, Some(stale), 5, Some(restored)).position_secs,
            0.25
        );
    }

    #[test]
    fn p4b2_consecutive_seeks_reject_intermediate_positions_for_the_same_pcm() {
        let revision = ResourceLease::new(Arc::new(())).revision();
        let intermediate = acknowledged_snapshot(6, revision, 0.2);
        assert_eq!(
            playback_snapshot_for_ui(0.7, true, Some(intermediate), 7, Some(revision))
                .position_secs,
            0.7
        );
        let fresh = acknowledged_snapshot(7, revision, 0.7);
        assert_eq!(
            playback_snapshot_for_ui(0.7, true, Some(fresh), 7, Some(revision)).position_secs,
            0.7
        );
    }

    #[test]
    fn p4b2_snapshot_of_another_revision_cannot_drive_preview_analysis_time() {
        let current = ResourceLease::new(Arc::new(())).revision();
        let other = ResourceLease::new(Arc::new(())).revision();
        let wrong_pcm = acknowledged_snapshot(4, other, 0.9);
        assert_eq!(
            playback_snapshot_for_ui(0.25, true, Some(wrong_pcm), 4, Some(current)).position_secs,
            0.25
        );
    }

    #[test]
    fn audio_transport_is_disabled_when_output_initialization_failed() {
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        app.audio = Some(Arc::new(AudioData {
            stereo_pcm: vec![0.0, 0.0],
            sample_rate: 48_000,
            duration_secs: 1.0 / 48_000.0,
        }));
        app.audio_player = None;

        assert!(app.runtime_resources_ready());
        assert!(!app.audio_transport_ready());
    }

    #[test]
    fn p4b2_device_open_failure_keeps_the_host_play_command_available() {
        let ctx = egui::Context::default();
        let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
        app.audio_player_factory = AudioPlayer::new_failing_headless;
        let pcm = Arc::new(AudioData {
            stereo_pcm: vec![0.25; 96_000],
            sample_rate: 48_000,
            duration_secs: 1.0,
        });
        app.position_secs = 0.25;
        app.commit_audio(
            PathBuf::from("test.wav"),
            pcm.clone(),
            TimelineWaveform::new(1.0, vec![]),
            ResourceLease::new(pcm),
            &ctx,
        );
        let player = app.audio_player.as_ref().unwrap();
        assert_eq!(
            player.wait_applied().error.as_deref(),
            Some("headless: no device")
        );
        assert!(app.audio_transport_ready());
        app.apply_chrome_actions(&ctx, vec![ChromeAction::TogglePlayback]);
        let player = app.audio_player.as_ref().unwrap();
        let snapshot = player.wait_applied();
        assert_eq!(snapshot.applied_operation, 2);
        assert_eq!(snapshot.error.as_deref(), Some("headless: no device"));
        assert!(!snapshot.playing);
        assert!(app.audio_transport_ready());
        app.poll_playback_error(&ctx);
        assert!(app.status.contains("headless: no device"));
    }
}
