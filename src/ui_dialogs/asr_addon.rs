//! モデルの準備と捕捉済み対象の引渡し。取得処理と進捗部品は既存モデルと共用する。
use super::transcript_job::TranscriptTargets;
use crate::ai::asr_worker::{GpuDetection, LoadRequest};
use crate::app::App;
use crate::asr_addon::{AddonEvent, AddonOperation, AddonStatus, AsrAddon, InstallProgress};
use crate::feedback::FeedbackKind;
use eframe::egui;
use std::sync::mpsc;
use std::time::Duration;

const NO_GPU: &str = "この PC には文字起こしに使える Vulkan 対応 GPU がありません";
const NO_VULKAN: &str = "Vulkan に対応したグラフィックスドライバが見つかりません";

#[derive(Default)]
enum Probe {
    #[default]
    Unchecked,
    Checking(mpsc::Receiver<Result<GpuDetection, String>>),
    Ready(Result<GpuDetection, String>),
}
enum Dialog {
    Checking(TranscriptTargets),
    Confirm(TranscriptTargets),
    Downloading(TranscriptTargets),
    Unsupported(String),
    Done,
    Cancelled,
    Error(String),
}
#[derive(Default)]
pub(crate) struct AsrSetup {
    addon: AsrAddon,
    gpu: Probe,
    dialog: Option<Dialog>,
}

impl AsrSetup {
    pub(crate) fn modal_open(&self) -> bool {
        matches!(
            self.dialog,
            Some(Dialog::Checking(_) | Dialog::Confirm(_) | Dialog::Unsupported(_))
        )
    }
    pub(crate) fn blocks_transcript_deletion(&self) -> bool {
        self.dialog.is_some()
            || self.addon.is_scanning()
            || self.addon.operation() != AddonOperation::None
            || matches!(self.gpu, Probe::Checking(_))
    }
    fn check(&mut self, ctx: &egui::Context) {
        self.addon.request_scan();
        if matches!(self.gpu, Probe::Unchecked) {
            let (tx, rx) = mpsc::channel();
            let repaint = ctx.clone();
            match std::thread::Builder::new()
                .name("asr-gpu-probe".into())
                .spawn(move || {
                    let _ = tx.send(crate::ai::asr_worker::detect_gpu().map_err(|e| e.to_string()));
                    repaint.request_repaint();
                }) {
                Ok(_) => self.gpu = Probe::Checking(rx),
                Err(e) => self.gpu = Probe::Ready(Err(e.to_string())),
            }
        }
    }
    fn eligible(&self) -> bool {
        matches!(&self.gpu, Probe::Ready(Ok(GpuDetection::Device(gpu))) if gpu.is_eligible())
    }
}

impl App {
    pub(crate) fn request_asr_setup(&mut self, ctx: &egui::Context, targets: TranscriptTargets) {
        if self.transcript_delete.is_some() {
            return;
        }
        // 取得中の対象を後の要求で置き換えない。
        if self.asr_setup.dialog.is_some()
            || self.asr_setup.addon.operation() != AddonOperation::None
        {
            self.show_feedback_result(
                "モデルの準備が終わってから選び直してください".into(),
                FeedbackKind::Info,
            );
            return;
        }
        if self.asr_setup.eligible()
            && matches!(self.asr_setup.addon.status(), AddonStatus::Valid { .. })
        {
            self.asr_targets_ready(ctx, targets);
            return;
        }
        self.asr_setup.check(ctx);
        self.asr_setup.dialog = Some(Dialog::Checking(targets));
    }
    fn asr_targets_ready(&mut self, ctx: &egui::Context, targets: TranscriptTargets) {
        let AddonStatus::Valid {
            model_id,
            model_path,
            ..
        } = self.asr_setup.addon.status()
        else {
            return;
        };
        let Probe::Ready(Ok(GpuDetection::Device(gpu))) = &self.asr_setup.gpu else {
            return;
        };
        let Some(device) = gpu.device.as_ref().filter(|_| gpu.is_eligible()) else {
            return;
        };
        let config = crate::ai::transcript_job_worker::TranscriptWorkerConfig {
            worker_executable: None,
            load: LoadRequest {
                model_path: model_path.clone(),
                device_luid: device.luid.clone(),
            },
            model_id: model_id.clone(),
            transcript_db_path: crate::transcript_db::TranscriptDb::db_path(),
        };
        self.start_transcript_job(ctx, targets, config);
    }
    pub(crate) fn poll_asr_setup(&mut self, ctx: &egui::Context) {
        if let Probe::Checking(rx) = &self.asr_setup.gpu {
            match rx.try_recv() {
                Ok(result) => self.asr_setup.gpu = Probe::Ready(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.asr_setup.gpu = Probe::Ready(Err("GPU の確認処理が終了しました".into()))
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        for _ in 0..64 {
            let cancelling = self.asr_setup.addon.operation() == AddonOperation::Cancelling;
            let Some(event) = self.asr_setup.addon.poll() else {
                break;
            };
            match event {
                AddonEvent::Installed { .. } => {
                    let targets = match self.asr_setup.dialog.take() {
                        Some(Dialog::Downloading(targets)) if !cancelling => Some(targets),
                        _ => None,
                    };
                    // 対象引渡し後に空の完了窓を残さない。
                    if let Some(targets) = targets.filter(|t| !t.paths.is_empty()) {
                        self.asr_targets_ready(ctx, targets);
                    } else if !cancelling {
                        self.asr_setup.dialog = Some(Dialog::Done);
                    }
                }
                AddonEvent::Cancelled => self.asr_setup.dialog = Some(Dialog::Cancelled),
                AddonEvent::Error { message } => {
                    crate::logger::log(format!("[ASR] addon: {message}"));
                    self.asr_setup.dialog = Some(Dialog::Error(message));
                }
                AddonEvent::Removed | AddonEvent::StatusChanged => {}
            }
        }
        if matches!(self.asr_setup.dialog, Some(Dialog::Checking(_)))
            && !self.asr_setup.addon.is_scanning()
            && matches!(self.asr_setup.gpu, Probe::Ready(_))
        {
            let Some(Dialog::Checking(targets)) = self.asr_setup.dialog.take() else {
                unreachable!()
            };
            match &self.asr_setup.gpu {
                Probe::Ready(Err(error)) => {
                    self.asr_setup.dialog =
                        Some(Dialog::Error(format!("GPU の確認に失敗しました: {error}")))
                }
                Probe::Ready(Ok(GpuDetection::VulkanLoaderUnavailable)) => {
                    self.asr_setup.dialog = Some(Dialog::Unsupported(NO_VULKAN.into()))
                }
                _ if !self.asr_setup.eligible() => {
                    self.asr_setup.dialog = Some(Dialog::Unsupported(NO_GPU.into()))
                }
                _ if matches!(self.asr_setup.addon.status(), AddonStatus::Valid { .. }) => {
                    self.asr_targets_ready(ctx, targets)
                }
                _ => self.asr_setup.dialog = Some(Dialog::Confirm(targets)),
            }
        }
        if self.asr_setup.addon.operation() != AddonOperation::None
            || self.asr_setup.addon.is_scanning()
            || matches!(self.asr_setup.gpu, Probe::Checking(_))
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
    pub(crate) fn show_asr_addon_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.asr_setup.dialog.take() else {
            return;
        };
        match draw_dialog(
            ctx,
            &dialog,
            self.asr_setup.addon.progress(),
            self.asr_setup.addon.operation(),
        ) {
            Action::Download => match self.asr_setup.addon.start_install() {
                Ok(()) => {
                    let targets = match dialog {
                        Dialog::Confirm(t) => t,
                        _ => TranscriptTargets::default(),
                    };
                    dialog = Dialog::Downloading(targets);
                }
                Err(error) => dialog = Dialog::Error(error),
            },
            Action::Cancel => {
                self.asr_setup.addon.cancel();
                dialog = Dialog::Downloading(TranscriptTargets::default());
            }
            Action::Close => {
                if matches!(dialog, Dialog::Downloading(_)) {
                    self.asr_setup.addon.cancel();
                }
                return;
            }
            Action::None => {}
        }
        self.asr_setup.dialog = Some(dialog);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    None,
    Download,
    Cancel,
    Close,
}
fn draw_dialog(
    ctx: &egui::Context,
    dialog: &Dialog,
    progress: &InstallProgress,
    operation: AddonOperation,
) -> Action {
    let mut action = Action::None;
    let mut open = true;
    let content = ctx.content_rect();
    egui::Window::new("文字起こしのモデル")
        .id(egui::Id::unique("asr_model_dialog"))
        .order(egui::Order::Foreground)
        .default_pos(content.min + egui::vec2(40.0, 30.0))
        .default_width(560.0_f32.min(content.width() - 100.0))
        .default_height(360.0_f32.min(content.height() - 100.0))
        .max_height((content.height() - 80.0).max(100.0))
        .collapsible(false)
        .resizable(true)
        .open(&mut open)
        .show(ctx, |ui| {
            let footer = ui.spacing().interact_size.y + ui.spacing().item_spacing.y + 12.0;
            egui::ScrollArea::vertical()
                .auto_shrink([false, true])
                .max_height((ui.available_height() - footer).max(20.0))
                .show(ui, |ui| {
                    let scroll = &mut ui.style_mut().spacing.scroll;
                    scroll.floating_allocated_width = scroll.bar_inner_margin + scroll.bar_width;
                    match dialog {
                        Dialog::Checking(_) => {
                            ui.label("モデルと GPU を確認中...");
                        }
                        Dialog::Confirm(targets) => {
                            ui.label(
                                "動画や音声ファイルの音声を、PC 内の AI が日本語の文字にします。",
                            );
                            ui.label(
                                "音声は PC の外へ出ません。聞き違いや抜けがあることがあります。",
                            );
                            ui.separator();
                            ui.label("取得するもの: whisper large-v3-turbo Q5_0 (約 574 MB)");
                            ui.label("取得元: mImageViewer の GitHub Releases");
                            ui.label("ライセンス: MIT (LICENSE と NOTICE を同時に取得)");
                            ui.label("中断しても次回は続きから取得できます。");
                            if targets.unsupported > 0 {
                                ui.label(format!("対象外: {} 件", targets.unsupported));
                            }
                        }
                        Dialog::Downloading(_) => {
                            super::model_addon::draw_download_status(ui, progress, operation)
                        }
                        Dialog::Unsupported(reason) => {
                            ui.label(reason);
                        }
                        Dialog::Done => {
                            ui.label("モデルのダウンロードが完了しました。");
                        }
                        Dialog::Cancelled => super::model_addon::draw_download_cancelled(ui),
                        Dialog::Error(error) => {
                            ui.colored_label(
                                ui.visuals().error_fg_color,
                                "モデルの準備に失敗しました",
                            );
                            ui.label(error);
                        }
                    }
                });
            ui.separator();
            ui.horizontal_wrapped(|ui| match dialog {
                Dialog::Confirm(_) => {
                    if ui.button("ダウンロード").clicked() {
                        action = Action::Download;
                    }
                    if ui.button("今はしない").clicked() {
                        action = Action::Close;
                    }
                }
                Dialog::Downloading(_) => {
                    if super::model_addon::draw_download_cancel(ui, operation) {
                        action = Action::Cancel;
                    }
                }
                _ => {
                    if ui.button("閉じる").clicked() {
                        action = Action::Close;
                    }
                }
            });
        });
    if !open {
        action = Action::Close;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::{NodeT, Queryable};

    #[test]
    fn asr_setup_small_window_actions_and_gpu_reason() {
        for size in [egui::vec2(1093.0, 614.0), egui::vec2(546.5, 307.0)] {
            for case in 0..4 {
                let (dialog, label) = match case {
                    0 => (
                        Dialog::Confirm(TranscriptTargets::default()),
                        "ダウンロード",
                    ),
                    1 => (
                        Dialog::Downloading(TranscriptTargets::default()),
                        "キャンセル",
                    ),
                    2 => (Dialog::Unsupported(NO_GPU.into()), "閉じる"),
                    _ => (
                        Dialog::Error("長いエラーメッセージと保存先です。".repeat(60)),
                        "閉じる",
                    ),
                };
                let mut harness = crate::test_ui::builder()
                    .with_context_setup(crate::ui_fonts::configure_fonts)
                    .with_size(size)
                    .build_ui_state(
                        move |ui, action| {
                            let result = draw_dialog(
                                ui.ctx(),
                                &dialog,
                                &InstallProgress::Downloading {
                                    bytes_done: 100,
                                    bytes_total: 200,
                                },
                                AddonOperation::Installing,
                            );
                            if result != Action::None {
                                *action = result;
                            }
                        },
                        Action::None,
                    );
                harness.run_steps(4);
                for button in harness.get_all_by_role(egui::accesskit::Role::Button) {
                    assert!(
                        egui::Rect::from_min_size(egui::Pos2::ZERO, size)
                            .contains_rect(button.rect()),
                        "case {case}"
                    );
                }
                if case == 2 {
                    assert!(harness.query_by_label("ダウンロード").is_none());
                    harness.get_by_label(NO_GPU);
                }
                harness.get_by_label(label).click();
                harness.run_steps(2);
                assert_ne!(*harness.state(), Action::None);
            }
        }
    }

    #[test]
    fn asr_setup_modal_boundary_excludes_download() {
        let mut setup = AsrSetup::default();
        setup.dialog = Some(Dialog::Confirm(TranscriptTargets::default()));
        assert!(setup.modal_open());
        setup.dialog = Some(Dialog::Downloading(TranscriptTargets::default()));
        assert!(!setup.modal_open());
        assert!(setup.blocks_transcript_deletion());
    }

    #[test]
    fn asr_setup_keeps_captured_audio_choice_when_playback_choice_changes() {
        let mut app = crate::app::setup_app_for_test();
        let path = app.tmp.path().join("movie.mp4");
        let key = crate::adjustment_db::normalize_path(&path);
        let captured = crate::video::SavedAudioTrackChoice {
            stream_index: 1,
            codec: "aac".into(),
            language: None,
            channels: Some(2),
            title: None,
        };
        app.items = vec![crate::grid_item::GridItem::Video(path.clone())];
        app.selected = Some(0);
        app.settings
            .video_audio_track_choices
            .insert(key.clone(), captured.clone());
        let targets = app.transcript_targets(false);
        app.asr_setup.dialog = Some(Dialog::Confirm(targets));
        app.settings.video_audio_track_choices.insert(
            key,
            crate::video::SavedAudioTrackChoice {
                stream_index: 2,
                ..captured.clone()
            },
        );
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(1093.0, 614.0))
            .build_ui_state(
                |ui, app: &mut crate::app::AppTestEnvForTest| app.show_asr_addon_dialog(ui.ctx()),
                app,
            );
        harness.run_steps(4);
        let Some(Dialog::Confirm(targets)) = &harness.state().asr_setup.dialog else {
            panic!("モデル案内の対象を維持する")
        };
        assert_eq!(targets.paths[0].path, path);
        assert_eq!(targets.paths[0].saved_audio_track, Some(captured));
    }

    #[test]
    fn asr_setup_confirm_snapshot() {
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(1093.0, 614.0))
            .build_ui(|ui| {
                crate::os_theme::apply_resolved(ui.ctx(), crate::os_theme::ResolvedTheme::Dark);
                draw_dialog(
                    ui.ctx(),
                    &Dialog::Confirm(TranscriptTargets {
                        paths: vec!["sample.mp4".into()],
                        unsupported: 2,
                    }),
                    &InstallProgress::Idle,
                    AddonOperation::None,
                );
            });
        harness.run_steps(4);
        harness.snapshot("asr_setup_confirm_dark");
    }
}
