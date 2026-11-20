//! Settings and consumed input contracts for §1.823–§1.826. No native window is launched.
use super::handler_tests::{app, frame, full_rect, seed, tap, wheel_event};
use super::*;
use crate::keymap::{
    KeyContext, KeyTrigger, Keymap, KeymapSettings, PageTurnPressKind, PressMultiplicity,
};
use crate::settings::{FullscreenNScrollOrder, FullscreenWheelAction, Settings};
use crate::settings_transfer::{export_preferences, parse_preferences};
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

#[cfg(windows)]
struct ClearKeyboard;

#[cfg(windows)]
impl Drop for ClearKeyboard {
    fn drop(&mut self) {
        crate::key_input::clear_test_frame();
    }
}

fn transfer_document(preferences: Value) -> String {
    json!({
        "format": "mimageviewer.preferences",
        "format_version": 1,
        "preferences": preferences,
    })
    .to_string()
}

#[test]
fn scroll_extension_settings_missing_fields_keep_released_defaults() {
    for settings in [Settings::default(), serde_json::from_str("{}").unwrap()] {
        assert_eq!(settings.fullscreen_n_scroll_step_percent, 85);
        assert!(!settings.fullscreen_smooth_page_scroll);
        assert_eq!(
            settings.fullscreen_wheel_action,
            FullscreenWheelAction::PageTurn
        );
        assert_eq!(settings.continuous_reading_wheel_scroll_percent, 20);
        assert_eq!(settings.continuous_reading_key_scroll_percent, 16);
    }
    for (wire, expected) in [
        ("PageTurn", FullscreenWheelAction::PageTurn),
        ("InPageScroll", FullscreenWheelAction::InPageScroll),
        (
            "ScrollThenPageTurn",
            FullscreenWheelAction::ScrollThenPageTurn,
        ),
        ("Zoom", FullscreenWheelAction::Zoom),
    ] {
        let mut settings: Settings = serde_json::from_value(json!({
            "fullscreen_wheel_action": wire,
        }))
        .unwrap();
        settings.sanitize();
        assert_eq!(settings.fullscreen_wheel_action, expected);
        assert_eq!(
            serde_json::to_value(settings).unwrap()["fullscreen_wheel_action"],
            wire
        );
    }
    let mut future: Settings = serde_json::from_value(json!({
        "fullscreen_wheel_action": "FutureWheelAction",
    }))
    .unwrap();
    assert_eq!(
        future.fullscreen_wheel_action,
        FullscreenWheelAction::Unknown
    );
    future.sanitize();
    assert_eq!(
        future.fullscreen_wheel_action,
        FullscreenWheelAction::PageTurn
    );
}

#[test]
fn scroll_extension_settings_roundtrip_and_clamp_only_the_ordered_step() {
    for (input, expected) in [
        (0, 10),
        (9, 10),
        (10, 10),
        (50, 50),
        (85, 85),
        (100, 100),
        (101, 100),
        (u32::MAX, 100),
    ] {
        for smooth in [false, true] {
            for &wheel in FullscreenWheelAction::all() {
                let mut settings = Settings {
                    fullscreen_n_scroll_step_percent: input,
                    fullscreen_smooth_page_scroll: smooth,
                    fullscreen_wheel_action: wheel,
                    ..Settings::default()
                };
                settings.sanitize();
                assert_eq!(settings.fullscreen_n_scroll_step_percent, expected);
                let loaded: Settings =
                    serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
                assert_eq!(loaded.fullscreen_n_scroll_step_percent, expected);
                assert_eq!(loaded.fullscreen_smooth_page_scroll, smooth);
                assert_eq!(loaded.fullscreen_wheel_action, wheel);
                assert_eq!(loaded.continuous_reading_wheel_scroll_percent, 20);
                assert_eq!(loaded.continuous_reading_key_scroll_percent, 16);
            }
        }
    }
}

#[test]
fn scroll_extension_transfer_roundtrips_each_choice_and_preserves_missing_fields() {
    for step in [10, 50, 85, 100] {
        for smooth in [false, true] {
            for &wheel in FullscreenWheelAction::all() {
                let source = Settings {
                    fullscreen_n_scroll_step_percent: step,
                    fullscreen_smooth_page_scroll: smooth,
                    fullscreen_wheel_action: wheel,
                    ..Settings::default()
                };
                let exported = export_preferences(&source).unwrap();
                assert!(exported.issues.is_empty());
                let mut destination = Settings::default();
                let report = parse_preferences(&exported.json)
                    .unwrap()
                    .apply_to(&mut destination);
                assert!(report.issues.is_empty());
                let missing = transfer_document(json!({}));
                for document in [&exported.json, &missing] {
                    parse_preferences(document)
                        .unwrap()
                        .apply_to(&mut destination);
                    assert_eq!(destination.fullscreen_n_scroll_step_percent, step);
                    assert_eq!(destination.fullscreen_smooth_page_scroll, smooth);
                    assert_eq!(destination.fullscreen_wheel_action, wheel);
                }
            }
        }
    }
}

#[test]
fn scroll_extension_transfer_rejects_invalid_types_ranges_and_unknown_values() {
    let mut destination = Settings {
        fullscreen_n_scroll_step_percent: 50,
        fullscreen_smooth_page_scroll: true,
        fullscreen_wheel_action: FullscreenWheelAction::Zoom,
        ..Settings::default()
    };
    for (field, invalid) in [
        ("fullscreen_n_scroll_step_percent", json!(9)),
        ("fullscreen_n_scroll_step_percent", json!(101)),
        ("fullscreen_n_scroll_step_percent", json!(-1)),
        ("fullscreen_n_scroll_step_percent", json!(50.5)),
        ("fullscreen_n_scroll_step_percent", json!("85")),
        ("fullscreen_n_scroll_step_percent", Value::Null),
        ("fullscreen_smooth_page_scroll", json!(1)),
        ("fullscreen_smooth_page_scroll", json!("true")),
        ("fullscreen_smooth_page_scroll", Value::Null),
        ("fullscreen_wheel_action", json!("Unknown")),
        ("fullscreen_wheel_action", json!("FutureWheelAction")),
    ] {
        let report = parse_preferences(&transfer_document(json!({field: invalid})))
            .unwrap()
            .apply_to(&mut destination);
        assert_eq!(report.accepted_count, 0, "{field}");
        assert_eq!(report.issues.len(), 1, "{field}");
        assert_eq!(destination.fullscreen_n_scroll_step_percent, 50);
        assert!(destination.fullscreen_smooth_page_scroll);
        assert_eq!(
            destination.fullscreen_wheel_action,
            FullscreenWheelAction::Zoom
        );
    }
}

#[test]
fn scroll_or_turn_actions_are_unassigned_image_only_and_ini_roundtrips() {
    let defaults = Keymap::default_reference_ini();
    assert_eq!(
        defaults,
        include_str!("../../../docs/keymap.ini.default").replace("\r\n", "\n")
    );
    for action in [
        KeyAction::FsPageScrollOrTurnNext,
        KeyAction::FsPageScrollOrTurnPrev,
    ] {
        assert!(KeyAction::all().contains(&action));
        assert_eq!(KeyAction::parse_ini_name(action.ini_name()), Some(action));
        assert_eq!(action.context(), KeyContext::FsImage);
        assert_eq!(action.trigger(), KeyTrigger::Press);
        assert_eq!(
            action.press_multiplicity(),
            PressMultiplicity::SinglePerFrame
        );
        assert!(action.default_chords().iter().next().is_none());
        assert!(defaults.contains(&format!(
            "# {} = none ; {}",
            action.ini_name(),
            action.description()
        )));
        let ini = format!("[FsImage]\n{} = F24\n", action.ini_name());
        let keymap = Keymap::from_ini_str(&ini);
        let saved = serde_json::to_string(&KeymapSettings::from_keymap(&keymap)).unwrap();
        let reloaded = Keymap::from_settings(&serde_json::from_str(&saved).unwrap());
        for keymap in [&keymap, &reloaded] {
            assert!(keymap.warnings().is_empty(), "{:?}", keymap.warnings());
            assert_eq!(keymap.chord_labels(action), vec!["F24"]);
        }
        let disabled = Keymap::from_ini_str(&format!("[FsImage]\n{} = none\n", action.ini_name()));
        assert!(disabled.warnings().is_empty());
        assert!(disabled.chord_labels(action).is_empty());
        assert!(Keymap::default().chord_labels(action).is_empty());
        assert!(
            !Keymap::default()
                .command_display_rows_for_active_scopes(
                    crate::keymap::FS_IMAGE_ACTIVE_SCOPES,
                    false
                )
                .iter()
                .any(|row| row.spec.action == action)
        );
    }
}

fn key_event(pressed: bool, repeat: bool) -> egui::Event {
    egui::Event::Key {
        key: egui::Key::F24,
        physical_key: None,
        pressed,
        repeat,
        modifiers: egui::Modifiers::NONE,
    }
}

fn cache_keyboard_owner(ctx: &egui::Context, modal: bool) {
    let owner = crate::keyboard_input::decide_keyboard_owner(
        crate::keyboard_input::KeyboardOwnershipSnapshot {
            viewport: ctx.viewport_id(),
            viewport_focused: true,
            modal,
            text_input: None,
            focused_ui: None,
            shortcut_scope: Some(crate::keyboard_input::ShortcutScope::new(
                ctx.viewport_id(),
                crate::keyboard_input::ShortcutSurface::Main,
            )),
        },
    );
    crate::keyboard_input::cache_keyboard_owner(ctx, owner);
}

#[test]
fn consumed_scroll_action_reports_egui_press_or_repeat_once_without_hold_reads() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _clear = ClearKeyboard;
    let keymap = Keymap::from_ini_str("[FsImage]\nFsPageScrollOrTurnNext = F24\n");
    for (repeats, expected) in [
        (vec![false], PageTurnPressKind::InitialPress),
        (vec![true], PageTurnPressKind::AutoRepeat),
        (vec![true, true], PageTurnPressKind::AutoRepeat),
        (vec![true, false, true], PageTurnPressKind::InitialPress),
    ] {
        let ctx = egui::Context::default();
        crate::key_input::install_operation_input_plugin(&ctx);
        // egui derives repeat from its held-key set, independently of the RawInput flag.
        let mut held = repeats.first() == Some(&true);
        if held {
            frame(&ctx, vec![key_event(true, false)], |_, _| {});
        }
        let mut events = Vec::new();
        for repeat in repeats {
            if !repeat && held {
                events.push(key_event(false, false));
            }
            events.push(key_event(true, repeat));
            held = true;
        }
        events.push(key_event(false, false));
        frame(&ctx, events, |_, ctx| {
            cache_keyboard_owner(ctx, false);
            let expected_operation =
                crate::key_input::egui_operation_key_id(ctx, ctx.viewport_id(), |edge| {
                    matches!(
                        edge,
                        crate::key_input::KeyOperationEdge::Egui {
                            key: egui::Key::F24,
                            ..
                        }
                    )
                });
            let input = keymap
                .consume_action_press(ctx, KeyAction::FsPageScrollOrTurnNext)
                .unwrap();
            assert_eq!(input.kind, expected);
            assert!(input.operation_id.is_some());
            assert_eq!(input.operation_id, expected_operation);
            assert!(
                !ctx.input(|input| input.key_down(egui::Key::F24)),
                "release does not change the consumed edge kind"
            );
            assert!(
                keymap
                    .consume_action_press(ctx, KeyAction::FsPageScrollOrTurnNext)
                    .is_none()
            );
            assert!(!keymap.consume_action(ctx, KeyAction::FsPageScrollOrTurnNext));
            assert!(!ctx.input(|input| input.events.iter().any(|event| matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::F24,
                    ..
                }
            ))));
        });
    }
}

#[test]
fn consumed_scroll_action_respects_keyboard_ownership_before_consuming() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _clear = ClearKeyboard;
    let ctx = egui::Context::default();
    let keymap = Keymap::from_ini_str("[FsImage]\nFsPageScrollOrTurnNext = F24\n");
    frame(&ctx, vec![key_event(true, false)], |_, ctx| {
        cache_keyboard_owner(ctx, true);
        assert!(
            keymap
                .consume_action_press(ctx, KeyAction::FsPageScrollOrTurnNext)
                .is_none()
        );
        assert!(ctx.input(|input| input.events.iter().any(|event| matches!(
            event,
            egui::Event::Key {
                key: egui::Key::F24,
                pressed: true,
                ..
            }
        ))));
        cache_keyboard_owner(ctx, false);
        assert_eq!(
            keymap
                .consume_action_press(ctx, KeyAction::FsPageScrollOrTurnNext)
                .unwrap()
                .kind,
            PageTurnPressKind::InitialPress
        );
    });
}

#[cfg(windows)]
fn native_f24_edge(pressed: bool, repeat: bool) -> crate::key_input::KeyEdge {
    crate::key_input::KeyEdge {
        operation_id: crate::key_input::KeyOperationId::unassigned(),
        source_hwnd: 1,
        source_viewport: egui::ViewportId::ROOT,
        virtual_key: 0x87,
        scan_code: 0,
        extended: false,
        pressed,
        repeat,
        ctrl: false,
        shift: false,
        alt: false,
    }
}

#[cfg(windows)]
#[test]
fn consumed_scroll_action_reports_native_kind_and_claims_the_egui_twin() {
    let _serial = crate::key_input::lock_test_input();
    let _clear = ClearKeyboard;
    let keymap = Keymap::from_ini_str("[FsImage]\nFsPageScrollOrTurnPrev = F24\n");
    for (repeats, expected) in [
        (vec![false], PageTurnPressKind::InitialPress),
        (vec![true], PageTurnPressKind::AutoRepeat),
        (vec![true, false, true], PageTurnPressKind::InitialPress),
    ] {
        let ctx = egui::Context::default();
        let mut edges: Vec<_> = repeats
            .iter()
            .copied()
            .map(|repeat| native_f24_edge(true, repeat))
            .collect();
        edges.push(native_f24_edge(false, false));
        let mut events: Vec<_> = repeats
            .into_iter()
            .map(|repeat| key_event(true, repeat))
            .collect();
        events.push(key_event(false, false));
        frame(&ctx, events, |_, ctx| {
            cache_keyboard_owner(ctx, false);
            crate::key_input::set_test_frame(edges.clone());
            let native_operations = crate::key_input::native_operation_key_edges(ctx.viewport_id());
            let expected_operation = native_operations
                .iter()
                .find(|edge| edge.pressed && !edge.repeat)
                .or_else(|| native_operations.iter().find(|edge| edge.pressed))
                .map(|edge| edge.operation_id);
            let input = keymap
                .consume_action_press(ctx, KeyAction::FsPageScrollOrTurnPrev)
                .unwrap();
            assert_eq!(input.kind, expected);
            assert!(input.operation_id.is_some());
            assert_eq!(input.operation_id, expected_operation);
            assert!(
                keymap
                    .consume_action_press(ctx, KeyAction::FsPageScrollOrTurnPrev)
                    .is_none()
            );
            assert!(!keymap.consume_action(ctx, KeyAction::FsPageScrollOrTurnPrev));
            assert!(!ctx.input(|input| input.events.iter().any(|event| matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::F24,
                    ..
                }
            ))));
            crate::key_input::clear_test_frame();
        });
    }
}

fn select_combo(harness: &mut egui_kittest::Harness<'_, &mut App>, label: &str, value: &str) {
    harness
        .get_by_role_and_label(egui::accesskit::Role::ComboBox, label)
        .scroll_to_me();
    harness.run_steps(3);
    harness
        .get_by_role_and_label(egui::accesskit::Role::ComboBox, label)
        .click();
    harness.run_steps(3);
    harness
        .get_by_role_and_label(egui::accesskit::Role::Button, value)
        .click();
    harness.run_steps(3);
}

fn preferences_harness(app: &mut App) -> egui_kittest::Harness<'_, &mut App> {
    crate::test_ui::builder()
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .with_size(egui::vec2(1093.0, 614.0))
        .build_ui_state(
            |root_ui, app| app.show_preferences_dialog(&root_ui.ctx().clone()),
            app,
        )
}

#[test]
fn preferences_n_scroll_amount_and_smooth_checkbox_ok_save_then_key_use_the_new_values() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _clear = ClearKeyboard;
    use crate::ui_dialogs::preferences::{PreferencesOpenRequest, PreferencesPage};

    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.fs_zoom = 3.0;
    app.settings.fullscreen_n_scroll_order = FullscreenNScrollOrder::VerticalFirst;
    seed(&ctx, &mut app);
    let initial_pan = app.fs_pan;
    frame(&ctx, tap(egui::Key::K), |_, ctx| {
        assert!(app.handle_fs_key_input(ctx, 0, false).page_nav.is_none());
    });
    let old_step = (app.fs_pan - initial_pan).length();
    assert!(old_step > 0.0);
    app.fs_pan = initial_pan;
    seed(&ctx, &mut app);
    app.open_preferences_request(PreferencesOpenRequest {
        page: PreferencesPage::SpreadMode,
        anchor: Some("spread/n-scroll-step"),
    });
    {
        let mut harness = preferences_harness(&mut app);
        harness.run_steps(5);
        harness.state_mut().pref_state.as_mut().unwrap().highlight = None;
        harness.run_steps(2);
        harness
            .get_by(|node| {
                node.min_numeric_value() == Some(10.0) && node.max_numeric_value() == Some(100.0)
            })
            .focus();
        harness.run_steps(2);
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.event(egui::Event::Text("50".into()));
        harness.run_steps(2);
        // Leave the numeric editor by clicking a regular label before scrolling away.
        harness
            .get_by_label("ページ内を順に読む操作だけに効きます。")
            .click();
        harness.run_steps(2);
        harness
            .get_by_label("ページ内のスクロールを滑らかに動かす")
            .scroll_to_me();
        harness.run_steps(3);
        harness
            .get_by_label("ページ内のスクロールを滑らかに動かす")
            .click();
        harness.run_steps(2);
        let draft = &harness.state().pref_state.as_ref().unwrap().settings;
        assert_eq!(draft.fullscreen_n_scroll_step_percent, 50);
        assert!(draft.fullscreen_smooth_page_scroll);
        assert_eq!(
            harness.state().settings.fullscreen_n_scroll_step_percent,
            85
        );
        assert!(!harness.state().settings.fullscreen_smooth_page_scroll);
        harness.get_by_label("  OK  ").click();
        harness.run_steps(3);
        assert!(!harness.state().show_preferences);
    }
    let persisted = crate::settings_db::SettingsDb::open(app.tmp.path())
        .unwrap()
        .load_into_settings()
        .unwrap();
    assert_eq!(persisted.fullscreen_n_scroll_step_percent, 50);
    assert!(persisted.fullscreen_smooth_page_scroll);
    assert_eq!(persisted.continuous_reading_key_scroll_percent, 16);
    assert_eq!(persisted.continuous_reading_wheel_scroll_percent, 20);
    app.settings = persisted;
    app.fs_pan = initial_pan;
    seed(&ctx, &mut app);
    frame(&ctx, tap(egui::Key::K), |_, ctx| {
        assert!(app.handle_fs_key_input(ctx, 0, false).page_nav.is_none());
    });
    let new_step = (app.fs_pan - initial_pan).length();
    assert!(
        (new_step / old_step - 50.0 / 85.0).abs() < 0.001,
        "the saved percentage controls the logical target immediately"
    );
    assert!(
        app.fs_page_scroll_animation.is_some(),
        "OK enables presentation interpolation for this same key movement"
    );
}

#[test]
fn preferences_wheel_zoom_ok_save_then_handler_zooms_instead_of_turning() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _clear = ClearKeyboard;
    use crate::ui_dialogs::preferences::PreferencesPage;

    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_wheel_action = FullscreenWheelAction::PageTurn;
    app.open_preferences_page(PreferencesPage::SpreadMode);
    {
        let mut harness = preferences_harness(&mut app);
        harness.run_steps(3);
        select_combo(&mut harness, "通常のホイール", "ズーム");
        assert_eq!(
            harness
                .state()
                .pref_state
                .as_ref()
                .unwrap()
                .settings
                .fullscreen_wheel_action,
            FullscreenWheelAction::Zoom
        );
        assert_eq!(
            harness.state().settings.fullscreen_wheel_action,
            FullscreenWheelAction::PageTurn
        );
        harness.get_by_label("  OK  ").click();
        harness.run_steps(3);
        assert!(!harness.state().show_preferences);
    }
    let persisted = crate::settings_db::SettingsDb::open(app.tmp.path())
        .unwrap()
        .load_into_settings()
        .unwrap();
    assert_eq!(
        persisted.fullscreen_wheel_action,
        FullscreenWheelAction::Zoom
    );
    app.settings = persisted;
    seed(&ctx, &mut app);
    let previous_zoom = app.fs_zoom;
    frame(
        &ctx,
        vec![
            egui::Event::PointerMoved(full_rect().center()),
            wheel_event(Vec2::new(0.0, 120.0), egui::Modifiers::NONE),
        ],
        |ui, ctx| {
            let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
            let (navigation, close) =
                app.handle_fs_wheel_and_click(ui, ctx, full_rect(), &state, false, 0, &mut None);
            assert!(navigation.is_none());
            assert!(!close);
            assert!(app.fs_zoom > previous_zoom);
            assert_eq!(app.fullscreen_idx, Some(0));
        },
    );
}

#[test]
fn preferences_scroll_extension_search_results_jump_to_the_actual_setting_anchors() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _clear = ClearKeyboard;
    use crate::ui_dialogs::preferences::PreferencesPage;

    let mut app = crate::app::setup_app_for_test();
    app.open_preferences_page(PreferencesPage::General);
    let mut harness = preferences_harness(&mut app);
    harness.run_steps(3);
    for (query, anchor) in [
        ("ホイール ズーム", "spread/wheel-action"),
        ("スクロール＋ページ送り", "spread/wheel-action"),
        ("滑らか", "spread/smooth-page-scroll"),
        ("N 字 移動量", "spread/n-scroll-step"),
    ] {
        let state = harness.state_mut().pref_state.as_mut().unwrap();
        state.search_query = query.into();
        state.showing_results = true;
        state.highlight = None;
        harness.run_steps(2);
        // Search rows are painted text, so use the production Enter-first-result route.
        harness.event(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        harness.event(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run_steps(2);
        let state = harness.state().pref_state.as_ref().unwrap();
        assert_eq!(state.selected, PreferencesPage::SpreadMode);
        assert_eq!(state.highlight.map(|highlight| highlight.0), Some(anchor));
        assert!(!state.showing_results);
    }
}

#[test]
fn image_help_explains_all_four_wheel_settings_and_assigned_scroll_or_turn_actions() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _clear = ClearKeyboard;
    for (wheel, explanation) in [
        (
            FullscreenWheelAction::PageTurn,
            "前または次の項目へ移動する（連結読みではスクロール）",
        ),
        (
            FullscreenWheelAction::InPageScroll,
            "ページ内をスクロールする（Shift・チルトは横）。Alt・シークバー上はページ送り",
        ),
        (
            FullscreenWheelAction::ScrollThenPageTurn,
            "読み順にスクロールし、端でもう一度回すとページ送り。Alt・シークバー上はページ送り",
        ),
        (
            FullscreenWheelAction::Zoom,
            "カーソル位置を基準にズームする。Shift は操作なし、Alt・シークバー上はページ送り",
        ),
    ] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_wheel_action = wheel;
        app.keymap = Keymap::from_ini_str(
            "[FsImage]\nFsPageScrollOrTurnNext = F23\nFsPageScrollOrTurnPrev = F24\n",
        );
        let rows = app
            .keymap
            .command_display_rows_for_active_scopes(crate::keymap::FS_IMAGE_ACTIVE_SCOPES, false);
        for action in [
            KeyAction::FsPageScrollOrTurnNext,
            KeyAction::FsPageScrollOrTurnPrev,
        ] {
            assert!(
                rows.iter()
                    .any(|row| row.spec.action == action && !row.shortcut_labels.is_empty())
            );
        }
        app.show_context_shortcuts_help = true;
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(1093.0, 614.0))
            .build_ui_state(
                |root_ui, app| app.show_context_shortcuts_dialog(&root_ui.ctx().clone()),
                &mut *app,
            );
        harness.run_steps(3);
        harness.get_by_label(explanation).scroll_to_me();
        harness.run_steps(2);
        assert!(harness.get_by_label(explanation).rect().is_positive());
    }
}
