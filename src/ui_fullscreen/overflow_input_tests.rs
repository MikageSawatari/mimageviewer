//! Overflow ownership through the production fullscreen input/navigation handlers.
use super::*;
use crate::ring_shortcut::{RightDragContext, RightDragMode};

const STATES: [FsOverflowPanelState; 5] = [
    FsOverflowPanelState::Root,
    FsOverflowPanelState::NavigatorPosition,
    FsOverflowPanelState::SpreadAdjustment,
    FsOverflowPanelState::Slideshow,
    FsOverflowPanelState::MusicNoteColor,
];

fn viewport() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 800.0))
}

fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
    app.frame_counter += 1;
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(viewport()),
            time: Some(app.frame_counter as f64 / 60.0),
            focused: true,
            events,
            ..Default::default()
        },
        |ui| {
            let Some(idx) = app.fullscreen_idx else {
                return;
            };
            let ctx = ui.ctx().clone();
            let state = app.prepare_fullscreen_state(&ctx, idx, FsPageTurnDecision::normal());
            let (page_nav, close) =
                app.handle_fs_wheel_and_click(ui, &ctx, viewport(), &state, false, 0, &mut None);
            app.handle_fs_navigation_with_operation(
                &ctx, close, false, None, None, None, page_nav, None, idx, None,
            );
        },
    )
    .drop_without_applying_deltas();
}

fn ready(ctx: &egui::Context) -> crate::app::AppTestEnvForTest {
    crate::ui_fonts::configure_fonts(ctx);
    page_turn::install(ctx);
    install_fs_navigator_input_tracking(ctx);
    let mut app = page_turn_submission_tests::ready_app(ctx, SpreadMode::Single);
    app.fullscreen_idx = Some(1);
    app.settings.touch_still_chrome_learned = true;
    #[cfg(windows)]
    {
        app.native_video_in_window_active = true;
    }
    for _ in 0..3 {
        frame(&mut app, ctx, vec![]);
    }
    app
}

fn button(pos: egui::Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn overflow_secondary_sequences_are_owned_through_release_in_all_states() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for state in STATES {
        for mode in [
            RightDragMode::Disabled,
            RightDragMode::RingShortcut,
            RightDragMode::MouseGesture,
        ] {
            for leave in [false, true] {
                let ctx = egui::Context::default();
                let mut app = ready(&ctx);
                app.settings
                    .ring_shortcuts
                    .set_right_drag_mode(RightDragContext::ImageFullscreen, mode.clone());
                app.fs_overflow_panel_state = state;
                let inside = fs_overflow_panel_rect(viewport(), state).unwrap().center();
                let end = if leave {
                    egui::pos2(250.0, 400.0)
                } else {
                    inside
                };
                frame(
                    &mut app,
                    &ctx,
                    vec![
                        egui::Event::PointerMoved(inside),
                        button(inside, egui::PointerButton::Secondary, true),
                    ],
                );
                assert!(
                    matches!(
                        app.fs_secondary_press,
                        FullscreenSecondaryPress::Overflow { .. }
                    ),
                    "{state:?} {mode:?}"
                );
                frame(&mut app, &ctx, vec![egui::Event::PointerMoved(end)]);
                assert!(app.mouse_ring_flick.is_none() && app.mouse_gesture.is_none());
                assert_eq!(app.fullscreen_idx, Some(1));
                frame(
                    &mut app,
                    &ctx,
                    vec![button(end, egui::PointerButton::Secondary, false)],
                );
                assert_eq!(
                    app.fullscreen_idx,
                    Some(1),
                    "panel secondary release must not close fullscreen: {state:?} {mode:?}"
                );
                assert_eq!(app.fs_overflow_panel_state, state);
                assert!(matches!(
                    app.fs_secondary_press,
                    FullscreenSecondaryPress::Idle
                ));
                assert!(app.mouse_ring_flick.is_none() && app.mouse_gesture.is_none());
            }
        }
    }
}

#[test]
fn overflow_outside_secondary_start_keeps_existing_recognizers_and_short_click() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for mode in [
        RightDragMode::Disabled,
        RightDragMode::RingShortcut,
        RightDragMode::MouseGesture,
    ] {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx);
        app.settings
            .ring_shortcuts
            .set_right_drag_mode(RightDragContext::ImageFullscreen, mode.clone());
        app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
        let outside = egui::pos2(250.0, 400.0);
        frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(outside),
                button(outside, egui::PointerButton::Secondary, true),
            ],
        );
        match mode {
            RightDragMode::RingShortcut => assert!(app.mouse_ring_flick.is_some()),
            RightDragMode::MouseGesture => assert!(app.mouse_gesture.is_some()),
            _ => assert!(matches!(
                app.fs_secondary_press,
                FullscreenSecondaryPress::Armed { .. }
            )),
        }
        frame(
            &mut app,
            &ctx,
            vec![button(outside, egui::PointerButton::Secondary, false)],
        );
        assert_eq!(
            app.fullscreen_idx, None,
            "outside short right-click keeps its existing close action: {mode:?}"
        );
    }
}

fn touch(pos: egui::Pos2, phase: egui::TouchPhase) -> egui::Event {
    egui::Event::Touch {
        device_id: egui::TouchDeviceId(84),
        id: egui::TouchId(1),
        phase,
        pos,
        force: None,
    }
}

fn tap(app: &mut App, ctx: &egui::Context, pos: egui::Pos2) {
    frame(
        app,
        ctx,
        vec![
            touch(pos, egui::TouchPhase::Start),
            egui::Event::PointerMoved(pos),
            button(pos, egui::PointerButton::Primary, true),
        ],
    );
    frame(
        app,
        ctx,
        vec![
            touch(pos, egui::TouchPhase::End),
            button(pos, egui::PointerButton::Primary, false),
        ],
    );
    frame(app, ctx, vec![egui::Event::PointerGone]);
}

#[test]
fn overflow_outside_tap_dismisses_only_then_next_tap_acts_and_releases_owner() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for state in STATES {
        for (pos, page_target) in [
            (egui::pos2(150.0, 400.0), Some(0)),
            (egui::pos2(850.0, 700.0), Some(2)),
            (viewport().center(), None),
        ] {
            let ctx = egui::Context::default();
            let mut app = ready(&ctx);
            app.fs_overflow_panel_state = state;
            assert!(
                !fs_overflow_panel_rect(viewport(), state)
                    .unwrap()
                    .contains(pos)
            );
            let chrome = app.still_touch_chrome_is_latched(&ctx);
            tap(&mut app, &ctx, pos);
            assert_eq!(
                app.fullscreen_idx,
                Some(1),
                "first tap only dismisses: {state:?}"
            );
            assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
            assert_eq!(app.still_touch_chrome_is_latched(&ctx), chrome);
            assert!(
                !app.fs_primary_suppression.is_active(),
                "terminal touch ownership must retire"
            );
            tap(&mut app, &ctx, pos);
            if let Some(target) = page_target {
                assert_eq!(
                    app.fullscreen_idx,
                    Some(target),
                    "second tap navigates: {state:?}"
                );
            } else {
                assert_ne!(
                    app.still_touch_chrome_is_latched(&ctx),
                    chrome,
                    "second center tap toggles chrome"
                );
            }
        }
    }
}

#[test]
fn overflow_outside_touch_drag_retains_existing_pan_and_panel() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for state in STATES {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx);
        app.fs_zoom = 2.0;
        app.fs_overflow_panel_state = state;
        let start = egui::pos2(350.0, 400.0);
        let end = start + egui::vec2(64.0, 24.0);
        frame(
            &mut app,
            &ctx,
            vec![
                touch(start, egui::TouchPhase::Start),
                egui::Event::PointerMoved(start),
                button(start, egui::PointerButton::Primary, true),
            ],
        );
        frame(
            &mut app,
            &ctx,
            vec![
                touch(end, egui::TouchPhase::Move),
                egui::Event::PointerMoved(end),
            ],
        );
        assert_ne!(app.fs_pan, egui::Vec2::ZERO);
        frame(
            &mut app,
            &ctx,
            vec![
                touch(end, egui::TouchPhase::End),
                button(end, egui::PointerButton::Primary, false),
            ],
        );
        assert_eq!(app.fs_overflow_panel_state, state);
        assert_eq!(app.fullscreen_idx, Some(1));
        assert!(!app.fs_primary_suppression.is_active());
    }
}

#[test]
fn overflow_all_states_prevent_cursor_auto_hide_until_closed() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    ctx.run_pass(egui::RawInput::default(), |ctx| {
        assert!(app.fs_ui_is_clean(ctx, viewport(), false, MusicViewFrameUiState::default()));
        for state in STATES {
            app.fs_overflow_panel_state = state;
            assert!(
                !app.fs_ui_is_clean(ctx, viewport(), false, MusicViewFrameUiState::default()),
                "{state:?}"
            );
        }
        app.fs_overflow_panel_state = FsOverflowPanelState::Closed;
        assert!(app.fs_ui_is_clean(ctx, viewport(), false, MusicViewFrameUiState::default()));
    })
    .drop_without_applying_deltas();
}

#[cfg(windows)]
#[test]
fn overflow_focus_reclaim_touch_uses_the_same_outside_tap_dismissal() {
    let _input = crate::key_input::lock_test_input();
    // Headless focus provider: no real focus request or native window is used.
    let _focus = crate::test_native_focus::scoped(11, 11, true);
    for state in STATES {
        for outside in [
            Some(viewport().center()),
            Some(egui::pos2(150.0, 400.0)),
            None,
        ] {
            let ctx = egui::Context::default();
            let mut app = ready(&ctx);
            app.fs_overflow_panel_state = state;
            app.settings.vst3_enabled = true;
            app.settings.vst3_gui_visible = true;
            let pos = outside
                .unwrap_or_else(|| fs_overflow_panel_rect(viewport(), state).unwrap().center());
            let chrome = app.still_touch_chrome_is_latched(&ctx);
            // Both edges in the activation frame are an existing supported delivery.
            frame(
                &mut app,
                &ctx,
                vec![
                    touch(pos, egui::TouchPhase::Start),
                    egui::Event::PointerMoved(pos),
                    button(pos, egui::PointerButton::Primary, true),
                    touch(pos, egui::TouchPhase::End),
                    button(pos, egui::PointerButton::Primary, false),
                ],
            );
            assert_eq!(
                app.fs_overflow_panel_state,
                if outside.is_some() {
                    FsOverflowPanelState::Closed
                } else {
                    state
                }
            );
            assert_eq!(app.fullscreen_idx, Some(1));
            assert_eq!(app.still_touch_chrome_is_latched(&ctx), chrome);
            frame(&mut app, &ctx, vec![egui::Event::PointerGone]);
            assert!(!app.fs_primary_suppression.is_active());
        }
    }
    assert!(crate::test_native_focus::requests().is_empty());
}

#[test]
fn overflow_secondary_capture_outlives_panel_close_and_retires_on_release() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = ready(&ctx);
    app.settings.ring_shortcuts.set_right_drag_mode(
        RightDragContext::ImageFullscreen,
        RightDragMode::MouseGesture,
    );
    app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
    let pos = fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state)
        .unwrap()
        .center();
    frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(pos),
            button(pos, egui::PointerButton::Secondary, true),
        ],
    );
    app.fs_overflow_panel_state = FsOverflowPanelState::Closed;
    let outside = egui::pos2(250.0, 400.0);
    frame(&mut app, &ctx, vec![egui::Event::PointerMoved(outside)]);
    assert!(app.mouse_gesture.is_none());
    frame(
        &mut app,
        &ctx,
        vec![button(outside, egui::PointerButton::Secondary, false)],
    );
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.fs_secondary_press, FullscreenSecondaryPress::Idle);
    frame(
        &mut app,
        &ctx,
        vec![button(outside, egui::PointerButton::Secondary, true)],
    );
    assert!(
        app.mouse_gesture.is_some(),
        "the next outside stream is not captured"
    );
}

#[test]
fn overflow_outside_right_drag_can_continue_into_panel() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for state in STATES {
        for mode in [RightDragMode::RingShortcut, RightDragMode::MouseGesture] {
            let ctx = egui::Context::default();
            let mut app = ready(&ctx);
            app.settings
                .ring_shortcuts
                .set_right_drag_mode(RightDragContext::ImageFullscreen, mode.clone());
            app.fs_overflow_panel_state = state;
            let outside = egui::pos2(250.0, 400.0);
            let inside = fs_overflow_panel_rect(viewport(), state).unwrap().center();
            frame(
                &mut app,
                &ctx,
                vec![
                    egui::Event::PointerMoved(outside),
                    button(outside, egui::PointerButton::Secondary, true),
                ],
            );
            frame(&mut app, &ctx, vec![egui::Event::PointerMoved(inside)]);
            match mode {
                RightDragMode::RingShortcut => assert!(app.mouse_ring_flick.is_some()),
                RightDragMode::MouseGesture => assert!(app.mouse_gesture.is_some()),
                _ => unreachable!(),
            }
            assert!(!matches!(
                app.fs_secondary_press,
                FullscreenSecondaryPress::Overflow { .. }
            ));
            frame(
                &mut app,
                &ctx,
                vec![button(inside, egui::PointerButton::Secondary, false)],
            );
            assert!(app.mouse_ring_flick.is_none() && app.mouse_gesture.is_none());
        }
    }
}
