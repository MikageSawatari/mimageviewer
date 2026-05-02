use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use eframe::egui;
use uuid::Uuid;

use crate::app::App;
use crate::video::upscale::job::{
    VideoUpscaleJob, VideoUpscaleMessage, VideoUpscaleOptions, VideoUpscalePreflight,
    VideoUpscaleProgressShared, VideoUpscaleQuality, VideoUpscaleScale, preflight, run_job,
};
use crate::video::upscale::paths::{manifest_path_for, work_dir_for};
use crate::video::upscale::queue::{FailureReason, TaskQueue, TaskState};
use crate::video::upscale::sidecar::{derived_sidecar_path_for, derived_video_path_for};

pub(crate) enum VideoUpscalePhase {
    Probing,
    Configure,
    Error { message: String },
}

pub(crate) struct VideoUpscaleState {
    pub source_path: PathBuf,
    pub options: VideoUpscaleOptions,
    pub preflight: Option<VideoUpscalePreflight>,
    pub phase: VideoUpscalePhase,
    pub rx: mpsc::Receiver<VideoUpscaleMessage>,
}

pub(crate) struct VideoUpscaleRunningTask {
    pub task_id: Uuid,
    pub source_path: PathBuf,
    pub cancel: Arc<AtomicBool>,
    pub pause: Arc<AtomicBool>,
    pub progress: Arc<VideoUpscaleProgressShared>,
    pub rx: mpsc::Receiver<VideoUpscaleMessage>,
    pub delete_artifacts_after_cancel: bool,
}

pub(crate) fn recover_video_upscale_queue_for_startup(queue: &mut TaskQueue) {
    queue.tasks.retain_mut(|task| {
        match task.state {
            TaskState::Planning | TaskState::Running => {
                task.state = TaskState::Queued;
                task.failure_reason = None;
            }
            TaskState::Canceling => return false,
            _ => {}
        }
        true
    });
}

impl App {
    pub(crate) fn request_video_upscale(&mut self, source_path: PathBuf) {
        if self.video_upscale.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let probe_path = source_path.clone();
        thread::spawn(move || {
            let result = preflight(&probe_path).map_err(|e| e.to_string());
            let _ = tx.send(VideoUpscaleMessage::PreflightDone(result));
        });
        self.video_upscale = Some(VideoUpscaleState {
            source_path,
            options: VideoUpscaleOptions::default(),
            preflight: None,
            phase: VideoUpscalePhase::Probing,
            rx,
        });
    }

    pub(crate) fn request_video_upscale_artifact_delete(&mut self, source_path: PathBuf) {
        let running_same_source = self
            .video_upscale_running
            .as_ref()
            .is_some_and(|running| same_path_ci(&running.source_path, &source_path));

        let mut removed_queue_task = false;
        self.video_upscale_queue.tasks.retain(|task| {
            let keep = !same_path_ci(&task.source_path, &source_path)
                || (running_same_source
                    && self
                        .video_upscale_running
                        .as_ref()
                        .is_some_and(|running| running.task_id == task.task_id));
            removed_queue_task |= !keep;
            keep
        });
        if removed_queue_task {
            self.save_video_upscale_queue();
        }

        if let Some(running) = self.video_upscale_running.as_mut()
            && same_path_ci(&running.source_path, &source_path)
        {
            running.cancel.store(true, Ordering::Relaxed);
            running.delete_artifacts_after_cancel = true;
            self.video_upscale_queue
                .mark_state(running.task_id, TaskState::Canceling);
            self.save_video_upscale_queue();
            self.show_feedback_toast("[変換中止後にアップスケールを削除]".to_owned());
            return;
        }

        self.enqueue_video_upscale_artifact_delete(source_path);
        self.show_feedback_toast("[アップスケール削除を開始]".to_owned());
    }

    pub(crate) fn show_video_upscale_dialog(&mut self, ctx: &egui::Context) {
        self.poll_video_upscale_registration_messages();

        let Some(state) = self.video_upscale.as_ref() else {
            return;
        };
        let src_name = state
            .source_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_owned();
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let mut open = true;
        let mut should_close = false;
        let mut register = false;
        let mut overwrite = state.options.overwrite;
        let mut scale = state.options.scale;
        let mut quality = state.options.quality;

        egui::Window::new("AI動画アップスケール登録")
            .id(egui::Id::new("video_upscale_register_dialog"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(ctx.content_rect().min + egui::vec2(72.0, 56.0))
            .show(ctx, |ui| {
                ui.set_min_width(460.0);
                ui.label(format!("入力: {src_name}"));
                ui.add_space(8.0);

                match &state.phase {
                    VideoUpscalePhase::Probing => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("動画情報を確認しています...");
                        });
                        ctx.request_repaint_after(Duration::from_millis(100));
                    }
                    VideoUpscalePhase::Configure => {
                        if let Some(preflight) = &state.preflight {
                            render_options(ui, &mut scale, &mut quality, &mut overwrite);
                            ui.add_space(8.0);
                            let (out_w, out_h) = preflight.output_size(scale);
                            let allowed = preflight.info.output_allowed(scale);
                            ui.separator();
                            ui.label(format!(
                                "出力解像度: {}x{} -> {}x{}",
                                preflight.info.width, preflight.info.height, out_w, out_h
                            ));
                            if let Some(bytes) = preflight.estimate_encode_bytes(scale, quality) {
                                ui.label(format!(
                                    "推定ファイルサイズ: 約{}",
                                    crate::ui_helpers::format_bytes(bytes)
                                ));
                            }
                            if let Some(frames) = preflight.info.estimated_frames {
                                ui.label(format!("フレーム数: {frames}"));
                            }
                            ui.label(
                                egui::RichText::new(
                                    "低ビットレート・ノイズが多い動画では効果が限定的です。",
                                )
                                .color(egui::Color32::from_rgb(210, 150, 60)),
                            );
                            ui.label("音声は最終出力時に元動画からコピーします。");
                            ui.label(format!(
                                "出力: {}",
                                preflight
                                    .output_path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("")
                            ));
                            if !allowed {
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(
                                        "出力解像度が8K UHD上限を超えるため登録できません。",
                                    )
                                    .color(egui::Color32::from_rgb(210, 80, 80)),
                                );
                            }
                            if preflight.output_path.exists() && !overwrite {
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(
                                        "出力ファイルがすでに存在します。上書きを有効にしてください。",
                                    )
                                    .color(egui::Color32::from_rgb(210, 150, 60)),
                                );
                            }
                            if self.video_upscale_queue_lock.is_none() {
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(
                                        "別の mIV がアップスケールキューを使用中のため登録できません。",
                                    )
                                    .color(egui::Color32::from_rgb(210, 80, 80)),
                                );
                            }
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                let can_register = allowed
                                    && (!preflight.output_path.exists() || overwrite)
                                    && self.video_upscale_queue_lock.is_some();
                                if ui
                                    .add_enabled(can_register, egui::Button::new("キューに登録"))
                                    .clicked()
                                {
                                    register = true;
                                }
                                if ui.button("キャンセル").clicked() {
                                    should_close = true;
                                }
                            });
                        }
                    }
                    VideoUpscalePhase::Error { message } => {
                        ui.label(
                            egui::RichText::new(message.as_str())
                                .color(egui::Color32::from_rgb(210, 80, 80)),
                        );
                        ui.add_space(8.0);
                        if ui.button("閉じる").clicked() {
                            should_close = true;
                        }
                    }
                }
            });

        if let Some(state) = self.video_upscale.as_mut() {
            state.options.scale = scale;
            state.options.quality = quality;
            state.options.overwrite = overwrite;
        }

        if register {
            self.register_current_video_upscale_task();
        }
        if !open || escape_pressed {
            should_close = true;
        }
        if should_close {
            self.video_upscale = None;
        }
    }

    pub(crate) fn show_video_upscale_tasks_window(&mut self, ctx: &egui::Context) {
        if !self.show_video_upscale_tasks {
            return;
        }

        let mut open = true;
        let tasks = self.video_upscale_queue.tasks.clone();
        let queue_paused = self.video_upscale_queue.paused;
        let running_task_id = self.video_upscale_running.as_ref().map(|r| r.task_id);
        let running_progress = self.video_upscale_running.as_ref().map(|r| {
            (
                r.task_id,
                r.progress.snapshot(),
                r.cancel.load(Ordering::Relaxed),
            )
        });
        let mut action = TaskUiAction::None;

        egui::Window::new("アップスケールタスク")
            .id(egui::Id::new("video_upscale_tasks_window"))
            .open(&mut open)
            .resizable(true)
            .default_size(egui::vec2(620.0, 420.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("{} 件", tasks.len()));
                    let pause_label = if queue_paused {
                        "再開"
                    } else {
                        "一時停止"
                    };
                    if ui.button(pause_label).clicked() {
                        action = TaskUiAction::TogglePause;
                    }
                    if self.video_upscale_running.is_some() {
                        ui.spinner();
                    }
                    if queue_paused {
                        ui.label(
                            egui::RichText::new("一時停止中")
                                .color(egui::Color32::from_rgb(210, 150, 60)),
                        );
                    }
                    if self.video_upscale_queue_lock.is_none() {
                        ui.label(
                            egui::RichText::new("別の mIV がキューを使用中です")
                                .color(egui::Color32::from_rgb(210, 80, 80)),
                        );
                    }
                });
                ui.separator();

                if tasks.is_empty() {
                    ui.label("アップスケールタスクはありません。");
                    return;
                }

                egui::ScrollArea::vertical().show(ui, |ui| {
                    for task in &tasks {
                        render_task_row(ui, task, running_task_id, running_progress, &mut action);
                        ui.separator();
                    }
                });
            });

        self.show_video_upscale_tasks = open;
        self.apply_video_upscale_task_action(action);
    }

    pub(crate) fn poll_video_upscale_queue(&mut self, ctx: &egui::Context) {
        self.poll_video_upscale_delete_results();
        self.poll_video_upscale_running_result();
        if self.video_upscale_running.is_none() {
            self.start_next_video_upscale_task();
        }
        if self.video_upscale_running.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    pub(crate) fn stop_video_upscale_queue_for_exit(&mut self) {
        if let Some(running) = self.video_upscale_running.as_ref() {
            running.cancel.store(true, Ordering::Relaxed);
            let _ = self
                .video_upscale_queue
                .mark_state(running.task_id, TaskState::Queued);
            self.save_video_upscale_queue();
        }
    }

    fn poll_video_upscale_registration_messages(&mut self) {
        let Some(state) = self.video_upscale.as_mut() else {
            return;
        };
        while let Ok(msg) = state.rx.try_recv() {
            match msg {
                VideoUpscaleMessage::PreflightDone(Ok(preflight)) => {
                    state.preflight = Some(preflight);
                    state.phase = VideoUpscalePhase::Configure;
                }
                VideoUpscaleMessage::PreflightDone(Err(e)) => {
                    state.phase = VideoUpscalePhase::Error { message: e };
                }
                VideoUpscaleMessage::Finished(_) => {}
            }
        }
    }

    fn register_current_video_upscale_task(&mut self) {
        if self.video_upscale_queue_lock.is_none() {
            if let Some(state) = self.video_upscale.as_mut() {
                state.phase = VideoUpscalePhase::Error {
                    message: "別の mIV がアップスケールキューを使用中です。".to_owned(),
                };
            }
            return;
        }
        let Some((preflight, options)) = self
            .video_upscale
            .as_ref()
            .and_then(|state| state.preflight.clone().map(|p| (p, state.options.clone())))
        else {
            return;
        };
        let options = options.normalized_for_video_export();
        if self.video_upscale_queue.tasks.iter().any(|task| {
            same_path_ci(&task.source_path, &preflight.source_path)
                && !matches!(task.state, TaskState::Done | TaskState::Failed)
        }) {
            if let Some(state) = self.video_upscale.as_mut() {
                state.phase = VideoUpscalePhase::Error {
                    message: "この動画はすでにキューに登録されています。".to_owned(),
                };
            }
            return;
        }

        self.video_upscale_queue.push_task(
            preflight.source_path.clone(),
            manifest_path_for(&preflight.source_path),
            options,
        );
        self.save_video_upscale_queue();
        self.show_feedback_toast("[アップスケールをキューに登録]".to_owned());
        self.show_video_upscale_tasks = true;
        self.video_upscale = None;
    }

    fn save_video_upscale_queue(&mut self) {
        if let Err(e) = self
            .video_upscale_queue
            .save_atomic(&self.video_upscale_queue_path)
        {
            crate::logger::log(format!("[VideoUpscale] queue save failed: {e}"));
        }
    }

    fn start_next_video_upscale_task(&mut self) {
        if self.video_upscale_queue.paused || self.video_upscale_queue_lock.is_none() {
            return;
        }
        let Some(task) = self
            .video_upscale_queue
            .tasks
            .iter()
            .find(|task| task.state == TaskState::Queued)
            .cloned()
        else {
            return;
        };

        self.ensure_ai_runtime();
        let Some(runtime) = self.ai_runtime.clone() else {
            self.video_upscale_queue
                .mark_failed(task.task_id, FailureReason::Io);
            self.save_video_upscale_queue();
            return;
        };
        let model_manager = self.ai_model_manager.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(self.video_upscale_queue.paused));
        let progress = Arc::new(VideoUpscaleProgressShared::new(None));
        let (tx, rx) = mpsc::channel();
        let task_for_worker = task.clone();
        let cancel_worker = cancel.clone();
        let pause_worker = pause.clone();
        let progress_worker = progress.clone();
        let parallel_segments_worker = Arc::new(AtomicU8::new(1));

        self.video_upscale_queue
            .mark_state(task.task_id, TaskState::Running);
        self.save_video_upscale_queue();

        thread::spawn(move || {
            let result = preflight(&task_for_worker.source_path).and_then(|pre| {
                progress_worker
                    .frames_total
                    .store(pre.info.estimated_frames.unwrap_or(0), Ordering::Relaxed);
                let job = VideoUpscaleJob {
                    source_path: pre.source_path,
                    output_path: pre.output_path,
                    sidecar_path: pre.sidecar_path,
                    info: pre.info,
                    options: task_for_worker.options.normalized_for_video_export(),
                    parallel_segments: parallel_segments_worker,
                    pause: pause_worker,
                };
                run_job(job, runtime, model_manager, cancel_worker, progress_worker)
            });
            let _ = tx.send(VideoUpscaleMessage::Finished(result));
        });

        self.video_upscale_running = Some(VideoUpscaleRunningTask {
            task_id: task.task_id,
            source_path: task.source_path,
            cancel,
            pause,
            progress,
            rx,
            delete_artifacts_after_cancel: false,
        });
    }

    fn poll_video_upscale_running_result(&mut self) {
        let Some(running) = self.video_upscale_running.as_ref() else {
            return;
        };
        let Ok(msg) = running.rx.try_recv() else {
            return;
        };
        let running = self
            .video_upscale_running
            .take()
            .expect("running task exists");
        let VideoUpscaleMessage::Finished(result) = msg else {
            return;
        };

        let task_state = self
            .video_upscale_queue
            .tasks
            .iter()
            .find(|task| task.task_id == running.task_id)
            .map(|task| task.state);
        if matches!(task_state, Some(TaskState::Canceling)) {
            if running.delete_artifacts_after_cancel {
                self.enqueue_video_upscale_artifact_delete(running.source_path.clone());
            } else {
                cleanup_video_upscale_work_dir(running.source_path.clone());
            }
            self.video_upscale_queue.remove_task(running.task_id);
            self.save_video_upscale_queue();
            return;
        }

        match result {
            Ok(output_path) => {
                self.video_upscale_queue.mark_done(running.task_id);
                self.save_video_upscale_queue();
                self.reload_folder_after_video_upscale(&output_path);
            }
            Err(e) => {
                crate::logger::log(format!("[VideoUpscale] task failed: {e}"));
                let failure_reason = failure_reason_from_error_text(&e);
                self.video_upscale_queue
                    .mark_failed(running.task_id, failure_reason);
                self.save_video_upscale_queue();
            }
        }
    }

    fn enqueue_video_upscale_artifact_delete(&mut self, source_path: PathBuf) {
        let output_path = derived_video_path_for(&source_path);
        let sidecar_path = derived_sidecar_path_for(&source_path);
        let work_dir = work_dir_for(&source_path);
        let (tx, rx) = mpsc::channel();
        let reload_path = output_path.clone();
        thread::spawn(move || {
            let _ = std::fs::remove_file(&output_path);
            let _ = std::fs::remove_file(&sidecar_path);
            if work_dir.exists() {
                let _ = std::fs::remove_dir_all(&work_dir);
            }
            let _ = tx.send(reload_path);
        });
        self.video_upscale_delete_pending.push(rx);
    }

    fn poll_video_upscale_delete_results(&mut self) {
        let mut completed = Vec::new();
        let mut i = 0;
        while i < self.video_upscale_delete_pending.len() {
            match self.video_upscale_delete_pending[i].try_recv() {
                Ok(path) => {
                    completed.push(path);
                    self.video_upscale_delete_pending.remove(i);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.video_upscale_delete_pending.remove(i);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    i += 1;
                }
            }
        }
        for path in completed {
            self.reload_folder_after_video_upscale(&path);
            self.show_feedback_toast("[アップスケール削除完了]".to_owned());
        }
    }

    fn apply_video_upscale_task_action(&mut self, action: TaskUiAction) {
        match action {
            TaskUiAction::None => {}
            TaskUiAction::TogglePause => {
                self.video_upscale_queue.paused = !self.video_upscale_queue.paused;
                if let Some(running) = self.video_upscale_running.as_ref() {
                    running
                        .pause
                        .store(self.video_upscale_queue.paused, Ordering::Relaxed);
                }
                self.save_video_upscale_queue();
            }
            TaskUiAction::Cancel(task_id) => {
                if let Some(running) = self.video_upscale_running.as_ref()
                    && running.task_id == task_id
                {
                    running.cancel.store(true, Ordering::Relaxed);
                    self.video_upscale_queue
                        .mark_state(task_id, TaskState::Canceling);
                } else {
                    self.video_upscale_queue.remove_task(task_id);
                }
                self.save_video_upscale_queue();
            }
            TaskUiAction::Remove(task_id) => {
                self.video_upscale_queue.remove_task(task_id);
                self.save_video_upscale_queue();
            }
            TaskUiAction::Retry(task_id) => {
                self.video_upscale_queue
                    .mark_state(task_id, TaskState::Queued);
                self.save_video_upscale_queue();
            }
            TaskUiAction::MoveUp(task_id) => {
                if self.video_upscale_queue.move_task_up(task_id) {
                    self.save_video_upscale_queue();
                }
            }
            TaskUiAction::MoveDown(task_id) => {
                if self.video_upscale_queue.move_task_down(task_id) {
                    self.save_video_upscale_queue();
                }
            }
        }
    }

    fn reload_folder_after_video_upscale(&mut self, output_path: &Path) {
        let Some(current_folder) = self.current_folder.as_ref() else {
            return;
        };
        let Some(output_folder) = output_path.parent() else {
            return;
        };
        if !same_path_ci(current_folder, output_folder) {
            return;
        }
        self.select_after_load = output_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_owned());
        self.reload_current_folder_preserving_override();
    }
}

fn cleanup_video_upscale_work_dir(source_path: PathBuf) {
    thread::spawn(move || {
        let work_dir = work_dir_for(&source_path);
        if work_dir.exists() {
            let _ = std::fs::remove_dir_all(work_dir);
        }
    });
}

#[derive(Clone, Copy)]
enum TaskUiAction {
    None,
    TogglePause,
    Cancel(Uuid),
    Remove(Uuid),
    Retry(Uuid),
    MoveUp(Uuid),
    MoveDown(Uuid),
}

fn render_task_row(
    ui: &mut egui::Ui,
    task: &crate::video::upscale::queue::VideoUpscaleTask,
    running_task_id: Option<Uuid>,
    running_progress: Option<(Uuid, (u64, u64, u64, Duration), bool)>,
    action: &mut TaskUiAction,
) {
    let is_running = running_task_id == Some(task.task_id);
    let file_name = task
        .source_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let state_label = state_label(task.state);

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(file_name).strong());
        ui.label(state_label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if matches!(task.state, TaskState::Queued | TaskState::Running) {
                let cancel_label = if task.state == TaskState::Queued {
                    "キューから外す"
                } else {
                    "中止"
                };
                if ui.button(cancel_label).clicked() {
                    *action = TaskUiAction::Cancel(task.task_id);
                }
            } else if task.state == TaskState::Failed {
                if ui.button("削除").clicked() {
                    *action = TaskUiAction::Remove(task.task_id);
                }
                if ui.button("再試行").clicked() {
                    *action = TaskUiAction::Retry(task.task_id);
                }
            } else if matches!(task.state, TaskState::Done | TaskState::Canceling) {
                let enabled = task.state != TaskState::Canceling;
                if ui
                    .add_enabled(enabled, egui::Button::new("一覧から削除"))
                    .clicked()
                {
                    *action = TaskUiAction::Remove(task.task_id);
                }
            }
            if task.state == TaskState::Queued {
                if ui.button("↓").on_hover_text("順序を下げる").clicked() {
                    *action = TaskUiAction::MoveDown(task.task_id);
                }
                if ui.button("↑").on_hover_text("順序を上げる").clicked() {
                    *action = TaskUiAction::MoveUp(task.task_id);
                }
            }
        });
    });

    ui.horizontal(|ui| {
        if let Some((_, (done, total, rate_base, elapsed), canceled)) =
            running_progress.filter(|(id, _, _)| *id == task.task_id)
        {
            let frac = if total > 0 {
                (done as f32 / total as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            ui.add_sized(
                [180.0, 16.0],
                egui::ProgressBar::new(frac).show_percentage(),
            );
            if total > 0 {
                ui.label(format!("{done}/{total} frame"));
            } else {
                ui.label(format!("{done} frame"));
            }
            if let Some((fps, remaining)) = progress_rate(done, total, rate_base, elapsed) {
                ui.label(format!("{fps:.2} fps"));
                ui.label(format!("残り: {}", format_eta(remaining)));
            } else if total > done {
                ui.label("残り: 計算中...");
            }
            if canceled {
                ui.label("中止中");
            }
        } else if is_running {
            ui.spinner();
            ui.label("開始中");
        } else {
            match task.state {
                TaskState::Done => {
                    ui.add_sized(
                        [180.0, 16.0],
                        egui::ProgressBar::new(1.0).show_percentage().text("完了"),
                    );
                    ui.label("出力済み");
                }
                TaskState::Failed => {
                    ui.add_sized([180.0, 16.0], egui::ProgressBar::new(0.0));
                    ui.label(
                        task.failure_reason
                            .map(failure_reason_label)
                            .unwrap_or("失敗"),
                    );
                }
                _ => {
                    ui.add_sized([180.0, 16.0], egui::ProgressBar::new(0.0));
                    ui.label("待機中");
                }
            }
        }
        ui.label(format!(
            "{} / {}",
            task.options.scale.label(),
            task.options.quality.label()
        ));
    });
}

fn failure_reason_from_error_text(error: &str) -> FailureReason {
    let lower = error.to_lowercase();
    if lower.contains("segment plan drift") || lower.contains("plan_drift") {
        FailureReason::PlanDrift
    } else if lower.contains("unsupported segment manifest schema")
        || lower.contains("schema")
        || lower.contains("スキーマ")
    {
        FailureReason::SchemaMismatch
    } else if lower.contains("stale") || lower.contains("元動画の情報") {
        FailureReason::StaleSource
    } else if lower.contains("no space")
        || lower.contains("not enough space")
        || lower.contains("容量")
        || lower.contains("ディスク")
    {
        FailureReason::NoSpace
    } else if lower.contains("audio") || lower.contains("音声") {
        FailureReason::AudioMux
    } else {
        FailureReason::Io
    }
}

fn failure_reason_label(reason: FailureReason) -> &'static str {
    match reason {
        FailureReason::SchemaMismatch => "失敗: タスク形式が古い",
        FailureReason::StaleSource => "失敗: 元動画が変更されています",
        FailureReason::AudioMux => "失敗: 音声コピー",
        FailureReason::NoSpace => "失敗: 容量不足",
        FailureReason::PlanDrift => "失敗: セグメント計画不一致",
        FailureReason::Io => "失敗: I/O",
    }
}

fn format_eta(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{hours}時間{minutes}分")
    } else if minutes > 0 {
        format!("{minutes}分{seconds}秒")
    } else {
        format!("{seconds}秒")
    }
}

fn progress_rate(
    done: u64,
    total: u64,
    rate_base: u64,
    elapsed: Duration,
) -> Option<(f64, Duration)> {
    if total <= done || done <= rate_base || elapsed.as_secs_f64() <= 0.0 {
        return None;
    }
    let processed = done - rate_base;
    let fps = processed as f64 / elapsed.as_secs_f64();
    if !fps.is_finite() || fps <= 0.0 {
        return None;
    }
    let remaining_secs = (total - done) as f64 / fps;
    Some((fps, Duration::from_secs_f64(remaining_secs.max(0.0))))
}

fn state_label(state: TaskState) -> &'static str {
    match state {
        TaskState::Queued => "待機中",
        TaskState::Planning => "計画中",
        TaskState::Running => "変換中",
        TaskState::Paused => "一時停止",
        TaskState::Canceling => "中止中",
        TaskState::Failed => "失敗",
        TaskState::Done => "完了",
    }
}

fn render_options(
    ui: &mut egui::Ui,
    scale: &mut VideoUpscaleScale,
    quality: &mut VideoUpscaleQuality,
    overwrite: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.label("倍率");
        ui.selectable_value(scale, VideoUpscaleScale::X2, VideoUpscaleScale::X2.label());
        ui.selectable_value(scale, VideoUpscaleScale::X4, VideoUpscaleScale::X4.label());
    });
    egui::ComboBox::from_label("圧縮率")
        .selected_text(quality.label())
        .show_ui(ui, |ui| {
            ui.selectable_value(
                quality,
                VideoUpscaleQuality::Q1,
                VideoUpscaleQuality::Q1.label(),
            );
            ui.selectable_value(
                quality,
                VideoUpscaleQuality::Q2,
                VideoUpscaleQuality::Q2.label(),
            );
            ui.selectable_value(
                quality,
                VideoUpscaleQuality::Q3,
                VideoUpscaleQuality::Q3.label(),
            );
            ui.selectable_value(
                quality,
                VideoUpscaleQuality::Q4,
                VideoUpscaleQuality::Q4.label(),
            );
            ui.selectable_value(
                quality,
                VideoUpscaleQuality::Q5,
                VideoUpscaleQuality::Q5.label(),
            );
        });
    ui.checkbox(overwrite, "既存の出力ファイルを上書き");
}

fn same_path_ci(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::upscale::queue::VideoUpscaleTask;

    fn task_with_state(state: TaskState) -> VideoUpscaleTask {
        VideoUpscaleTask {
            task_id: Uuid::new_v4(),
            source_path: PathBuf::from("movie.mp4"),
            manifest_path: PathBuf::from("movie.miv.work/job.miv-upscale.json"),
            options: VideoUpscaleOptions::default(),
            state,
            failure_reason: if state == TaskState::Failed {
                Some(FailureReason::Io)
            } else {
                None
            },
            created_unix_ms: 1,
            updated_unix_ms: 2,
        }
    }

    #[test]
    fn startup_recovery_resets_in_progress_tasks_and_drops_canceling() {
        let mut queue = TaskQueue {
            schema: crate::video::upscale::queue::QUEUE_SCHEMA_VERSION,
            paused: true,
            parallel_segments: 1,
            tasks: vec![
                task_with_state(TaskState::Planning),
                task_with_state(TaskState::Running),
                task_with_state(TaskState::Canceling),
                task_with_state(TaskState::Done),
                task_with_state(TaskState::Failed),
            ],
        };

        recover_video_upscale_queue_for_startup(&mut queue);

        assert_eq!(queue.tasks.len(), 4);
        assert_eq!(queue.tasks[0].state, TaskState::Queued);
        assert_eq!(queue.tasks[1].state, TaskState::Queued);
        assert_eq!(queue.tasks[2].state, TaskState::Done);
        assert_eq!(queue.tasks[3].state, TaskState::Failed);
        assert_eq!(queue.tasks[0].failure_reason, None);
        assert_eq!(queue.tasks[1].failure_reason, None);
    }

    #[test]
    fn failure_reason_is_classified_from_error_text() {
        assert_eq!(
            failure_reason_from_error_text("segment plan drift: planned 3 frames"),
            FailureReason::PlanDrift
        );
        assert_eq!(
            failure_reason_from_error_text("音声packetの書き込みに失敗しました"),
            FailureReason::AudioMux
        );
        assert_eq!(
            failure_reason_from_error_text("ディスク容量が不足しています"),
            FailureReason::NoSpace
        );
        assert_eq!(
            failure_reason_from_error_text("unsupported segment manifest schema"),
            FailureReason::SchemaMismatch
        );
    }

    #[test]
    fn eta_format_uses_compact_japanese_units() {
        assert_eq!(format_eta(Duration::from_secs(45)), "45秒");
        assert_eq!(format_eta(Duration::from_secs(12 * 60 + 34)), "12分34秒");
        assert_eq!(format_eta(Duration::from_secs(3600 + 23 * 60)), "1時間23分");
    }

    #[test]
    fn progress_rate_uses_processed_delta_not_cumulative_done() {
        let (fps, remaining) = progress_rate(1_010, 2_000, 1_000, Duration::from_secs(2)).unwrap();

        assert!((fps - 5.0).abs() < f64::EPSILON);
        assert_eq!(remaining.as_secs(), 198);
        assert!(progress_rate(1_000, 2_000, 1_000, Duration::from_secs(2)).is_none());
    }
}
