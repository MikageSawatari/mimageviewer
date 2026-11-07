//! App-owned depth model management. Download completion never starts 3D mode.
use crate::addon::{AddonEvent, AddonOperation, AddonStatus, InstallProgress};
use crate::app::{App, DepthEntryRequest};
use crate::depth_addon::DepthAddon;
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

#[derive(Debug, PartialEq)]
enum Dialog {
    Checking(CheckingOrigin),
    Confirm,
    Running,
    RemoveConfirm,
    Done,
    Cancelled,
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckingOrigin {
    Preferences,
    ModeEntry(DepthEntryRequest),
}

#[derive(Default)]
pub(crate) struct DepthSetup {
    addon: DepthAddon,
    dialog: Option<Dialog>,
    folder_result: Option<mpsc::Receiver<Result<(), String>>>,
}

#[derive(Clone, Default)]
pub(crate) struct DepthPreferencesView {
    pub status: String,
    pub size: String,
    pub path: PathBuf,
    pub busy: bool,
    pub activity: String,
    pub can_remove: bool,
}

impl DepthSetup {
    #[cfg(test)]
    pub(crate) fn for_test(root: PathBuf) -> Self {
        Self {
            addon: DepthAddon::new(root),
            ..Default::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn checking_for_test(&self) -> bool {
        matches!(self.dialog, Some(Dialog::Checking(_)))
    }

    #[cfg(test)]
    pub(crate) fn confirming_for_test(&self) -> bool {
        self.dialog == Some(Dialog::Confirm)
    }

    #[cfg(test)]
    pub(crate) fn dialog_open_for_test(&self) -> bool {
        self.dialog.is_some()
    }

    #[cfg(test)]
    pub(crate) fn set_status_for_test(&mut self, status: AddonStatus) {
        self.addon.set_status_for_test(status);
    }

    pub(crate) fn modal_open(&self) -> bool {
        matches!(
            self.dialog,
            Some(Dialog::Confirm | Dialog::RemoveConfirm | Dialog::Checking(_))
        )
    }
    /// Cached API for the mode entry path in stage B; it performs no I/O.
    pub(crate) fn installed_model_path(&self) -> Option<&std::path::Path> {
        if self.addon.operation() != AddonOperation::None {
            return None;
        }
        self.addon.installed_model_path()
    }
    fn view(&self) -> DepthPreferencesView {
        let busy = self.addon.is_scanning() || self.addon.operation() != AddonOperation::None;
        let (status, size, can_remove) = match self.addon.status() {
            AddonStatus::Valid { bytes, .. } => (
                "導入済みです".into(),
                format!("{:.2} GB", *bytes as f64 / 1e9),
                true,
            ),
            AddonStatus::Missing => ("未導入です".into(), "約 0.67 GB".into(), false),
            AddonStatus::Corrupt(error) => (
                format!("導入データを確認できません: {error}"),
                "約 0.67 GB".into(),
                true,
            ),
        };
        let activity = match self.addon.operation() {
            AddonOperation::Installing => "ダウンロード中です",
            AddonOperation::Cancelling | AddonOperation::Removing => "前の処理を片付けています",
            AddonOperation::None if self.addon.is_scanning() => "モデルを確認中...",
            AddonOperation::None => "",
        }
        .into();
        DepthPreferencesView {
            status,
            size,
            path: self.addon.root().into(),
            busy,
            activity,
            can_remove: can_remove && !busy,
        }
    }
}

impl App {
    /// Management and download requests never carry an intent to enter 3D mode.
    pub(crate) fn request_depth_addon_setup(&mut self, ctx: &egui::Context) {
        self.request_depth_addon_check(ctx, CheckingOrigin::Preferences);
    }

    pub(crate) fn request_depth_addon_for_entry(
        &mut self,
        ctx: &egui::Context,
        request: DepthEntryRequest,
    ) {
        self.request_depth_addon_check(ctx, CheckingOrigin::ModeEntry(request));
    }

    fn request_depth_addon_check(&mut self, ctx: &egui::Context, origin: CheckingOrigin) {
        if self.depth_setup.addon.operation() != AddonOperation::None {
            self.depth_setup.dialog = Some(Dialog::Running);
        } else {
            self.depth_setup.addon.request_scan();
            self.depth_setup.dialog = Some(if self.depth_setup.addon.is_scanning() {
                Dialog::Checking(origin)
            } else {
                Dialog::Confirm
            });
        }
        ctx.request_repaint();
    }
    pub(crate) fn refresh_depth_preferences(&mut self) {
        if self
            .pref_state
            .as_ref()
            .is_some_and(|state| state.selected == super::preferences::PreferencesPage::DepthAddon)
        {
            self.depth_setup.addon.request_scan();
        }
        let view = self.depth_setup.view();
        if let Some(state) = self.pref_state.as_mut() {
            state.depth_view = view;
        }
    }
    pub(crate) fn process_depth_preferences_requests(&mut self, ctx: &egui::Context) {
        let Some(state) = self.pref_state.as_mut() else {
            return;
        };
        let download = std::mem::take(&mut state.depth_download_requested);
        let remove = std::mem::take(&mut state.depth_remove_requested);
        let folder = std::mem::take(&mut state.depth_folder_requested);
        if download && !self.depth_setup.view().busy {
            self.pref_state = None;
            self.show_preferences = false;
            self.request_depth_addon_setup(ctx);
        }
        if remove && self.depth_setup.view().can_remove {
            self.depth_setup.dialog = Some(Dialog::RemoveConfirm);
        }
        if folder && self.depth_setup.folder_result.is_none() {
            let dir = self.depth_setup.addon.root().to_owned();
            let (tx, rx) = mpsc::channel();
            let repaint = ctx.clone();
            match std::thread::Builder::new()
                .name("depth-open-folder".into())
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
                Ok(_) => self.depth_setup.folder_result = Some(rx),
                Err(error) => self.depth_setup.dialog = Some(Dialog::Error(error.to_string())),
            }
        }
    }
    pub(crate) fn poll_depth_setup(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.depth_setup.folder_result {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("フォルダを開く処理が終了しました".into()))
                }
            };
            if let Some(result) = result {
                self.depth_setup.folder_result = None;
                if let Err(error) = result {
                    crate::logger::log(format!("[Depth addon] open folder: {error}"));
                    self.depth_setup.dialog = Some(Dialog::Error(error));
                }
            }
        }
        let cancelling = self.depth_setup.addon.operation() == AddonOperation::Cancelling;
        if let Some(event) = self.depth_setup.addon.poll() {
            match event {
                AddonEvent::Installed { .. } => {
                    if cancelling {
                        if self.depth_setup.dialog.is_some() {
                            self.depth_setup.dialog = Some(Dialog::Cancelled);
                        }
                    } else {
                        if self.depth_setup.dialog.is_some() {
                            self.depth_setup.dialog = Some(Dialog::Done);
                        }
                        self.show_feedback_result(
                            depth_setup_completion_message(&self.keymap),
                            crate::feedback::FeedbackKind::Info,
                        );
                    }
                }
                AddonEvent::Cancelled => {
                    if self.depth_setup.dialog.is_some() {
                        self.depth_setup.dialog = Some(Dialog::Cancelled);
                    }
                }
                AddonEvent::Removed => self.depth_setup.dialog = None,
                AddonEvent::Error { message } => {
                    crate::logger::log(format!("[Depth addon] {message}"));
                    self.show_feedback_result(
                        "3D 写真モデルの処理に失敗しました".into(),
                        crate::feedback::FeedbackKind::Error,
                    );
                    self.depth_setup.dialog = Some(Dialog::Error(message));
                }
                AddonEvent::StatusChanged => {}
            }
        }
        if matches!(self.depth_setup.dialog, Some(Dialog::Checking(_)))
            && !self.depth_setup.addon.is_scanning()
        {
            let Some(Dialog::Checking(origin)) = self.depth_setup.dialog.take() else {
                unreachable!()
            };
            if self.depth_setup.installed_model_path().is_some() {
                match origin {
                    CheckingOrigin::ModeEntry(request) => {
                        self.resume_depth_entry_after_check(ctx, request);
                    }
                    CheckingOrigin::Preferences => self.depth_setup.dialog = Some(Dialog::Done),
                }
            } else {
                self.depth_setup.dialog = Some(Dialog::Confirm);
            }
        }
        if self.depth_setup.addon.is_scanning()
            || self.depth_setup.addon.operation() != AddonOperation::None
            || self.depth_setup.folder_result.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
    pub(crate) fn show_depth_addon_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.depth_setup.dialog.take() else {
            return;
        };
        let action = draw_dialog(
            ctx,
            &dialog,
            self.depth_setup.addon.progress(),
            self.depth_setup.addon.operation(),
            self.depth_setup.addon.root(),
            &depth_setup_completion_message(&self.keymap),
        );
        match action {
            Action::Download => match self.depth_setup.addon.start_install() {
                Ok(()) => dialog = Dialog::Running,
                Err(error) => dialog = Dialog::Error(error),
            },
            Action::Remove => match self.depth_setup.addon.start_remove() {
                Ok(()) => dialog = Dialog::Running,
                Err(error) => dialog = Dialog::Error(error),
            },
            Action::Cancel => self.depth_setup.addon.cancel(),
            Action::Close => {
                self.depth_setup.addon.cancel();
                return;
            }
            Action::None => {}
        }
        self.depth_setup.dialog = Some(dialog);
    }
}

#[derive(Debug, Default, PartialEq)]
enum Action {
    #[default]
    None,
    Download,
    Remove,
    Cancel,
    Close,
}

fn depth_setup_completion_message(keymap: &crate::keymap::Keymap) -> String {
    match keymap.first_chord_label(crate::keymap::KeyAction::FsDepthParallax) {
        Some(chord) => format!(
            "モデルの準備ができました。もう一度 {chord} キーか上部の「3D」ボタンで 3D 写真に切り替えてください。"
        ),
        None => {
            "モデルの準備ができました。上部の「3D」ボタンで 3D 写真に切り替えてください。".into()
        }
    }
}

fn draw_dialog(
    ctx: &egui::Context,
    dialog: &Dialog,
    progress: &InstallProgress,
    operation: AddonOperation,
    root: &std::path::Path,
    completion_message: &str,
) -> Action {
    let mut open = true;
    let mut action = Action::None;
    let screen = ctx.content_rect();
    let margin = 16.0;
    egui::Window::new("3D 写真の追加モデル")
        .id(egui::Id::new("depth_model_dialog"))
        .order(egui::Order::Foreground)
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_pos(screen.min + egui::vec2(margin, margin))
        .default_width(520.0_f32.min((screen.width() - 2.0 * margin).max(160.0)))
        .default_height((screen.height() - 2.0 * margin).min(380.0))
        .max_height((screen.height() - 2.0 * margin).max(120.0))
        .show(ctx, |ui| {
            let footer = ui.spacing().interact_size.y + ui.spacing().item_spacing.y + 12.0;
            let body_height = (ui.available_height() - footer).max(20.0);
            ui.style_mut().spacing.scroll =
                super::non_overlapping_dialog_scroll_style(ui.style().spacing.scroll);
            egui::ScrollArea::vertical()
                .max_height(body_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    match dialog {
                        Dialog::Checking(_) => {
                            ui.label("モデルを確認中...");
                        }
                        Dialog::Confirm => {
                            ui.label(
                                "写真やイラストの奥行きを PC 内で計算する追加モデルを取得します。",
                            );
                            ui.label("取得するもの: DA3 Mono-Large (約 0.67 GB)");
                            ui.label(
                                "ライセンス: Apache License 2.0 (LICENSE と NOTICE を同時に取得)",
                            );
                            ui.label("取得元: mImageViewer の GitHub Releases (depth-model-v1)");
                            ui.label("画像は PC の外へ出ません。");
                        }
                        Dialog::Running => {
                            if matches!(
                                operation,
                                AddonOperation::Cancelling | AddonOperation::Removing
                            ) {
                                ui.label("前の処理を片付けています");
                            } else {
                                super::model_addon::draw_progress(ui, progress);
                            }
                        }
                        Dialog::RemoveConfirm => {
                            ui.label("3D 写真の追加モデルを削除しますか？");
                            ui.label("必要になったら再ダウンロードできます。");
                        }
                        Dialog::Done => {
                            ui.label(completion_message);
                        }
                        Dialog::Cancelled => {
                            ui.label("ダウンロードをキャンセルしました。");
                        }
                        Dialog::Error(error) => {
                            ui.colored_label(
                                ui.visuals().error_fg_color,
                                "モデルの処理に失敗しました",
                            );
                            ui.label(error);
                        }
                    }
                    ui.add_space(8.0);
                    ui.label(format!("保存先: {}", root.display()));
                });
            ui.separator();
            ui.horizontal_wrapped(|ui| match dialog {
                Dialog::Confirm => {
                    if ui.button("ダウンロード").clicked() {
                        action = Action::Download;
                    }
                    if ui.button("今はしない").clicked() {
                        action = Action::Close;
                    }
                }
                Dialog::RemoveConfirm => {
                    if ui.button("削除する").clicked() {
                        action = Action::Remove;
                    }
                    if ui.button("今はしない").clicked() {
                        action = Action::Close;
                    }
                }
                Dialog::Running => {
                    if ui
                        .add_enabled(
                            operation == AddonOperation::Installing,
                            egui::Button::new("キャンセル"),
                        )
                        .clicked()
                    {
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

pub(super) fn draw_preferences(
    ui: &mut egui::Ui,
    view: &DepthPreferencesView,
) -> (bool, bool, bool) {
    ui.label(egui::RichText::new("3D 写真の追加モデル").strong());
    ui.label("写真やイラストを立体的に動かすための奥行きを、PC 内で計算します。");
    ui.add_space(8.0);
    ui.label(&view.status);
    ui.label(format!("モデルの大きさ: {}", view.size));
    ui.label("ライセンス: Apache License 2.0 (LICENSE と NOTICE を保存先に同梱)");
    ui.label("取得元: mImageViewer の GitHub Releases (depth-model-v1)");
    ui.label(format!("保存先: {}", view.path.display()));
    if !view.activity.is_empty() {
        ui.label(&view.activity);
    }
    ui.add_space(8.0);
    super::model_addon::draw_actions(ui, view.busy, view.can_remove)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::{NodeT, Queryable};

    #[test]
    fn depth_setup_completion_uses_effective_shortcut_and_handles_unassigned() {
        let rebound = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsDepthParallax = F13\n");
        let message = depth_setup_completion_message(&rebound);
        assert!(message.contains("F13 キー"));
        assert!(!message.contains(" D キー"));
        let unassigned = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsDepthParallax = none\n");
        let message = depth_setup_completion_message(&unassigned);
        assert!(!message.contains("キー"));
        assert!(message.contains("「3D」ボタン"));
    }

    #[test]
    fn depth_management_modal_boundary_keeps_download_modeless() {
        let mut setup = DepthSetup::default();
        for dialog in [
            Dialog::Confirm,
            Dialog::Checking(CheckingOrigin::Preferences),
            Dialog::RemoveConfirm,
        ] {
            setup.dialog = Some(dialog);
            assert!(setup.modal_open());
        }
        for dialog in [
            Dialog::Running,
            Dialog::Done,
            Dialog::Cancelled,
            Dialog::Error("failure".into()),
        ] {
            setup.dialog = Some(dialog);
            assert!(!setup.modal_open());
        }
    }

    #[test]
    fn depth_setup_scan_is_cached_and_never_starts_3d_or_download() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = crate::app::setup_app_for_test();
        app.depth_setup.addon = DepthAddon::new(temp.path().join("depth"));
        let ctx = egui::Context::default();
        app.request_depth_addon_setup(&ctx);
        assert_eq!(
            app.depth_setup.dialog,
            Some(Dialog::Checking(CheckingOrigin::Preferences))
        );
        assert_eq!(app.depth_setup.addon.operation(), AddonOperation::None);
        assert!(app.depth_setup.installed_model_path().is_none());
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while app.depth_setup.addon.is_scanning() {
            app.poll_depth_setup(&ctx);
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(app.depth_setup.dialog, Some(Dialog::Confirm));
        assert!(!temp.path().join("depth").exists());
        app.request_depth_addon_setup(&ctx);
        assert!(!app.depth_setup.addon.is_scanning());
        assert_eq!(app.depth_setup.addon.operation(), AddonOperation::None);
    }

    #[test]
    fn depth_addon_small_window_buttons_remain_visible_and_clickable() {
        for size in [egui::vec2(1093.0, 614.0), egui::vec2(546.5, 307.0)] {
            for case in 0..7 {
                let (dialog, operation, label) = match case {
                    0 => (Dialog::Confirm, AddonOperation::None, "ダウンロード"),
                    1 => (Dialog::Running, AddonOperation::Installing, "キャンセル"),
                    2 => (Dialog::RemoveConfirm, AddonOperation::None, "削除する"),
                    3 => (
                        Dialog::Checking(CheckingOrigin::Preferences),
                        AddonOperation::None,
                        "閉じる",
                    ),
                    4 => (Dialog::Done, AddonOperation::None, "閉じる"),
                    5 => (Dialog::Cancelled, AddonOperation::None, "閉じる"),
                    _ => (
                        Dialog::Error("長いエラーメッセージと保存先のパスです。".repeat(50)),
                        AddonOperation::None,
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
                                    bytes_done: 500,
                                    bytes_total: 1000,
                                },
                                operation,
                                std::path::Path::new("C:/disposable/addons/depth"),
                                &depth_setup_completion_message(&crate::keymap::Keymap::empty()),
                            );
                            if result != Action::None {
                                *action = result;
                            }
                        },
                        Action::None,
                    );
                harness.run_steps(4);
                let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                for button in harness.get_all_by_role(egui::accesskit::Role::Button) {
                    assert!(
                        bounds.contains_rect(button.rect()),
                        "case {case} at {size:?}: {:?}",
                        button.rect()
                    );
                }
                harness
                    .get_by_role_and_label(egui::accesskit::Role::Button, label)
                    .click();
                harness.run_steps(2);
                assert_ne!(*harness.state(), Action::None, "case {case}");
            }
        }
    }

    #[test]
    fn depth_addon_preferences_share_busy_controls() {
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(700.0, 320.0))
            .build_ui(|ui| {
                draw_preferences(
                    ui,
                    &DepthPreferencesView {
                        status: "導入済みです".into(),
                        size: "約 0.67 GB".into(),
                        path: "C:/disposable/addons/depth".into(),
                        busy: true,
                        activity: "ダウンロード中です".into(),
                        can_remove: false,
                    },
                );
            });
        harness.run();
        for label in ["ダウンロード", "削除"] {
            assert!(
                harness
                    .get_by_role_and_label(egui::accesskit::Role::Button, label)
                    .accesskit_node()
                    .is_disabled()
            );
        }
        assert!(
            !harness
                .get_by_role_and_label(egui::accesskit::Role::Button, "フォルダを開く")
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("保存先: C:/disposable/addons/depth");
    }

    #[test]
    fn depth_addon_confirm_snapshot() {
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(1093.0, 614.0))
            .build_ui(|ui| {
                crate::os_theme::apply_resolved(ui.ctx(), crate::os_theme::ResolvedTheme::Dark);
                draw_dialog(
                    ui.ctx(),
                    &Dialog::Confirm,
                    &InstallProgress::Idle,
                    AddonOperation::None,
                    std::path::Path::new("C:/disposable/addons/depth"),
                    &depth_setup_completion_message(&crate::keymap::Keymap::empty()),
                );
            });
        harness.run();
        harness.snapshot("depth_addon_confirm_dark");
    }

    #[test]
    fn depth_addon_preferences_snapshot() {
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(600.0, 300.0))
            .build_ui(|ui| {
                crate::os_theme::apply_resolved(ui.ctx(), crate::os_theme::ResolvedTheme::Dark);
                draw_preferences(
                    ui,
                    &DepthPreferencesView {
                        status: "導入済みです".into(),
                        size: "約 0.67 GB".into(),
                        path: "C:/disposable/addons/depth".into(),
                        busy: true,
                        activity: "ダウンロード中です".into(),
                        can_remove: false,
                    },
                );
            });
        harness.run();
        harness.snapshot("depth_addon_preferences_dark");
    }
}
