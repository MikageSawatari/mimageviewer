use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use eframe::egui;

use crate::app::App;
use crate::video::upscale::job::{
    VideoUpscaleJob, VideoUpscaleMessage, VideoUpscaleModelPreset, VideoUpscaleOptions,
    VideoUpscalePreflight, VideoUpscaleProgressShared, VideoUpscaleQuality, VideoUpscaleScale,
    preflight, run_job,
};

pub(crate) enum VideoUpscalePhase {
    Probing,
    Confirm,
    Running {
        progress: Arc<VideoUpscaleProgressShared>,
        cancel: Arc<AtomicBool>,
    },
    Canceling {
        progress: Arc<VideoUpscaleProgressShared>,
        cancel: Arc<AtomicBool>,
    },
    Finished {
        output_path: PathBuf,
    },
    Error {
        message: String,
    },
}

pub(crate) struct VideoUpscaleState {
    pub source_path: PathBuf,
    pub options: VideoUpscaleOptions,
    pub preflight: Option<VideoUpscalePreflight>,
    pub phase: VideoUpscalePhase,
    pub rx: mpsc::Receiver<VideoUpscaleMessage>,
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

    pub(crate) fn show_video_upscale_dialog(&mut self, ctx: &egui::Context) {
        self.poll_video_upscale_messages();

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
        let mut start = false;
        let mut cancel = false;
        let mut overwrite = state.options.overwrite;
        let mut scale = state.options.scale;
        let mut model = state.options.model;
        let mut quality = state.options.quality;

        egui::Window::new("AI動画アップスケール")
            .id(egui::Id::new("video_upscale_dialog"))
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
                    VideoUpscalePhase::Confirm => {
                        if let Some(preflight) = &state.preflight {
                            render_options(ui, &mut scale, &mut model, &mut quality, &mut overwrite);
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
                                    "推定ファイルサイズ: 約 {}",
                                    crate::ui_helpers::format_bytes(bytes)
                                ));
                            }
                            if let Some(frames) = preflight.info.estimated_frames {
                                ui.label(format!(
                                    "フレーム数: {}。残り時間は開始後の実測速度から表示します。",
                                    frames
                                ));
                            } else {
                                ui.label("残り時間は開始後の実測速度から表示します。");
                            }
                            ui.label("音声はMVPでは含めません。映像のみのMKVとして保存します。");
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
                                    egui::RichText::new("出力解像度が8K UHD上限を超えるため実行できません。")
                                        .color(egui::Color32::from_rgb(210, 80, 80)),
                                );
                            }
                            if preflight.output_path.exists() && !overwrite {
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new("出力ファイルがすでに存在します。上書きを有効にしてください。")
                                        .color(egui::Color32::from_rgb(210, 150, 60)),
                                );
                            }
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                let can_start = allowed && (!preflight.output_path.exists() || overwrite);
                                if ui.add_enabled(can_start, egui::Button::new("変換開始")).clicked() {
                                    start = true;
                                }
                                if ui.button("キャンセル").clicked() {
                                    should_close = true;
                                }
                            });
                        }
                    }
                    VideoUpscalePhase::Running { progress, .. } => {
                        render_progress(ui, progress);
                        ui.add_space(8.0);
                        if ui.button("中止").clicked() {
                            cancel = true;
                        }
                        ctx.request_repaint_after(Duration::from_millis(120));
                    }
                    VideoUpscalePhase::Canceling { progress, .. } => {
                        render_progress(ui, progress);
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("中止処理中です...");
                        });
                        ctx.request_repaint_after(Duration::from_millis(120));
                    }
                    VideoUpscalePhase::Finished { output_path } => {
                        ui.label("変換が完了しました。");
                        ui.label(format!("出力: {}", output_path.display()));
                        ui.add_space(8.0);
                        if ui.button("閉じる").clicked() {
                            should_close = true;
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
            state.options.model = model;
            state.options.quality = quality;
            state.options.overwrite = overwrite;
        }

        if start {
            self.start_video_upscale();
        }
        if cancel {
            if let Some(state) = self.video_upscale.as_mut() {
                let canceling = match &state.phase {
                    VideoUpscalePhase::Running { progress, cancel } => {
                        cancel.store(true, Ordering::Relaxed);
                        Some((progress.clone(), cancel.clone()))
                    }
                    _ => None,
                };
                if let Some((progress, cancel)) = canceling {
                    state.phase = VideoUpscalePhase::Canceling { progress, cancel };
                }
            }
        }
        if !open || escape_pressed {
            should_close = true;
        }
        if should_close {
            let mut close_now = true;
            if let Some(state) = self.video_upscale.as_mut() {
                let canceling = match &state.phase {
                    VideoUpscalePhase::Running { progress, cancel } => {
                        cancel.store(true, Ordering::Relaxed);
                        Some((progress.clone(), cancel.clone()))
                    }
                    VideoUpscalePhase::Canceling { .. } => {
                        close_now = false;
                        None
                    }
                    _ => None,
                };
                if let Some((progress, cancel)) = canceling {
                    state.phase = VideoUpscalePhase::Canceling { progress, cancel };
                    close_now = false;
                }
            }
            if close_now {
                self.video_upscale = None;
            }
        }
    }

    fn poll_video_upscale_messages(&mut self) {
        let Some(state) = self.video_upscale.as_mut() else {
            return;
        };
        while let Ok(msg) = state.rx.try_recv() {
            match msg {
                VideoUpscaleMessage::PreflightDone(Ok(preflight)) => {
                    state.preflight = Some(preflight);
                    state.phase = VideoUpscalePhase::Confirm;
                }
                VideoUpscaleMessage::PreflightDone(Err(e)) => {
                    state.phase = VideoUpscalePhase::Error { message: e };
                }
                VideoUpscaleMessage::Finished(Ok(path)) => match &state.phase {
                    VideoUpscalePhase::Running { .. } => {
                        state.phase = VideoUpscalePhase::Finished { output_path: path };
                    }
                    VideoUpscalePhase::Canceling { .. } => {
                        self.video_upscale = None;
                        return;
                    }
                    _ => {}
                },
                VideoUpscaleMessage::Finished(Err(e)) => {
                    if matches!(state.phase, VideoUpscalePhase::Canceling { .. }) {
                        self.video_upscale = None;
                        return;
                    }
                    state.phase = VideoUpscalePhase::Error { message: e };
                }
            }
        }
    }

    fn start_video_upscale(&mut self) {
        let Some((preflight, options)) = self
            .video_upscale
            .as_ref()
            .and_then(|state| state.preflight.clone().map(|p| (p, state.options.clone())))
        else {
            return;
        };
        if !preflight.info.output_allowed(options.scale) {
            if let Some(state) = self.video_upscale.as_mut() {
                state.phase = VideoUpscalePhase::Error {
                    message: "出力解像度が8K UHD上限を超えています。".to_owned(),
                };
            }
            return;
        }

        self.ensure_ai_runtime();
        let Some(runtime) = self.ai_runtime.clone() else {
            if let Some(state) = self.video_upscale.as_mut() {
                state.phase = VideoUpscalePhase::Error {
                    message: "AIランタイムを初期化できませんでした。".to_owned(),
                };
            }
            return;
        };
        let model_manager = self.ai_model_manager.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(VideoUpscaleProgressShared::new(
            preflight.info.estimated_frames,
        ));
        let (tx, rx) = mpsc::channel();
        let job = VideoUpscaleJob {
            source_path: preflight.source_path.clone(),
            output_path: preflight.output_path.clone(),
            sidecar_path: preflight.sidecar_path.clone(),
            info: preflight.info.clone(),
            options,
        };
        let cancel_worker = cancel.clone();
        let progress_worker = progress.clone();
        thread::spawn(move || {
            let result = run_job(job, runtime, model_manager, cancel_worker, progress_worker);
            let _ = tx.send(VideoUpscaleMessage::Finished(result));
        });
        if let Some(state) = self.video_upscale.as_mut() {
            state.rx = rx;
            state.phase = VideoUpscalePhase::Running { progress, cancel };
        }
    }
}

fn render_options(
    ui: &mut egui::Ui,
    scale: &mut VideoUpscaleScale,
    model: &mut VideoUpscaleModelPreset,
    quality: &mut VideoUpscaleQuality,
    overwrite: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.label("倍率");
        ui.selectable_value(scale, VideoUpscaleScale::X2, VideoUpscaleScale::X2.label());
        ui.selectable_value(scale, VideoUpscaleScale::X4, VideoUpscaleScale::X4.label());
    });
    ui.horizontal(|ui| {
        ui.label("モデル");
        ui.selectable_value(
            model,
            VideoUpscaleModelPreset::GeneralFast,
            VideoUpscaleModelPreset::GeneralFast.label(),
        );
        ui.selectable_value(
            model,
            VideoUpscaleModelPreset::Anime,
            VideoUpscaleModelPreset::Anime.label(),
        );
        ui.selectable_value(
            model,
            VideoUpscaleModelPreset::Photo,
            VideoUpscaleModelPreset::Photo.label(),
        );
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

fn render_progress(ui: &mut egui::Ui, progress: &VideoUpscaleProgressShared) {
    let (done, total, elapsed) = progress.snapshot();
    let frac = if total > 0 {
        (done as f32 / total as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ui.add(egui::ProgressBar::new(frac).show_percentage());
    ui.add_space(4.0);
    if total > 0 {
        ui.label(format!("{done} / {total} フレーム"));
    } else {
        ui.label(format!("{done} フレーム処理済み"));
    }
    ui.label(format!("経過時間: {}", format_duration(elapsed)));
    if done > 0 && elapsed.as_secs_f64() > 0.0 {
        let fps = done as f64 / elapsed.as_secs_f64();
        if total > done && fps > 0.0 {
            let remaining = Duration::from_secs_f64((total - done) as f64 / fps);
            ui.label(format!(
                "実測速度: {:.2} fps / 残り時間: 約 {}",
                fps,
                format_duration(remaining)
            ));
        } else {
            ui.label(format!("実測速度: {:.2} fps", fps));
        }
    } else if total > 0 {
        ui.label("残り時間を計測しています...");
    }
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if hours > 0 {
        format!("{hours}時間{minutes}分")
    } else if minutes > 0 {
        format!("{minutes}分{seconds}秒")
    } else {
        format!("{seconds}秒")
    }
}
