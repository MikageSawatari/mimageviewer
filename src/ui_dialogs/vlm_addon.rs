//! Model setup and captured-target handoff to the App-owned caption job.
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use eframe::egui;

use super::caption_job::CaptionTargets;
use crate::ai::vlm_worker::GpuDetection;
use crate::app::App;
use crate::vlm_addon::{AddonEvent, AddonOperation, AddonStatus, InstallProgress, VlmAddon};

const GPU_REQUIREMENTS_REASON: &str =
    "この PC の GPU では使えません (Vulkan 対応で VRAM 8GB 以上が必要)";
const VULKAN_LOADER_REASON: &str = "Vulkan に対応したグラフィックスドライバが見つかりません";

#[derive(Default)]
enum GpuProbe {
    #[default]
    Unchecked,
    Checking(mpsc::Receiver<Result<GpuDetection, String>>),
    Ready(Result<GpuDetection, String>),
}

#[derive(Debug, PartialEq)]
enum Dialog {
    Checking(CaptionTargets),
    Confirm(CaptionTargets),
    Running(CaptionTargets),
    Unsupported(String),
    Done,
    Cancelled,
    Error(String),
    GpuError(String),
    RemoveConfirm,
}

/// App owns this for its whole lifetime, independently of the visible window.
/// Closing the download window cancels but retains the operation completion barrier.
#[derive(Default)]
pub(crate) struct VlmSetup {
    addon: VlmAddon,
    gpu: GpuProbe,
    dialog: Option<Dialog>,
    folder_result: Option<mpsc::Receiver<Result<(), String>>>,
}

#[derive(Clone, Default)]
pub(crate) struct VlmPreferencesView {
    pub status: String,
    pub gpu: String,
    pub size: String,
    pub busy: bool,
    pub activity: String,
    pub can_remove: bool,
}

impl VlmSetup {
    #[cfg(test)]
    pub(super) fn for_caption_menu_test(root: PathBuf) -> Self {
        Self {
            addon: VlmAddon::with_test_status(root, AddonStatus::Missing),
            gpu: GpuProbe::Ready(Ok(GpuDetection::Device(
                crate::ai::vlm_worker::GpuDecision {
                    device: None,
                    vulkan_available: true,
                    vram_total_bytes: 8 * 1073741824,
                },
            ))),
            ..Default::default()
        }
    }

    #[cfg(test)]
    pub(super) fn captured_caption_targets_for_test(&self) -> Option<&CaptionTargets> {
        match &self.dialog {
            Some(
                Dialog::Checking(targets) | Dialog::Confirm(targets) | Dialog::Running(targets),
            ) => Some(targets),
            _ => None,
        }
    }

    fn installed(&mut self, cancelling: bool) -> Option<CaptionTargets> {
        let targets = match self.dialog.take() {
            Some(Dialog::Running(paths)) if !cancelling => Some(paths),
            _ => None,
        };
        self.dialog = Some(if cancelling {
            Dialog::Cancelled
        } else {
            Dialog::Done
        });
        targets
    }

    pub(crate) fn modal_open(&self) -> bool {
        matches!(
            self.dialog,
            Some(
                Dialog::Checking(_)
                    | Dialog::Confirm(_)
                    | Dialog::Unsupported(_)
                    | Dialog::RemoveConfirm
            )
        )
    }

    /// Even a modeless/terminal model window can cover the deletion modal.
    /// Retain exclusion while preparation workers finish after their window closes.
    pub(crate) fn blocks_caption_deletion(&self) -> bool {
        self.dialog.is_some()
            || self.addon.operation() != AddonOperation::None
            || self.addon.is_scanning()
            || matches!(self.gpu, GpuProbe::Checking(_))
    }

    fn ensure_checked(&mut self, ctx: &egui::Context) {
        self.addon.request_scan();
        if matches!(self.gpu, GpuProbe::Unchecked) {
            let (tx, rx) = mpsc::channel();
            let repaint = ctx.clone();
            match std::thread::Builder::new()
                .name("vlm-gpu-probe".into())
                .spawn(move || {
                    let result = crate::ai::vlm_worker::detect_gpu().map_err(|e| e.to_string());
                    let _ = tx.send(result);
                    repaint.request_repaint();
                }) {
                Ok(_) => self.gpu = GpuProbe::Checking(rx),
                Err(e) => self.gpu = GpuProbe::Ready(Err(e.to_string())),
            }
        }
    }

    fn eligible(&self) -> bool {
        matches!(&self.gpu, GpuProbe::Ready(Ok(GpuDetection::Device(gpu))) if gpu.is_eligible())
    }

    fn view(&self, job_running: bool) -> VlmPreferencesView {
        let (status, size, installed) = match self.addon.status() {
            AddonStatus::Valid { bytes, .. } => (
                "導入済みです".into(),
                format!("{:.2} GB", *bytes as f64 / 1e9),
                true,
            ),
            AddonStatus::Missing => (
                "未導入です".into(),
                "約 3.4 GB (取得中の空き容量は約 7 GB)".into(),
                false,
            ),
            AddonStatus::Corrupt(message) => (
                format!("導入データを確認できません: {message}"),
                "約 3.4 GB".into(),
                true,
            ),
        };
        let gpu = match &self.gpu {
            GpuProbe::Unchecked | GpuProbe::Checking(_) => "GPU を確認中...".into(),
            GpuProbe::Ready(Err(error)) => format!("GPU の確認に失敗しました: {error}"),
            GpuProbe::Ready(Ok(GpuDetection::VulkanLoaderUnavailable)) => {
                VULKAN_LOADER_REASON.into()
            }
            GpuProbe::Ready(Ok(GpuDetection::Device(gpu))) => format!(
                "{} / VRAM {:.1} GiB / Vulkan {} / {}",
                gpu.device
                    .as_ref()
                    .map(|d| d.name.as_str())
                    .unwrap_or("GPU なし"),
                gpu.vram_total_bytes as f64 / 1073741824.0,
                if gpu.vulkan_available {
                    "対応"
                } else {
                    "非対応"
                },
                if gpu.is_eligible() {
                    "使えます"
                } else {
                    "使えません"
                }
            ),
        };
        // Reinstall replaces the same pack too, so it shares deletion's job fence.
        let busy = self.addon.operation() != AddonOperation::None
            || self.addon.is_scanning()
            || job_running;
        let activity = match self.addon.operation() {
            AddonOperation::Installing => "ダウンロード中です",
            AddonOperation::Cancelling | AddonOperation::Removing => "前の処理を片付けています",
            AddonOperation::None if self.addon.is_scanning() => "モデルを確認中...",
            AddonOperation::None if job_running => "AI 説明文を作成中です",
            AddonOperation::None => "",
        }
        .into();
        VlmPreferencesView {
            status,
            size,
            gpu,
            busy,
            activity,
            can_remove: installed && !busy && !job_running,
        }
    }
}

impl App {
    /// V1-3c calls this with the selected paths captured at the input boundary.
    /// A second request during installation is rejected; it never replaces/adds targets.
    /// Success calls `on_vlm_caption_targets_ready` exactly once with the original paths.
    pub(crate) fn request_vlm_caption_setup(&mut self, ctx: &egui::Context, targets: Vec<PathBuf>) {
        self.request_vlm_caption_setup_targets(ctx, targets.into());
    }

    pub(crate) fn request_vlm_caption_setup_targets(
        &mut self,
        ctx: &egui::Context,
        targets: CaptionTargets,
    ) {
        // Both creation and settings requests enter here, including queued requests.
        if self.caption_delete.is_some() {
            self.show_feedback_result(
                "AI 説明文の削除を処理しています。終わってから選んでください".into(),
                crate::feedback::FeedbackKind::Info,
            );
            return;
        }
        if targets.paths.is_empty() && self.vlm_caption_job_running() {
            return;
        }
        if self.vlm_setup.addon.operation() != AddonOperation::None {
            let removing = self.vlm_setup.addon.operation() == AddonOperation::Removing;
            self.show_feedback_result(
                if removing {
                    "前の処理を片付けています。終わってから選び直してください"
                } else {
                    "取得が終わってから選び直してください"
                }
                .into(),
                crate::feedback::FeedbackKind::Info,
            );
            if removing {
                return;
            }
            if self.vlm_setup.dialog.is_none() {
                self.vlm_setup.dialog = Some(Dialog::Running(CaptionTargets::default()));
            }
            ctx.move_to_top(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("vlm_model_dialog"),
            ));
            return;
        }
        if matches!(
            self.vlm_setup.dialog,
            Some(
                Dialog::Checking(_)
                    | Dialog::Confirm(_)
                    | Dialog::Running(_)
                    | Dialog::RemoveConfirm
            )
        ) {
            return;
        }
        if self.vlm_setup.eligible()
            && matches!(self.vlm_setup.addon.status(), AddonStatus::Valid { .. })
            && !targets.paths.is_empty()
        {
            self.on_vlm_caption_targets_ready(ctx, targets);
            return;
        }
        self.vlm_setup.ensure_checked(ctx);
        self.vlm_setup.dialog = Some(Dialog::Checking(targets));
    }

    /// Setup retains the original paths and counts, independent of later navigation.
    /// Empty settings-only installs never start a caption job.
    fn on_vlm_caption_targets_ready(&mut self, ctx: &egui::Context, targets: CaptionTargets) {
        if targets.paths.is_empty() {
            return;
        }
        let AddonStatus::Valid {
            model_id,
            model_path,
            mmproj_path,
            ..
        } = self.vlm_setup.addon.status()
        else {
            return;
        };
        let GpuProbe::Ready(Ok(GpuDetection::Device(gpu))) = &self.vlm_setup.gpu else {
            return;
        };
        let Some(device) = gpu.device.as_ref().filter(|_| gpu.is_eligible()) else {
            return;
        };
        let config = crate::ai::caption_job_worker::CaptionWorkerConfig {
            worker_executable: None,
            book_cover_env: self.book_cover_env(),
            model_id: model_id.clone(),
            load: crate::ai::vlm_worker::LoadRequest {
                model_path: model_path.clone(),
                mmproj_path: mmproj_path.clone(),
                device_luid: device.luid.clone(),
                cpu_threads: 4,
            },
            caption_db_path: crate::ai_caption_db::CaptionDb::db_path(),
            rotation_db_path: crate::rotation_db::RotationDb::db_path(),
            raw: crate::raw::RawDecodeContext::new(
                std::sync::Arc::clone(&self.raw_develop_executor),
                self.settings.raw_brightness,
            ),
        };
        self.start_caption_job(ctx, targets, config);
    }

    pub(crate) fn vlm_caption_job_running(&self) -> bool {
        self.caption_job.is_some()
    }

    pub(crate) fn refresh_vlm_preferences(&mut self, ctx: &egui::Context) {
        let selected = self
            .pref_state
            .as_ref()
            .is_some_and(|s| s.selected == super::preferences::PreferencesPage::AiCaption);
        if selected {
            self.vlm_setup.ensure_checked(ctx);
        }
        let view = self.vlm_setup.view(self.vlm_caption_job_running());
        if let Some(state) = self.pref_state.as_mut() {
            state.vlm_view = view;
        }
    }

    /// Root-frame polling continues even when the settings/download window is closed.
    pub(crate) fn poll_vlm_setup(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.vlm_setup.folder_result {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("フォルダを開く処理が終了しました".into()))
                }
            };
            if let Some(result) = result {
                self.vlm_setup.folder_result = None;
                if let Err(error) = result {
                    crate::logger::log(format!("[VLM] open folder: {error}"));
                    self.show_feedback_result(
                        format!("フォルダを開けませんでした: {error}"),
                        crate::feedback::FeedbackKind::Error,
                    );
                }
            }
        }
        if let GpuProbe::Checking(rx) = &self.vlm_setup.gpu {
            match rx.try_recv() {
                Ok(result) => self.vlm_setup.gpu = GpuProbe::Ready(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.vlm_setup.gpu = GpuProbe::Ready(Err("GPU の確認処理が終了しました".into()))
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        for _ in 0..64 {
            let cancelling = self.vlm_setup.addon.operation() == AddonOperation::Cancelling;
            let Some(event) = self.vlm_setup.addon.poll() else {
                break;
            };
            match event {
                AddonEvent::Installed { .. } => {
                    if let Some(targets) = self.vlm_setup.installed(cancelling) {
                        self.on_vlm_caption_targets_ready(ctx, targets);
                    }
                }
                AddonEvent::Cancelled => self.vlm_setup.dialog = Some(Dialog::Cancelled),
                AddonEvent::Error { message } => {
                    crate::logger::log(format!("[VLM] {message}"));
                    self.show_feedback_result(
                        "AI 説明文モデルの処理に失敗しました".into(),
                        crate::feedback::FeedbackKind::Error,
                    );
                    self.vlm_setup.dialog = Some(Dialog::Error(message));
                }
                AddonEvent::Removed => {
                    if matches!(self.vlm_setup.dialog, Some(Dialog::Running(_))) {
                        self.vlm_setup.dialog = None;
                    }
                }
                AddonEvent::StatusChanged => {}
            }
        }
        if matches!(&self.vlm_setup.dialog, Some(Dialog::Checking(_)))
            && !self.vlm_setup.addon.is_scanning()
            && matches!(self.vlm_setup.gpu, GpuProbe::Ready(_))
        {
            let Some(Dialog::Checking(paths)) = self.vlm_setup.dialog.take() else {
                unreachable!()
            };
            if let GpuProbe::Ready(Err(error)) = &self.vlm_setup.gpu {
                crate::logger::log(format!("[VLM] GPU probe: {error}"));
                self.vlm_setup.dialog = Some(Dialog::GpuError(error.clone()));
            } else if matches!(
                self.vlm_setup.gpu,
                GpuProbe::Ready(Ok(GpuDetection::VulkanLoaderUnavailable))
            ) {
                self.vlm_setup.dialog = Some(Dialog::Unsupported(VULKAN_LOADER_REASON.into()));
            } else if self.vlm_setup.eligible() {
                if matches!(self.vlm_setup.addon.status(), AddonStatus::Valid { .. })
                    && !paths.paths.is_empty()
                {
                    self.on_vlm_caption_targets_ready(ctx, paths);
                } else {
                    self.vlm_setup.dialog = Some(Dialog::Confirm(paths));
                }
            } else {
                self.vlm_setup.dialog = Some(Dialog::Unsupported(GPU_REQUIREMENTS_REASON.into()));
            }
        }
        if self.vlm_setup.addon.operation() != AddonOperation::None
            || self.vlm_setup.addon.is_scanning()
            || matches!(self.vlm_setup.gpu, GpuProbe::Checking(_))
            || self.vlm_setup.folder_result.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    pub(crate) fn process_vlm_preferences_requests(&mut self, ctx: &egui::Context) {
        let Some(state) = self.pref_state.as_mut() else {
            return;
        };
        let download = std::mem::take(&mut state.vlm_download_requested);
        let remove = std::mem::take(&mut state.vlm_remove_requested);
        let folder = std::mem::take(&mut state.vlm_folder_requested);
        if download && !self.vlm_caption_job_running() {
            // Same immediate management action as the existing addon pages.
            self.pref_state = None;
            self.show_preferences = false;
            self.request_vlm_caption_setup(ctx, Vec::new());
        }
        if remove && !self.vlm_caption_job_running() && self.vlm_setup.view(false).can_remove {
            self.vlm_setup.dialog = Some(Dialog::RemoveConfirm);
        }
        if folder && self.vlm_setup.folder_result.is_none() {
            // Shell/process launch and directory creation are also off the UI thread.
            let dir = self.vlm_setup.addon.root().to_owned();
            let (tx, rx) = mpsc::channel();
            let repaint = ctx.clone();
            match std::thread::Builder::new()
                .name("vlm-open-folder".into())
                .spawn(move || {
                    let result = std::fs::create_dir_all(&dir).and_then(|_| {
                        std::process::Command::new("explorer.exe")
                            .arg(&dir)
                            .spawn()
                            .map(|_| ())
                    });
                    let _ = tx.send(result.map_err(|error| error.to_string()));
                    repaint.request_repaint();
                }) {
                Ok(_) => self.vlm_setup.folder_result = Some(rx),
                Err(error) => {
                    crate::logger::log(format!("[VLM] open folder: {error}"));
                    self.show_feedback_result(
                        format!("フォルダを開けませんでした: {error}"),
                        crate::feedback::FeedbackKind::Error,
                    );
                }
            }
        }
    }

    pub(crate) fn show_vlm_addon_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.vlm_setup.dialog.take() else {
            return;
        };
        let warning = matches!(&self.vlm_setup.gpu, GpuProbe::Ready(Ok(GpuDetection::Device(g))) if g.vram_total_bytes < 9 * 1073741824)
            && self.settings.ai_feature_mode == crate::settings::AiFeatureMode::HighQuality;
        let progress = self.vlm_setup.addon.progress();
        let action = draw_dialog(
            ctx,
            &dialog,
            warning,
            progress,
            self.vlm_setup.addon.operation(),
        );
        match action {
            Action::Download if self.vlm_caption_job_running() => return,
            Action::Download => match self.vlm_setup.addon.start_install() {
                Ok(()) => {
                    let paths = match dialog {
                        Dialog::Confirm(paths) => paths,
                        _ => CaptionTargets::default(),
                    };
                    dialog = Dialog::Running(paths);
                }
                Err(e) => dialog = Dialog::Error(e),
            },
            Action::Remove => {
                if !self.vlm_caption_job_running() {
                    if let Err(e) = self.vlm_setup.addon.start_remove() {
                        self.vlm_setup.dialog = Some(Dialog::Error(e));
                    }
                }
                return;
            }
            Action::Cancel => {
                self.vlm_setup.addon.cancel();
                // Discard paths now, retain owner until worker completion.
                dialog = Dialog::Running(CaptionTargets::default());
            }
            Action::Close => {
                if matches!(dialog, Dialog::Running(_)) {
                    self.vlm_setup.addon.cancel();
                }
                return;
            }
            Action::Retry => {
                dialog = Dialog::Checking(CaptionTargets::default());
            }
            Action::None => {}
        }
        self.vlm_setup.dialog = Some(dialog);
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Action {
    #[default]
    None,
    Download,
    Remove,
    Cancel,
    Close,
    Retry,
}

fn draw_dialog(
    ctx: &egui::Context,
    dialog: &Dialog,
    warning: bool,
    progress: &InstallProgress,
    operation: AddonOperation,
) -> Action {
    let mut action = Action::None;
    let mut open = true;
    let content = ctx.content_rect();
    egui::Window::new("AI 説明文のモデル")
        .id(egui::Id::unique("vlm_model_dialog"))
        .order(egui::Order::Foreground)
        .default_pos(content.min + egui::vec2(40.0, 30.0))
        .default_width(560.0_f32.min(content.width() - 100.0))
        .default_height(360.0_f32.min(content.height() - 100.0))
        .max_height((content.height() - 80.0).max(100.0))
        .collapsible(false).resizable(true).open(&mut open)
        .show(ctx, |ui| {
            let footer = ui.spacing().interact_size.y + ui.spacing().item_spacing.y + 12.0;
            egui::ScrollArea::vertical().auto_shrink([false, true])
                .max_height((ui.available_height() - footer).max(20.0))
                .show(ui, |ui| {
                    let scroll = &mut ui.style_mut().spacing.scroll;
                    scroll.floating_allocated_width = scroll.bar_inner_margin + scroll.bar_width;
                    match dialog {
                        Dialog::Checking(_) => { ui.label("モデルと GPU を確認中..."); }
                        Dialog::Unsupported(reason) => { ui.label(reason); }
                        Dialog::Confirm(_) => {
                            ui.label("画像の内容と画像内の文字を、PC 内の AI が文章と検索語にします。");
                            ui.label("画像は PC の外へ出ません。説明文や書き写した文字は正確とは限りません。");
                            ui.separator();
                            ui.label("取得するもの: Qwen3.5-4B のモデル (約 3.4 GB)");
                            ui.label("取得元: mImageViewer の GitHub Releases");
                            ui.label("ライセンス: Apache License 2.0 (LICENSE と NOTICE を同時に取得)");
                            ui.label("取得中は約 7 GB の空き容量が必要です。中断しても次回は続きから取得できます。");
                            if warning { ui.colored_label(ui.visuals().warn_fg_color, "AI 補正と同時に使うと遅くなることがあります。快適に使うには AI 補正を『軽量』にして、mIV を再起動してください。"); }
                        }
                        Dialog::Running(_) => {
                            if operation == AddonOperation::Cancelling || operation == AddonOperation::Removing {
                                ui.label("前の処理を片付けています");
                            } else {
                                ui.label("モデルをダウンロードしています。取得中も画像を閲覧できます。");
                                match progress {
                                    InstallProgress::Downloading { bytes_done, bytes_total } => {
                                        let ratio = if *bytes_total > 0 { *bytes_done as f32 / *bytes_total as f32 } else { 0.0 };
                                        ui.add(egui::ProgressBar::new(ratio.clamp(0.0, 1.0)).text(format!("{:.2} / {:.2} GB", *bytes_done as f64 / 1e9, *bytes_total as f64 / 1e9)));
                                    }
                                    InstallProgress::Idle | InstallProgress::FetchingIndex => { ui.label("配布一覧を取得中..."); }
                                    InstallProgress::Verifying => { ui.label("ファイルを検証中..."); }
                                    InstallProgress::Concatenating => { ui.label("モデルのファイルをまとめています..."); }
                                    InstallProgress::Installing => { ui.label("モデルを配置中..."); }
                                }
                            }
                        }
                        Dialog::Done => { ui.label("モデルのダウンロードが完了しました。"); }
                        Dialog::Cancelled => { ui.label("ダウンロードをキャンセルしました。次回は続きから取得できます。"); }
                        Dialog::Error(message) => { ui.colored_label(ui.visuals().error_fg_color, "モデルの処理に失敗しました"); ui.label(message); }
                        Dialog::GpuError(message) => { ui.colored_label(ui.visuals().error_fg_color, "GPU の確認に失敗しました"); ui.label(message); ui.label("mIV を再起動してからもう一度お試しください。"); }
                        Dialog::RemoveConfirm => { ui.label("AI 説明文のモデルを削除しますか？"); ui.label("保存済みの説明文は消えません。必要になったら再ダウンロードできます。"); }
                    }
                });
            ui.separator();
            ui.horizontal_wrapped(|ui| match dialog {
                Dialog::Confirm(_) => {
                    if ui.button("ダウンロード").clicked() { action = Action::Download; }
                    if ui.button("今はしない").clicked() { action = Action::Close; }
                }
                Dialog::Running(_) => {
                    if ui.add_enabled(operation == AddonOperation::Installing, egui::Button::new("キャンセル")).clicked() { action = Action::Cancel; }
                }
                Dialog::RemoveConfirm => {
                    if ui.button("削除する").clicked() { action = Action::Remove; }
                    if ui.button("今はしない").clicked() { action = Action::Close; }
                }
                Dialog::Error(_) => {
                    if ui.button("再試行").clicked() { action = Action::Retry; }
                    if ui.button("閉じる").clicked() { action = Action::Close; }
                }
                _ => { if ui.button("閉じる").clicked() { action = Action::Close; } }
            });
        });
    if !open {
        action = Action::Close;
    }
    action
}

pub(super) fn draw_preferences(ui: &mut egui::Ui, view: &VlmPreferencesView) -> (bool, bool, bool) {
    ui.label(egui::RichText::new("AI 説明文").strong());
    ui.label("画像の内容と画像内の文字を、PC 内の AI が文章と検索語にします。");
    ui.add_space(8.0);
    ui.label(&view.status);
    ui.label(format!("モデルの大きさ: {}", view.size));
    ui.label("ライセンス: Apache License 2.0 (LICENSE と NOTICE を保存先に同梱)");
    ui.label(&view.gpu);
    if !view.activity.is_empty() {
        ui.label(&view.activity);
    }
    ui.add_space(8.0);
    let mut result = (false, false, false);
    ui.horizontal_wrapped(|ui| {
        result.0 = ui
            .add_enabled(!view.busy, egui::Button::new("ダウンロード"))
            .clicked();
        result.1 = ui
            .add_enabled(view.can_remove, egui::Button::new("削除"))
            .clicked();
        result.2 = ui.button("フォルダを開く").clicked();
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::{NodeT, Queryable};

    #[test]
    fn vlm_setup_blocks_caption_deletion_for_visible_windows_and_hidden_preparation() {
        let temp = tempfile::tempdir().unwrap();
        let mut setup = VlmSetup::for_caption_menu_test(temp.path().join("vlm"));
        assert!(!setup.blocks_caption_deletion());
        for dialog in [
            Dialog::Checking(vec![].into()),
            Dialog::Confirm(vec![].into()),
            Dialog::Running(vec![].into()),
            Dialog::Unsupported("unsupported".into()),
            Dialog::Done,
            Dialog::Cancelled,
            Dialog::Error("failure".into()),
            Dialog::GpuError("failure".into()),
            Dialog::RemoveConfirm,
        ] {
            setup.dialog = Some(dialog);
            assert!(setup.blocks_caption_deletion());
        }
        setup.dialog = None;
        for operation in [
            AddonOperation::Installing,
            AddonOperation::Cancelling,
            AddonOperation::Removing,
        ] {
            setup.addon = VlmAddon::with_test_operation(temp.path().join("vlm"), operation);
            assert!(setup.blocks_caption_deletion(), "{operation:?}");
        }
        setup.addon = VlmAddon::new(temp.path().join("vlm"));
        setup.addon.request_scan();
        assert!(setup.blocks_caption_deletion());
        let until = std::time::Instant::now() + Duration::from_secs(20);
        while setup.addon.is_scanning() {
            setup.addon.poll();
            assert!(std::time::Instant::now() < until);
            std::thread::yield_now();
        }
        assert!(!setup.blocks_caption_deletion());
        let (_tx, rx) = mpsc::channel();
        setup.gpu = GpuProbe::Checking(rx);
        assert!(setup.blocks_caption_deletion());
    }

    #[test]
    fn vlm_setup_download_with_or_without_reserved_targets_blocks_caption_delete_and_confirm() {
        use super::super::caption_delete::CaptionDeleteState;
        const REASON: &str =
            "AI 説明文のモデルを準備中は削除できません。終わってから選んでください";
        for reserved in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            app.items = vec![crate::grid_item::GridItem::Image(
                "C:/caption/image.png".into(),
            )];
            app.selected = Some(0);
            let targets: CaptionTargets = if reserved {
                vec![PathBuf::from("C:/caption/image.png")]
            } else {
                Vec::new()
            }
            .into();
            app.vlm_setup = VlmSetup::for_caption_menu_test(app.tmp.path().join("vlm"));
            app.vlm_setup.addon = VlmAddon::with_test_operation(
                app.tmp.path().join("vlm"),
                AddonOperation::Installing,
            );
            app.vlm_setup.dialog = Some(Dialog::Running(targets.clone()));
            assert_eq!(app.caption_deletion_disabled_reason(false), Some(REASON));
            app.request_delete_ai_captions(&egui::Context::default(), false);
            assert!(app.caption_delete.is_none());
            assert_eq!(app.fs_feedback_toast.as_ref().unwrap().0, REASON);
            assert_eq!(
                app.vlm_setup.captured_caption_targets_for_test(),
                Some(&targets)
            );

            // Exercise final admission even if preparation appeared after confirmation.
            app.caption_delete = Some(CaptionDeleteState::Confirm {
                keys: vec!["key".into()],
                ok: 1,
                failed: 0,
            });
            let mut harness = crate::test_ui::builder()
                .with_context_setup(crate::ui_fonts::configure_fonts)
                .with_size(egui::vec2(1093.0, 614.0))
                .build_ui_state(
                    |ui, app: &mut crate::app::AppTestEnvForTest| {
                        app.show_caption_delete_dialog(ui.ctx())
                    },
                    app,
                );
            harness.run_steps(4);
            harness.get_by_label("削除").click();
            harness.run_steps(2);
            assert!(harness.state().caption_delete.is_none());
            assert_eq!(
                harness.state().fs_feedback_toast.as_ref().unwrap().0,
                REASON
            );
            assert!(harness.state().caption_job.is_none());
            assert_eq!(
                harness
                    .state()
                    .vlm_setup
                    .captured_caption_targets_for_test(),
                Some(&targets)
            );
            assert_eq!(
                harness.state_mut().vlm_setup.installed(false),
                Some(targets)
            );
        }
    }

    #[test]
    fn vlm_setup_requests_from_settings_and_creation_are_rejected_during_caption_deletion() {
        use super::super::caption_delete::CaptionDeleteState;
        for phase in 0..3 {
            let mut app = crate::app::setup_app_for_test();
            app.items = vec![crate::grid_item::GridItem::Image(
                "C:/caption/image.png".into(),
            )];
            app.selected = Some(0);
            app.vlm_setup = VlmSetup::for_caption_menu_test(app.tmp.path().join("vlm"));
            app.caption_delete = Some(match phase {
                0 => CaptionDeleteState::Counting {
                    rx: mpsc::channel().1,
                },
                1 => CaptionDeleteState::Confirm {
                    keys: vec!["key".into()],
                    ok: 1,
                    failed: 0,
                },
                _ => CaptionDeleteState::Deleting {
                    rx: mpsc::channel().1,
                },
            });
            let ctx = egui::Context::default();
            // Both settings-only and directly submitted creation requests share this entry.
            for targets in [Vec::new(), vec![PathBuf::from("C:/caption/image.png")]] {
                app.request_vlm_caption_setup(&ctx, targets);
                assert!(app.vlm_setup.dialog.is_none());
                assert!(!app.vlm_setup.addon.is_scanning());
                assert_eq!(app.vlm_setup.addon.operation(), AddonOperation::None);
                assert_eq!(
                    app.fs_feedback_toast.as_ref().unwrap().0,
                    "AI 説明文の削除を処理しています。終わってから選んでください"
                );
            }
            app.request_create_ai_captions(&ctx, false);
            assert!(app.vlm_setup.dialog.is_none());
            assert!(app.caption_job.is_none());
        }
    }

    #[test]
    fn vlm_setup_model_remove_is_disabled_for_running_and_stopping_job_owner() {
        use super::super::caption_job::CaptionJob;
        use crate::ai::caption_job_worker::CaptionWorker;
        use std::sync::{Arc, atomic::AtomicBool};
        let temp = tempfile::tempdir().unwrap();
        let mut app = crate::app::setup_app_for_test();
        app.vlm_setup.addon = VlmAddon::with_test_status(
            temp.path().join("vlm"),
            AddonStatus::Valid {
                model_id: "synthetic-model".into(),
                bytes: 10,
                model_path: "synthetic-model.gguf".into(),
                mmproj_path: "synthetic-mmproj.gguf".into(),
            },
        );
        assert!(app.vlm_setup.view(app.vlm_caption_job_running()).can_remove);
        let cancel = Arc::new(AtomicBool::new(false));
        let (worker, _commands, _events) = CaptionWorker::test_channels(cancel.clone());
        app.caption_job = Some(CaptionJob::new(worker, cancel.clone()));
        assert!(!app.vlm_setup.view(app.vlm_caption_job_running()).can_remove);
        assert!(app.vlm_setup.view(app.vlm_caption_job_running()).busy);
        app.request_vlm_caption_setup(&egui::Context::default(), Vec::new());
        assert!(app.vlm_setup.dialog.is_none());
        app.caption_job.as_mut().unwrap().stop();
        assert!(!app.vlm_setup.view(app.vlm_caption_job_running()).can_remove);
        assert!(app.vlm_setup.view(app.vlm_caption_job_running()).busy);
        app.caption_job = None;
        assert!(app.vlm_setup.view(app.vlm_caption_job_running()).can_remove);
    }

    #[test]
    fn vlm_setup_targets_are_handed_off_once_and_discarded_on_cancel() {
        let mut setup = VlmSetup::default();
        let paths = vec![PathBuf::from("captured-before-navigation.png")];
        setup.dialog = Some(Dialog::Running(paths.clone().into()));
        assert_eq!(setup.installed(false), Some(paths.clone().into()));
        assert_eq!(setup.installed(false), None);
        setup.dialog = Some(Dialog::Running(paths.into()));
        assert_eq!(setup.installed(true), None);
        assert_eq!(setup.dialog, Some(Dialog::Cancelled));
        setup.dialog = None; // Window close already discarded pending targets.
        assert_eq!(setup.installed(false), None);
    }

    #[test]
    fn vlm_setup_retains_book_kind_and_unsupported_count_through_install() {
        use crate::ai::caption_job_worker::CaptionTarget;
        use crate::app::book_cover::BookKind;
        let targets = CaptionTargets {
            paths: vec![CaptionTarget::Book {
                path: "captured-before-navigation.epub".into(),
                kind: BookKind::Pdf,
            }],
            unsupported: 3,
        };
        let mut setup = VlmSetup::default();
        setup.dialog = Some(Dialog::Running(targets.clone()));
        assert_eq!(setup.installed(false), Some(targets));
        assert_eq!(setup.installed(false), None);
    }

    #[test]
    fn vlm_setup_modal_boundary_excludes_download_and_terminal_windows() {
        let mut setup = VlmSetup::default();
        for dialog in [
            Dialog::Confirm(vec![].into()),
            Dialog::Checking(vec![].into()),
            Dialog::Unsupported(GPU_REQUIREMENTS_REASON.into()),
            Dialog::RemoveConfirm,
        ] {
            setup.dialog = Some(dialog);
            assert!(setup.modal_open());
        }
        for dialog in [
            Dialog::Running(vec![].into()),
            Dialog::Done,
            Dialog::Cancelled,
            Dialog::Error("failure".into()),
        ] {
            setup.dialog = Some(dialog);
            assert!(!setup.modal_open());
        }
    }

    #[test]
    fn vlm_setup_loader_absence_is_unsupported_but_worker_failure_is_gpu_error() {
        use crate::ai::vlm_worker::{VlmError, detect_gpu_with};
        for loader_available in [false, true] {
            let result = detect_gpu_with(
                || loader_available,
                || Err(VlmError::Io("injected start failure".into())),
            )
            .map_err(|error| error.to_string());
            let mut app = crate::app::setup_app_for_test();
            app.vlm_setup.gpu = GpuProbe::Ready(result);
            app.vlm_setup.dialog = Some(Dialog::Checking(vec!["original.png".into()].into()));
            app.poll_vlm_setup(&egui::Context::default());
            if loader_available {
                assert!(
                    matches!(app.vlm_setup.dialog, Some(Dialog::GpuError(ref error)) if error.contains("injected start failure"))
                );
            } else {
                assert_eq!(
                    app.vlm_setup.dialog,
                    Some(Dialog::Unsupported(VULKAN_LOADER_REASON.into()))
                );
                assert!(app.vlm_setup.modal_open());
                assert_eq!(app.vlm_setup.view(false).gpu, VULKAN_LOADER_REASON);
            }
        }
    }

    #[test]
    fn vlm_setup_request_during_remove_is_rejected_then_new_request_can_confirm() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = crate::app::setup_app_for_test();
        app.vlm_setup.addon = VlmAddon::new(temp.path().join("vlm"));
        app.vlm_setup.gpu = GpuProbe::Ready(Ok(GpuDetection::Device(
            crate::ai::vlm_worker::GpuDecision {
                device: None,
                vulkan_available: true,
                vram_total_bytes: 8 * 1073741824,
            },
        )));
        app.vlm_setup.addon.start_remove().unwrap();
        let ctx = egui::Context::default();
        app.request_vlm_caption_setup(&ctx, vec!["rejected.png".into()]);
        assert!(app.vlm_setup.dialog.is_none());
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while app.vlm_setup.addon.operation() != AddonOperation::None {
            app.poll_vlm_setup(&ctx);
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        app.request_vlm_caption_setup(&ctx, vec!["accepted.png".into()]);
        app.poll_vlm_setup(&ctx);
        assert_eq!(
            app.vlm_setup.dialog,
            Some(Dialog::Confirm(vec!["accepted.png".into()].into()))
        );
        app.vlm_setup.dialog = Some(Dialog::Cancelled);
        app.request_vlm_caption_setup(&ctx, vec!["after-cancel.png".into()]);
        app.poll_vlm_setup(&ctx);
        assert_eq!(
            app.vlm_setup.dialog,
            Some(Dialog::Confirm(vec!["after-cancel.png".into()].into()))
        );
    }

    fn dialog_case(case: usize) -> (Dialog, AddonOperation, &'static str) {
        match case {
            0 => (
                Dialog::Confirm(vec![].into()),
                AddonOperation::None,
                "ダウンロード",
            ),
            1 => (
                Dialog::Running(vec![].into()),
                AddonOperation::Installing,
                "キャンセル",
            ),
            2 => (
                Dialog::Unsupported(GPU_REQUIREMENTS_REASON.into()),
                AddonOperation::None,
                "閉じる",
            ),
            3 => (Dialog::Done, AddonOperation::None, "閉じる"),
            4 => (
                Dialog::Error("長いエラーメッセージと保存先のパスです。".repeat(60)),
                AddonOperation::None,
                "再試行",
            ),
            5 => (Dialog::Cancelled, AddonOperation::None, "閉じる"),
            7 => (
                Dialog::Unsupported(VULKAN_LOADER_REASON.into()),
                AddonOperation::None,
                "閉じる",
            ),
            _ => (Dialog::RemoveConfirm, AddonOperation::None, "削除する"),
        }
    }

    #[test]
    fn vlm_setup_small_window_buttons_remain_visible_and_clickable() {
        for size in [egui::vec2(1093.0, 614.0), egui::vec2(546.5, 307.0)] {
            for case in 0..8 {
                let (dialog, operation, label) = dialog_case(case);
                let mut harness = crate::test_ui::builder()
                    .with_context_setup(crate::ui_fonts::configure_fonts)
                    .with_size(size)
                    .build_ui_state(
                        move |ui, action| {
                            let result = draw_dialog(
                                ui.ctx(),
                                &dialog,
                                true,
                                &InstallProgress::Downloading {
                                    bytes_done: 500,
                                    bytes_total: 1000,
                                },
                                operation,
                            );
                            if result != Action::None {
                                *action = result;
                            }
                        },
                        Action::None,
                    );
                harness.run_steps(4);
                if case == 7 {
                    harness.get_by_label(VULKAN_LOADER_REASON);
                    assert!(
                        harness
                            .query_by_label("mIV を再起動してからもう一度お試しください。")
                            .is_none()
                    );
                    assert!(
                        harness
                            .query_by_role_and_label(egui::accesskit::Role::Button, "ダウンロード")
                            .is_none()
                    );
                }
                let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                for button in harness.get_all_by_role(egui::accesskit::Role::Button) {
                    assert!(
                        bounds.contains_rect(button.rect()),
                        "case {case}: every button including title close must fit: {:?}",
                        button.rect()
                    );
                }
                let rect = harness
                    .get_by_role_and_label(egui::accesskit::Role::Button, label)
                    .rect();
                assert!(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(rect),
                    "case {case} at {size:?}: {rect:?}"
                );
                harness
                    .get_by_role_and_label(egui::accesskit::Role::Button, label)
                    .click();
                harness.run_steps(2);
                assert!(
                    *harness.state() != Action::None,
                    "case {case}: button must receive click"
                );
            }
        }
    }

    #[test]
    fn vlm_setup_confirm_snapshot() {
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(1093.0, 614.0))
            .build_ui(|ui| {
                crate::os_theme::apply_resolved(ui.ctx(), crate::os_theme::ResolvedTheme::Dark);
                draw_dialog(
                    ui.ctx(),
                    &Dialog::Confirm(vec![].into()),
                    true,
                    &InstallProgress::Idle,
                    AddonOperation::None,
                );
            });
        harness.run();
        harness.snapshot("vlm_setup_confirm_dark");
    }

    #[test]
    fn vlm_setup_preferences_snapshot_and_busy_buttons() {
        let view = VlmPreferencesView {
            status: "導入済みです".into(),
            size: "3.46 GB".into(),
            gpu: "GPU / VRAM 8.0 GiB / Vulkan 対応 / 使えます".into(),
            busy: true,
            can_remove: false,
            activity: "前の処理を片付けています".into(),
        };
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(600.0, 260.0))
            .build_ui(move |ui| {
                crate::os_theme::apply_resolved(ui.ctx(), crate::os_theme::ResolvedTheme::Dark);
                draw_preferences(ui, &view);
            });
        harness.run();
        assert!(
            harness
                .get_by_role_and_label(egui::accesskit::Role::Button, "ダウンロード")
                .accesskit_node()
                .is_disabled()
        );
        assert!(
            harness
                .get_by_role_and_label(egui::accesskit::Role::Button, "削除")
                .accesskit_node()
                .is_disabled()
        );
        harness.snapshot("vlm_setup_preferences_dark");
    }
}
