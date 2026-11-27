//! The preferences draft owns consent; the App loader owns committed results.
use super::*;
use crate::operation_script::{OperationScriptState, ScriptSlot};

const NOTICE: &str = "スクリプトは mIV の画面の状態を読み、許可された mIV の操作 (ページ送り・表示の切り替え・レーティングやピン留めタグの付け外し・キャプチャ保存など) を自動で実行します。本を開いたときやページが変わったときに、表示の切り替えを自動で行うこともできます。ファイルの削除・移動、ほかのプログラムの起動、インターネットへの接続はできません。\n人から受け取ったスクリプトは、何をするか確かめてから使ってください。";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum OperationScriptView {
    #[default]
    Disabled,
    Loading,
    Ready {
        slots: Vec<ScriptSlot>,
        has_on_event: bool,
    },
    Failed(String),
}

impl OperationScriptView {
    pub(super) fn from_loader(state: &OperationScriptState) -> Self {
        match state {
            OperationScriptState::Disabled => Self::Disabled,
            OperationScriptState::Loading(_) => Self::Loading,
            OperationScriptState::Ready(script) => Self::Ready {
                slots: script.slots.clone(),
                has_on_event: script.has_on_event,
            },
            OperationScriptState::Failed(error) => Self::Failed(error.clone()),
        }
    }
}

#[derive(Default)]
pub(crate) struct Actions {
    pub open: bool,
    pub reload: bool,
    pub reset: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Confirmation {
    Enable,
    Reset,
}

pub(super) fn page(ui: &mut egui::Ui, state: &mut PreferencesState) {
    anchored(ui, state, "extensions/operation-script", |ui, state| {
        let mut enabled = state.settings.operation_script_enabled;
        if ui
            .checkbox(&mut enabled, "操作のスクリプトを使う")
            .changed()
        {
            if enabled && !state.settings.operation_script_notice_confirmed {
                state.operation_script_confirmation = Some(Confirmation::Enable);
            } else {
                state.settings.operation_script_enabled = enabled;
            }
        }
        ui.label(NOTICE);
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            state.operation_script_actions.open |= ui.button("スクリプトを開く").clicked();
            state.operation_script_actions.reload |= ui
                .add_enabled(
                    state.operation_script_committed_enabled,
                    egui::Button::new("再読み込み"),
                )
                .clicked();
            if ui.button("既定に戻す").clicked() {
                state.operation_script_confirmation = Some(Confirmation::Reset);
            }
        });
        ui.label("ファイルを変えたら「再読み込み」を押してください。");
        ui.label("止めるにはチェックを外して「OK」を押してください。ファイルを削除しても、次の読み込みで雛形が作られます。");
        ui.label("スクリプト操作 1〜20 は、操作カスタマイズでキー・リングなどに割り当てます。");
        ui.add_space(8.0);
        ui.separator();
        ui.label(egui::RichText::new("確定済みの設定での読み込み結果").strong());
        match &state.operation_script_view {
            OperationScriptView::Disabled => {
                ui.label("OFF");
            }
            OperationScriptView::Loading => {
                ui.label("読み込み中");
            }
            OperationScriptView::Failed(error) => {
                ui.colored_label(ui.visuals().error_fg_color, "失敗");
                ui.add(egui::Label::new(error).wrap());
            }
            OperationScriptView::Ready {
                slots,
                has_on_event,
            } => {
                ui.label("使える");
                ui.label(if *has_on_event {
                    "本を開いたときなどに自動で動く処理 (on_event): あり"
                } else {
                    "本を開いたときなどに自動で動く処理 (on_event): なし"
                });
                // Vertical rows keep long names/functions readable at the narrowest
                // preferences width; there is no unbounded fixed-width table.
                for slot in slots {
                    ui.group(|ui| {
                        ui.add(
                            egui::Label::new(format!("スロット {}: {}", slot.number, slot.label))
                                .wrap(),
                        );
                        ui.add(egui::Label::new(format!("関数: {}", slot.function)).wrap());
                    });
                }
            }
        }
        ui.add_space(8.0);
        let url = crate::ui_helpers::manual_url("operation-script.html", None);
        ui.hyperlink_to("操作のスクリプトのマニュアル", url);
    });
}

pub(super) fn draw_confirmation(
    ctx: &egui::Context,
    confirmation: Confirmation,
    enter_pressed: bool,
    escape_pressed: bool,
) -> Option<bool> {
    // Consent and replacement require an explicit button action; an Enter
    // captured by the parent before this child opened must not accept either.
    if enter_pressed {
        ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
        });
    }
    let mut accept = false;
    let mut cancel = false;
    let response =
        egui::Modal::new(egui::Id::unique("operation_script_confirmation")).show(ctx, |ui| {
            ui.set_width(420.0_f32.min((ctx.content_rect().width() - 48.0).max(0.0)));
            let heading = ui.heading(match confirmation {
                Confirmation::Enable => "操作のスクリプトを使いますか？",
                Confirmation::Reset => "スクリプトを既定に戻しますか？",
            });
            ui.add_space(8.0);
            // Keep title and action row visible even at 200% on a small screen.
            let frame_margin = egui::Frame::popup(ui.style()).total_margin().sum();
            let fixed_height = heading.rect.height()
                + 16.0
                + ui.spacing().interact_size.y
                + 4.0 * ui.spacing().item_spacing.y
                + frame_margin.y;
            let body_height = (ctx.content_rect().height() - 48.0 - fixed_height).max(24.0);
            let mut scroll = pref_panel_scroll_style();
            scroll.floating = false;
            ui.style_mut().spacing.scroll = scroll;
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), body_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("operation_script_notice_body")
                        .max_height(body_height)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(match confirmation {
                                    Confirmation::Enable => NOTICE,
                                    Confirmation::Reset => {
                                        "今のスクリプトは既定の内容で置き換えられ、元に戻せません"
                                    }
                                })
                                .wrap(),
                            );
                        });
                },
            );
            ui.add_space(8.0);
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                cancel = ui.button("やめる").clicked();
                accept = ui
                    .button(match confirmation {
                        Confirmation::Enable => "使う",
                        Confirmation::Reset => "既定の内容で置き換える",
                    })
                    .clicked();
            });
        });
    if accept {
        Some(true)
    }
    // Use the App's IME-aware Escape, rather than ModalResponse::should_close's
    // raw Escape. Like the folder-background confirmation, clicking outside
    // means cancel, including a click at a disabled parent control's position.
    // Enter has no default destructive/consent action.
    else if cancel || escape_pressed || response.backdrop_response.clicked() {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::{NodeT, Queryable};

    const ENABLE: &str = "操作のスクリプトを使う";

    // Release blocked jobs and join before AppTestEnv drops its temporary
    // directory and settings/data-dir isolation, including assertion panics.
    struct ScriptAppFixture {
        app: crate::app::AppTestEnvForTest,
        release: Option<mpsc::Sender<()>>,
    }

    impl std::ops::Deref for ScriptAppFixture {
        type Target = crate::app::AppTestEnvForTest;
        fn deref(&self) -> &Self::Target {
            &self.app
        }
    }
    impl std::ops::DerefMut for ScriptAppFixture {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.app
        }
    }
    impl Drop for ScriptAppFixture {
        fn drop(&mut self) {
            drop(self.release.take());
            self.app.operation_script.shutdown_file_worker_for_test();
        }
    }

    fn file_harness(
        gated: bool,
        service: bool,
        editor: impl Fn(&Path) -> Result<(), String> + Send + 'static,
    ) -> egui_kittest::Harness<'static, ScriptAppFixture> {
        let mut app = crate::app::setup_app_for_test();
        let path = app.tmp.path().join("operation_scripts.rhai");
        let (release, gate) = mpsc::channel::<()>();
        *app.operation_script = crate::operation_script::OperationScriptLoader::with_file_worker(
            path,
            editor,
            move || {
                if gated {
                    let _ = gate.recv();
                }
            },
        );
        app.open_preferences_page(PreferencesPage::OperationScript);
        let mut last_service_frame = None;
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(1093.0, 614.0))
            .build_ui_state(
                move |ui, fixture: &mut ScriptAppFixture| {
                    let ctx = ui.ctx().clone();
                    // Match the root-frame service boundary even when egui requests
                    // multiple layout passes after the OK click in this frame.
                    let frame = ctx.cumulative_frame_nr();
                    if service && last_service_frame != Some(frame) {
                        last_service_frame = Some(frame);
                        fixture.app.service_operation_script_files(&ctx);
                    }
                    fixture.app.show_preferences_dialog(&ctx);
                },
                ScriptAppFixture {
                    app,
                    release: Some(release),
                },
            );
        harness.run_steps(3);
        harness
    }

    fn state() -> PreferencesState {
        let mut state = super::super::tests::preferences_state_for_test(&Settings::default());
        state.selected = PreferencesPage::OperationScript;
        state
    }

    fn ready() -> OperationScriptView {
        OperationScriptView::Ready {
            slots: (1..=3)
                .map(|number| ScriptSlot {
                    number,
                    label: format!("表示の切り替え {number}"),
                    function: format!("display_{number}"),
                })
                .collect(),
            has_on_event: true,
        }
    }

    fn page_harness(
        state: PreferencesState,
        width: f32,
    ) -> egui_kittest::Harness<'static, PreferencesState> {
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(width, 850.0))
            .build_ui_state(
                |root, state| {
                    egui::CentralPanel::default().show(root, |ui| {
                        ui.heading("操作のスクリプト");
                        page(ui, state);
                    });
                },
                state,
            );
        harness.run_steps(3);
        harness
    }

    #[test]
    fn operation_script_preferences_index_and_committed_results() {
        let entry = search_index::PREF_SEARCH_INDEX
            .iter()
            .find(|entry| entry.anchor == "extensions/operation-script")
            .unwrap();
        assert_eq!(entry.page, PreferencesPage::OperationScript);
        assert_eq!(entry.title, ENABLE);
        assert_eq!(preference_category(entry.page).0, "拡張と診断");
        assert_eq!(entry.page.label(), "操作のスクリプト");
        for (view, labels) in [
            (OperationScriptView::Disabled, vec!["OFF"]),
            (OperationScriptView::Loading, vec!["読み込み中"]),
            (
                OperationScriptView::Failed("関数がありません".into()),
                vec!["失敗", "関数がありません"],
            ),
            (
                ready(),
                vec![
                    "使える",
                    "本を開いたときなどに自動で動く処理 (on_event): あり",
                    "スロット 1: 表示の切り替え 1",
                    "関数: display_3",
                ],
            ),
        ] {
            let mut state = state();
            state.operation_script_view = view;
            let harness = page_harness(state, 430.0);
            for label in labels {
                assert!(harness.get_by_label(label).rect().is_positive());
            }
            assert!(harness.get_by_label(NOTICE).rect().is_positive());
            assert!(
                harness
                    .get_by_label("操作のスクリプトのマニュアル")
                    .rect()
                    .is_positive()
            );
        }
    }

    #[test]
    fn operation_script_preferences_reload_uses_committed_setting_only() {
        for committed in [false, true] {
            let mut state = state();
            state.settings.operation_script_enabled = !committed;
            state.operation_script_committed_enabled = committed;
            let mut harness = page_harness(state, 430.0);
            assert_eq!(
                harness
                    .get_by_label("再読み込み")
                    .accesskit_node()
                    .is_disabled(),
                !committed
            );
            harness.get_by_label("再読み込み").click();
            harness.run_steps(2);
            assert_eq!(harness.state().operation_script_actions.reload, committed);
        }
    }

    #[test]
    fn operation_script_preferences_consent_draft_cancel_and_second_enable() {
        let mut harness =
            super::super::tests::preferences_view_harness(PreferencesPage::OperationScript);
        harness.get_by_label(ENABLE).click();
        harness.run_steps(3);
        assert_eq!(
            harness
                .state()
                .pref_state
                .as_ref()
                .unwrap()
                .operation_script_confirmation,
            Some(Confirmation::Enable)
        );
        harness.get_by_label("やめる").click();
        harness.run_steps(3);
        let draft = &harness.state().pref_state.as_ref().unwrap().settings;
        assert!(!draft.operation_script_enabled && !draft.operation_script_notice_confirmed);
        harness.get_by_label(ENABLE).click();
        harness.run_steps(3);
        harness.get_by_label("使う").click();
        harness.key_press(egui::Key::Enter);
        harness.run_steps(3);
        let draft = &harness.state().pref_state.as_ref().unwrap().settings;
        assert!(draft.operation_script_enabled && draft.operation_script_notice_confirmed);
        assert!(!harness.state().settings.operation_script_enabled);
        // Repeated OFF -> ON within this draft does not ask again.
        for _ in 0..2 {
            harness.get_by_label(ENABLE).click();
            harness.run_steps(3);
            assert!(
                harness
                    .state()
                    .pref_state
                    .as_ref()
                    .unwrap()
                    .operation_script_confirmation
                    .is_none()
            );
        }
        harness.get_by_label("キャンセル").click();
        harness.run_steps(3);
        harness.get_by_label("破棄して閉じる").click();
        harness.run_steps(3);
        assert!(!harness.state().settings.operation_script_enabled);
        assert!(!harness.state().settings.operation_script_notice_confirmed);
        harness
            .state_mut()
            .open_preferences_page(PreferencesPage::OperationScript);
        harness.run_steps(3);
        harness.get_by_label(ENABLE).click();
        harness.run_steps(3);
        assert_eq!(
            harness
                .state()
                .pref_state
                .as_ref()
                .unwrap()
                .operation_script_confirmation,
            Some(Confirmation::Enable)
        );
    }

    #[test]
    fn operation_script_preferences_modals_own_open_close_frames_and_parent_controls() {
        for (trigger, confirmation) in [
            (ENABLE, Confirmation::Enable),
            ("既定に戻す", Confirmation::Reset),
        ] {
            for opening_key in [egui::Key::Enter, egui::Key::Escape] {
                let mut harness =
                    super::super::tests::preferences_view_harness(PreferencesPage::OperationScript);
                // Establish the hovered widget and press in earlier frames, as
                // the clipboard-save same-frame input test does. The release
                // which activates the trigger must share RawInput with the key.
                harness.get_by_label(trigger).hover();
                harness.run_steps(1);
                let pos = harness.get_by_label(trigger).rect().center();
                harness.input_mut().events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                });
                harness.run_steps(1);
                assert!(
                    harness
                        .state()
                        .pref_state
                        .as_ref()
                        .unwrap()
                        .operation_script_confirmation
                        .is_none(),
                    "{confirmation:?}: child must open on release"
                );
                let opening_frame = harness.ctx.cumulative_frame_nr();
                // Harness::step processes each queued .click()/key_press()
                // event in a separate frame. Put all initiating input in the
                // same RawInput to exercise the actual opening-frame boundary.
                harness.input_mut().events.extend([
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::Key {
                        key: opening_key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::Key {
                        key: opening_key,
                        physical_key: None,
                        pressed: false,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
                harness.run_steps(1);
                assert_eq!(harness.ctx.cumulative_frame_nr(), opening_frame + 1);
                assert!(
                    harness.state().show_preferences,
                    "{confirmation:?}: opening_key={opening_key:?}"
                );
                assert!(!harness.state().show_preferences_discard_confirm);
                assert_eq!(
                    harness
                        .state()
                        .pref_state
                        .as_ref()
                        .unwrap()
                        .operation_script_confirmation,
                    Some(confirmation),
                    "{confirmation:?}: opening_key={opening_key:?}"
                );
                // The child must still be present after settling subsequent
                // frames, and both draft consent flags must remain unchanged.
                harness.run_steps(2);
                assert_eq!(
                    harness
                        .state()
                        .pref_state
                        .as_ref()
                        .unwrap()
                        .operation_script_confirmation,
                    Some(confirmation),
                    "{confirmation:?}: opening_key={opening_key:?}"
                );
                let draft = &harness.state().pref_state.as_ref().unwrap().settings;
                assert!(!draft.operation_script_enabled);
                assert!(!draft.operation_script_notice_confirmed);
                for label in ["  OK  ", "キャンセル", "Close window"] {
                    assert!(
                        harness.get_by_label(label).accesskit_node().is_disabled(),
                        "{label}"
                    );
                }
                harness.key_press(egui::Key::Enter);
                harness.run_steps(2);
                assert_eq!(
                    harness
                        .state()
                        .pref_state
                        .as_ref()
                        .unwrap()
                        .operation_script_confirmation,
                    Some(confirmation),
                    "{confirmation:?}: opening_key={opening_key:?}"
                );
                harness.key_press(egui::Key::Escape);
                harness.run_steps(3);
                assert!(harness.state().show_preferences);
                assert!(!harness.state().show_preferences_discard_confirm);
                assert!(
                    harness
                        .state()
                        .pref_state
                        .as_ref()
                        .unwrap()
                        .operation_script_confirmation
                        .is_none()
                );
                assert!(!harness.state().settings.operation_script_enabled);
                assert!(!harness.state().settings.operation_script_notice_confirmed);
            }
        }
    }

    #[test]
    fn operation_script_preferences_parent_clicks_cancel_only_child_without_applying_or_resetting()
    {
        use std::sync::atomic::AtomicUsize;

        for (trigger, confirmation) in [
            (ENABLE, Confirmation::Enable),
            ("既定に戻す", Confirmation::Reset),
        ] {
            // Each physical click can dismiss the modal as a backdrop click.
            // Reopen a fresh fixture for each target so all parent controls are
            // actually tested while the child is still open.
            for target in ["  OK  ", "キャンセル", "Close window", "backdrop"] {
                let opens = Arc::new(AtomicUsize::new(0));
                let editor_opens = opens.clone();
                let mut harness = file_harness(false, false, move |_| {
                    editor_opens.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                });
                let path = harness.state().tmp.path().join("operation_scripts.rhai");
                std::fs::write(&path, "preserve user script").unwrap();
                let live = harness.state().settings.preferences_snapshot();
                let draft = {
                    let state = harness.state_mut().pref_state.as_mut().unwrap();
                    // If the parent's OK leaks through, this unrelated dirty
                    // setting would be applied even though script enable stays OFF.
                    state.settings.thumb_show_type_icon = !live.thumb_show_type_icon;
                    state.settings.preferences_snapshot()
                };
                let db = crate::settings_db::SettingsDb::open(harness.state().tmp.path()).unwrap();
                let saved = db.load_into_settings().unwrap();
                harness.run_steps(3);
                harness.get_by_label(trigger).click();
                harness.run_steps(3);
                assert_eq!(
                    harness
                        .state()
                        .pref_state
                        .as_ref()
                        .unwrap()
                        .operation_script_confirmation,
                    Some(confirmation),
                    "{confirmation:?}: {target}"
                );
                for label in ["  OK  ", "キャンセル", "Close window"] {
                    assert!(harness.get_by_label(label).accesskit_node().is_disabled());
                }
                if target == "backdrop" {
                    let pos = egui::pos2(5.0, 5.0);
                    harness.event(egui::Event::PointerMoved(pos));
                    for pressed in [true, false] {
                        harness.event(egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        });
                        harness.run_steps(1);
                    }
                } else {
                    harness.get_by_label(target).click();
                }
                harness.run_steps(3);
                assert!(
                    harness.state().show_preferences,
                    "{confirmation:?}: {target}"
                );
                assert!(!harness.state().show_preferences_discard_confirm);
                let state = harness.state().pref_state.as_ref().unwrap();
                assert!(state.operation_script_confirmation.is_none());
                assert!(settings_equal_for_close_prompt(&state.settings, &draft));
                assert!(!state.settings.operation_script_enabled);
                assert!(!state.settings.operation_script_notice_confirmed);
                assert!(settings_equal_for_close_prompt(
                    &harness.state().settings,
                    &live
                ));
                assert!(settings_equal_for_close_prompt(
                    &db.load_into_settings().unwrap(),
                    &saved
                ));
                // Catch an erroneous Reset enqueue as well as a leaked setting
                // change, and finish any work before the fixture is cleaned up.
                harness
                    .state_mut()
                    .operation_script
                    .shutdown_file_worker_for_test();
                assert_eq!(opens.load(Ordering::SeqCst), 0);
                assert_eq!(
                    std::fs::read_to_string(&path).unwrap(),
                    "preserve user script"
                );
            }
        }
    }

    #[test]
    fn operation_script_preferences_file_buttons_immediate_and_reset_requires_confirmation() {
        use std::sync::atomic::AtomicUsize;
        let opens = Arc::new(AtomicUsize::new(0));
        let editor_opens = opens.clone();
        let mut harness = file_harness(false, false, move |_| {
            editor_opens.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        let path = harness.state().tmp.path().join("operation_scripts.rhai");
        std::fs::write(&path, "user script").unwrap();
        harness.get_by_label("既定に戻す").click();
        harness.run_steps(3);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "user script");
        harness.get_by_label("やめる").click();
        harness.run_steps(3);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "user script");
        harness.get_by_label("スクリプトを開く").click();
        harness.run_steps(3);
        harness.get_by_label("既定に戻す").click();
        harness.run_steps(3);
        harness.get_by_label("既定の内容で置き換える").click();
        harness.key_press(egui::Key::Enter);
        harness.run_steps(3);
        harness
            .state_mut()
            .operation_script
            .shutdown_file_worker_for_test();
        assert_eq!(opens.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            crate::operation_script::DEFAULT_SCRIPT
        );
        assert!(matches!(
            harness.state().operation_script.state,
            OperationScriptState::Disabled
        ));
        assert!(!harness.state().settings.operation_script_enabled);
        assert!(harness.state().show_preferences);
        // File operations survive closing the uncommitted preferences draft.
        harness.get_by_label("キャンセル").click();
        harness.run_steps(3);
        assert!(!harness.state().show_preferences);
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            crate::operation_script::DEFAULT_SCRIPT
        );
    }

    #[test]
    fn operation_script_preferences_ok_next_frame_loading_ready_and_reload() {
        let mut harness = file_harness(true, true, |_| Ok(()));
        harness.get_by_label(ENABLE).click();
        harness.run_steps(3);
        harness.get_by_label("使う").click();
        harness.run_steps(3);
        assert!(!harness.state().settings.operation_script_enabled);
        harness.get_by_label("  OK  ").click();
        harness.run_steps(1);
        assert!(harness.state().settings.operation_script_enabled);
        assert!(harness.state().settings.operation_script_notice_confirmed);
        assert!(matches!(
            harness.state().operation_script.state,
            OperationScriptState::Disabled
        ));
        harness.run_steps(1);
        assert!(matches!(
            harness.state().operation_script.state,
            OperationScriptState::Loading(_)
        ));
        let db = crate::settings_db::SettingsDb::open(harness.state().tmp.path()).unwrap();
        let saved = db.load_into_settings().unwrap();
        assert!(saved.operation_script_enabled && saved.operation_script_notice_confirmed);
        drop(harness.state_mut().release.take());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !matches!(
            harness.state().operation_script.state,
            OperationScriptState::Ready(_)
        ) {
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not become Ready"
            );
            harness.run_steps(1);
            std::thread::yield_now();
        }
        harness
            .state_mut()
            .open_preferences_page(PreferencesPage::OperationScript);
        harness.run_steps(3);
        assert!(matches!(
            harness.state().operation_script.state,
            OperationScriptState::Ready(_)
        ));
        assert!(harness.get_by_label("使える").rect().is_positive());
        assert!(
            !harness
                .get_by_label("再読み込み")
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("再読み込み").click();
        harness.run_steps(1);
        assert!(matches!(
            harness.state().operation_script.state,
            OperationScriptState::Loading(_)
        ));
        harness
            .state_mut()
            .operation_script
            .shutdown_file_worker_for_test();
    }

    #[test]
    fn operation_script_preferences_narrow_long_result_wraps() {
        let mut state = state();
        state.operation_script_view = OperationScriptView::Ready {
            slots: vec![ScriptSlot {
                number: 20,
                label: "とても長い名前".repeat(30),
                function: "very_long_function_name_".repeat(10),
            }],
            has_on_event: false,
        };
        let harness = page_harness(state, 350.0);
        for label in [
            format!("スロット 20: {}", "とても長い名前".repeat(30)),
            format!("関数: {}", "very_long_function_name_".repeat(10)),
        ] {
            let rect = harness.get_by_label(&label).rect();
            assert!(rect.right() <= 350.0, "{rect:?}");
        }
        assert!(
            harness
                .get_by_label("本を開いたときなどに自動で動く処理 (on_event): なし")
                .rect()
                .is_positive()
        );
        let mut state = self::state();
        let error = "読み込みに失敗しました。".repeat(35);
        state.operation_script_view = OperationScriptView::Failed(error.clone());
        let harness = page_harness(state, 350.0);
        assert!(harness.get_by_label(&error).rect().right() <= 350.0);
    }

    #[test]
    fn operation_script_preferences_narrow_window_reaches_slot_twenty_and_manual() {
        use crate::test_ui::scroll_layout::ScrollLayout;
        let mut harness =
            super::super::tests::preferences_view_harness(PreferencesPage::OperationScript);
        let mut script = crate::operation_script::CompiledOperationScript::compile(
            crate::operation_script::DEFAULT_SCRIPT,
        )
        .unwrap();
        script.slots = (1..=20)
            .map(|number| ScriptSlot {
                number,
                label: format!("読む向きと表示を切り替える操作 {number}"),
                function: format!("change_reading_direction_and_display_{number}"),
            })
            .collect();
        harness.state_mut().operation_script.state = OperationScriptState::Ready(Box::new(script));
        harness.run_steps(3);
        let window_id = egui::Id::unique(egui::Atoms::new("環境設定").text());
        let before = harness
            .ctx
            .memory(|memory| memory.area_rect(window_id))
            .unwrap();
        let start = before.right_center();
        let end = start - egui::vec2(before.width(), 0.0);
        harness.hover_at(start);
        harness.run_steps(2);
        harness.drag_at(start);
        harness.run_steps(2);
        harness.hover_at(end);
        harness.run_steps(3);
        harness.drop_at(end);
        harness.run_steps(4);
        let narrow = harness
            .ctx
            .memory(|memory| memory.area_rect(window_id))
            .unwrap();
        assert!((narrow.width() - PREFERENCES_MIN_WIDTH).abs() <= 1.0);
        for label in [
            "スロット 20: 読む向きと表示を切り替える操作 20",
            "操作のスクリプトのマニュアル",
        ] {
            harness.get_by_label(label).scroll_to_me();
            harness.run_steps(4);
            let (_, layout) = harness
                .ctx
                .data(|data| {
                    data.get_temp::<(PreferencesPage, ScrollLayout)>(egui::Id::unique(
                        "preferences_right_panel_layout",
                    ))
                })
                .unwrap();
            assert!(layout.offset.y > 0.0);
            assert!(
                layout
                    .inner
                    .contains_rect(harness.get_by_label(label).rect()),
                "{label}: {layout:?}"
            );
            assert!(layout.content.x <= layout.inner.width() + 1.0);
        }
        for label in ["  OK  ", "キャンセル"] {
            assert!(
                harness
                    .ctx
                    .content_rect()
                    .contains_rect(harness.get_by_label(label).rect())
            );
        }
    }

    #[test]
    fn operation_script_preferences_modals_small_screen_actions_stay_visible() {
        for size in [egui::vec2(1093.0, 614.0), egui::vec2(546.5, 307.0)] {
            for confirmation in [Confirmation::Enable, Confirmation::Reset] {
                let mut harness = crate::test_ui::builder()
                    .with_context_setup(crate::ui_fonts::configure_fonts)
                    .with_size(size)
                    .build_ui(move |root_ui| {
                        let ctx = root_ui.ctx().clone();
                        let _ = draw_confirmation(&ctx, confirmation, false, false);
                    });
                harness.run_steps(4);
                for label in [
                    "やめる",
                    if confirmation == Confirmation::Enable {
                        "使う"
                    } else {
                        "既定の内容で置き換える"
                    },
                ] {
                    let rect = harness.get_by_label(label).rect();
                    assert!(
                        egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(rect),
                        "{confirmation:?}: {rect:?}"
                    );
                }
            }
        }
    }

    fn snapshot(dark: bool) {
        let mut state = state();
        state.settings.operation_script_enabled = true;
        state.settings.operation_script_notice_confirmed = true;
        state.operation_script_committed_enabled = true;
        state.operation_script_view = ready();
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(560.0, 690.0))
            .build_ui_state(
                move |root, state| {
                    crate::os_theme::apply_resolved(
                        root.ctx(),
                        if dark {
                            crate::os_theme::ResolvedTheme::Dark
                        } else {
                            crate::os_theme::ResolvedTheme::Light
                        },
                    );
                    egui::CentralPanel::default().show(root, |ui| {
                        ui.heading("操作のスクリプト");
                        page(ui, state);
                    });
                },
                state,
            );
        harness.run_steps(3);
        harness.snapshot(if dark {
            "operation_script_preferences_dark"
        } else {
            "operation_script_preferences_light"
        });
    }

    #[test]
    fn operation_script_preferences_snapshot_light() {
        snapshot(false);
    }
    #[test]
    fn operation_script_preferences_snapshot_dark() {
        snapshot(true);
    }
}
