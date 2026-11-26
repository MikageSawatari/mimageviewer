use super::*;
use crate::operation_script::{CompiledOperationScript, OperationScriptState};

fn frame(ctx: &egui::Context, events: Vec<egui::Event>, mut run: impl FnMut(&egui::Context)) {
    let _ = ctx
        .run_pass(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 400.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| run(ctx),
        )
        .drop_without_applying_deltas();
}

fn press(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn mouse_slot_press() -> egui::Event {
    egui::Event::PointerButton {
        pos: egui::pos2(100.0, 100.0),
        button: egui::PointerButton::Extra1,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    }
}

fn install_script(app: &mut App, operations: &str) {
    app.settings.operation_script_enabled = true;
    app.settings.ring_shortcuts.mouse_buttons_image.back =
        crate::ring_shortcut::RingActionId::ScriptSlot1;
    let source = format!(
        "fn slots() {{ [#{{slot:1,label:\"test\",run:\"entry\"}}] }}\nfn entry(s) {{ {operations} }}"
    );
    app.operation_script.state =
        OperationScriptState::Ready(Box::new(CompiledOperationScript::compile(&source).unwrap()));
}

#[test]
fn root_arrow_navigation_survives_empty_a_b_and_rejected_mouse_slots() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for (operations, navigator_changes) in [
        ("[]", false),
        ("[\"FsNavigatorToggle\"]", true),
        ("[\"FsNavigatorToggle\",\"FsPagePrev\"]", false),
        ("[\"NoSuchOperation\"]", false),
    ] {
        let ctx = egui::Context::default();
        let mut app = page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
        app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
        app.settings.page_turn_animation_enabled = false;
        install_script(&mut app, operations);
        let before = app.settings.fullscreen_navigator_visible;
        frame(
            &ctx,
            vec![press(egui::Key::ArrowDown), mouse_slot_press()],
            |ctx| assert!(app.handle_fullscreen_root_key_input(ctx)),
        );
        assert_eq!(app.fullscreen_idx, Some(1), "{operations}");
        assert_eq!(
            app.settings.fullscreen_navigator_visible != before,
            navigator_changes,
            "{operations}"
        );
        #[cfg(windows)]
        crate::key_input::clear_test_frame();
    }
}

#[test]
fn root_rebased_input_rejects_mouse_slot_navigation_before_any_script_action() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
    app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
    app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsViewSpread = F17\n");
    install_script(&mut app, "[\"FsZoomIn\",\"FsPageNext\"]");
    let before = app.fs_zoom;
    frame(
        &ctx,
        vec![press(egui::Key::F17), mouse_slot_press()],
        |ctx| assert!(app.handle_fullscreen_root_key_input(ctx)),
    );
    assert_ne!(app.spread_mode, SpreadMode::Single);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(
        app.fs_zoom, before,
        "Rebased must count as accepted navigation"
    );
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
}

#[test]
fn root_mouse_slot_preserves_arrow_operation_on_page_turn_continuation() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
    app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
    install_script(&mut app, "[]");
    page_turn_submission_tests::seed_leaf(&ctx, &mut app);
    frame(
        &ctx,
        vec![press(egui::Key::ArrowLeft), mouse_slot_press()],
        |ctx| {
            let operation = crate::key_input::operation_key_edges(ctx, ctx.viewport_id())
                .into_iter()
                .find(|edge| {
                    matches!(
                        edge,
                        crate::key_input::KeyOperationEdge::Egui {
                            key: egui::Key::ArrowLeft,
                            ..
                        }
                    )
                })
                .unwrap()
                .operation_id();
            assert!(app.handle_fullscreen_root_key_input(ctx));
            assert_eq!(
                app.page_turn_presentation.continuation_operation_for_test(),
                Some((ViewerOperationId::Key(operation), 0))
            );
        },
    );
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
}

#[test]
fn script_merge_preserves_request_landing_and_operation_across_empty_and_rebase() {
    let original = Some(ViewerOperationId::Pad {
        batch: 37,
        ordinal: 2,
    });
    let mut operation = original;
    let mut action = FsKeyAction {
        page_nav: FsPageNavRequest {
            nav: FsPageNav::Target(2),
            landing: Some(FsPageLanding::End),
        },
        ..Default::default()
    };
    action.merge_script_action(FsKeyAction::default(), &mut operation);
    action.merge_script_action(
        FsKeyAction {
            page_transition: FsKeyPageTransition::Rebased { idx: 1 },
            ..Default::default()
        },
        &mut operation,
    );
    assert_eq!(action.page_nav.nav, FsPageNav::Target(2));
    assert_eq!(action.page_nav.landing, Some(FsPageLanding::End));
    assert_eq!(
        action.page_transition,
        FsKeyPageTransition::Rebased { idx: 1 }
    );
    assert_eq!(operation, original);
    action.merge_script_action(
        FsKeyAction {
            page_transition: FsKeyPageTransition::Closed,
            close: true,
            ..Default::default()
        },
        &mut operation,
    );
    assert_eq!(action.page_transition, FsKeyPageTransition::Closed);
    assert!(action.page_nav.is_none());
    assert!(!action.close);
    assert_eq!(operation, None);
}

#[test]
fn script_navigation_never_inherits_an_unrelated_operation() {
    let mut action = FsKeyAction::default();
    let mut operation = Some(ViewerOperationId::Pad {
        batch: 37,
        ordinal: 2,
    });
    action.merge_script_action(
        FsKeyAction {
            page_nav: FsPageNav::Delta(1).into(),
            ..Default::default()
        },
        &mut operation,
    );
    assert_eq!(action.page_nav, FsPageNav::Delta(1));
    assert_eq!(operation, None);
}

#[test]
fn before_render_closed_result_is_terminal_and_rebase_uses_current_index() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
    install_script(&mut app, "[\"FsZoomIn\"]");
    let zoom = app.fs_zoom;
    let mut closed = FsKeyAction {
        page_transition: FsKeyPageTransition::Closed,
        ..Default::default()
    };
    let mut operation = None;
    frame(&ctx, vec![], |ctx| {
        app.run_fullscreen_script_slot_for_action(ctx, 1, &mut closed, &mut operation);
    });
    assert_eq!(app.fs_zoom, zoom);
    app.fullscreen_idx = Some(2);
    app.settings.page_turn_animation_enabled = false;
    app.finish_fullscreen_key_action(
        &ctx,
        0,
        FsKeyAction {
            page_transition: FsKeyPageTransition::Rebased { idx: 2 },
            page_nav: FsPageNav::Delta(1).into(),
            ..Default::default()
        },
    );
    assert_eq!(app.fullscreen_idx, Some(3));
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
}

#[cfg(windows)]
#[test]
fn fullscreen_browser_repeat_is_rejected_when_collecting_slot_but_navigation_still_repeats() {
    let _serial = crate::key_input::lock_test_input();
    for (script, repeat) in [(true, false), (true, true), (false, true)] {
        let ctx = egui::Context::default();
        let mut app = page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
        app.settings.ring_shortcuts.mouse_buttons_image.back = if script {
            crate::ring_shortcut::RingActionId::ScriptSlot1
        } else {
            crate::ring_shortcut::RingActionId::GridHistoryBack
        };
        crate::take_pending_mouse_nav();
        crate::inject_browser_keydown_for_test(false, repeat);
        frame(&ctx, vec![], |ctx| {
            let action = app.handle_fs_key_input(ctx, 0, false);
            assert_eq!(action.script_slot, (script && !repeat).then_some(1));
            assert_eq!(action.mouse_nav.is_some(), !script);
        });
        crate::key_input::clear_test_frame();
    }
}
