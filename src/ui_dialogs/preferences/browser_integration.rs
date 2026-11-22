//! Pure renderers: immutable worker snapshot in, one explicit command out.
use super::*;
use crate::browser_integration::{Action, Selection, Status, View};

/// Stage 9 writes this page and section in `htdocs/mimageviewer/manual/`.
pub(crate) const BROWSER_INTEGRATION_MANUAL_PAGE: &str = "browser-integration.html";
pub(crate) const BROWSER_INTEGRATION_MANUAL_ANCHOR: &str = "install-extension";

pub(super) fn page(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let mut view = state.browser_integration_view.clone();
    if view.status != Status::Processing {
        view.selection = Selection::from_settings(&state.settings);
    }
    status(ui, &view);
    anchored(ui, state, "browser-integration/enabled", |ui, state| {
        if let Some(action) = enabled_controls(ui, &view, &state.browser_output_input) {
            record_action(state, action);
        }
    });
    if view.status != Status::Processing {
        view.selection = Selection::from_settings(&state.settings);
    }
    ui.separator();
    anchored(ui, state, "browser-integration/folder", |ui, state| {
        let (action, edited) = folder_controls(ui, &view, &mut state.browser_output_input);
        if edited {
            state.settings.browser_view_output_dir =
                selection(&state.browser_output_input, view.selection.enabled).output_dir;
        }
        if let Some(action) = action {
            record_action(state, action);
        }
    });
    notices(ui, &view);
}
fn status(ui: &mut egui::Ui, view: &View) {
    match &view.status {
        Status::Unavailable(reason) => {
            ui.label(reason);
        }
        Status::Preparing => {
            ui.label("ブラウザ連携を確認中です。");
        }
        Status::Processing => {
            ui.label("ブラウザ連携を処理中です。画面を閉じても処理は続きます。");
        }
        Status::Available => {
            ui.label(if view.registered {
                "Chrome / Edge に登録されています。"
            } else {
                "ブラウザ連携は未登録です。"
            });
        }
    }
}
fn enabled_controls(ui: &mut egui::Ui, view: &View, input: &str) -> Option<Action> {
    let mut action = None;
    ui.add_enabled_ui(matches!(view.status, Status::Available), |ui| {
        let mut enabled = view.selection.enabled;
        if ui.checkbox(&mut enabled, "ブラウザ連携を使う").changed() {
            action = Some(Action::Apply(selection(input, enabled)));
        }
        if ui.button("連携を解除").clicked() {
            action = Some(Action::Remove);
        }
    });
    // Store URLs live only on the manual page, so the product never carries stale
    // publication wording (2026-11-22 coordinator decision).
    if ui.link("拡張の入れ方 (マニュアル)").clicked() {
        let url = crate::ui_helpers::manual_url(
            BROWSER_INTEGRATION_MANUAL_PAGE,
            Some(BROWSER_INTEGRATION_MANUAL_ANCHOR),
        );
        crate::ui_helpers::open_url(&url);
    }
    action
}
fn folder_controls(ui: &mut egui::Ui, view: &View, input: &mut String) -> (Option<Action>, bool) {
    let mut action = None;
    let mut edited = false;
    ui.label("ブラウザ閲覧の保存先");
    ui.add_enabled_ui(matches!(view.status, Status::Available), |ui| {
        let hint = view
            .default_output
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "確認中".into());
        let width = ui.available_width();
        let mut output = crate::ime_focus::show_singleline(ui, input, None, |edit| {
            edit.desired_width(width).hint_text(hint)
        });
        let menu_changed =
            crate::ui_helpers::singleline_text_edit_context_menu(ui, &mut output, input);
        edited = output.response.changed() || menu_changed;
        ui.horizontal_wrapped(|ui| {
            if ui.button("保存先を適用").clicked() {
                action = Some(Action::Apply(selection(input, view.selection.enabled)));
            }
            if ui.button("変更…").clicked() {
                action = Some(Action::ChooseFolder);
            }
            if ui.button("既定に戻す").clicked() {
                action = Some(Action::Apply(Selection {
                    enabled: view.selection.enabled,
                    output_dir: None,
                }));
            }
            if ui.button("フォルダを開く").clicked() {
                action = Some(Action::OpenFolder(
                    selection(input, view.selection.enabled).output_dir,
                ));
            }
        });
    });
    ui.weak("保存先は「保存先を適用」または環境設定の OK で反映します。月のフォルダは保存時に作成します。");
    (action, edited)
}
fn notices(ui: &mut egui::Ui, view: &View) {
    if let Some(message) = &view.message {
        ui.label(message);
    }
    ui.separator();
    ui.weak("登録・解除と保存先の変更は、この画面の操作ごとに反映します。");
    ui.label("連携の登録を変更できるのは、既定のデータフォルダの mIV だけです。");
    ui.label("★固定中はブラウザからの画像を開けません。★固定を解除してから操作してください。");
    ui.label("アンインストールの前に、mIV の環境設定で連携を解除してください。");
    ui.weak(
        "別の管理者アカウントでアンインストールした場合、使っていたアカウントの登録は残ります。",
    );
}
fn selection(input: &str, enabled: bool) -> Selection {
    let input = input.trim();
    Selection {
        enabled,
        output_dir: (!input.is_empty()).then(|| PathBuf::from(input)),
    }
}
fn record_action(state: &mut PreferencesState, action: Action) {
    match &action {
        Action::Apply(selection) => selection.apply(&mut state.settings),
        Action::Remove => {
            let mut selection = state.browser_integration_view.selection.clone();
            selection.enabled = false;
            selection.apply(&mut state.settings);
        }
        _ => {}
    }
    state.browser_integration_action = Some(action);
}
pub(super) fn snapshot(ui: &mut egui::Ui, scenario: &str) {
    let mut view = View::default();
    view.status = match scenario {
        "unavailable" => Status::Unavailable(
            "mIV の起動元を確認できません。mimageviewer.exe から起動してください。".into(),
        ),
        "busy" => Status::Processing,
        _ => Status::Available,
    };
    view.default_output = Some(PathBuf::from(
        r"C:\Users\User\Pictures\mimageviewer\browser",
    ));
    let mut input = if scenario == "long" {
        view.registered = true;
        view.selection.enabled = true;
        format!(r"D:\{}\browser", "長い保存先のフォルダ\\".repeat(12))
    } else {
        String::new()
    };
    ui.heading("ブラウザ連携");
    ui.add_space(8.0);
    status(ui, &view);
    let _ = enabled_controls(ui, &view, &input);
    ui.separator();
    let _ = folder_controls(ui, &view, &mut input);
    notices(ui, &view);
}
