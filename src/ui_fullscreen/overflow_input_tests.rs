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
                assert!(app.fs_front_pointer.buttons[1], "{state:?} {mode:?}");
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
                !app.fs_front_pointer.is_active(),
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
        assert!(!app.fs_front_pointer.is_active());
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
            assert!(!app.fs_front_pointer.is_active());
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
            assert!(!app.fs_front_pointer.buttons[1]);
            frame(
                &mut app,
                &ctx,
                vec![button(inside, egui::PointerButton::Secondary, false)],
            );
            assert!(app.mouse_ring_flick.is_none() && app.mouse_gesture.is_none());
        }
    }
}

fn render_frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
    render_frame_with_modifiers(app, ctx, events, egui::Modifiers::NONE);
}

fn render_frame_with_modifiers(
    app: &mut App,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
) {
    render_frame_in_rect(app, ctx, events, modifiers, viewport());
}

fn render_frame_in_rect(
    app: &mut App,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
    rect: egui::Rect,
) {
    render_output_in_rect(app, ctx, events, modifiers, rect).drop_without_applying_deltas();
}

fn render_output_in_rect(
    app: &mut App,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
    rect: egui::Rect,
) -> egui::FullOutput {
    app.frame_counter += 1;
    crate::test_ui::headless_paint_output(
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(rect),
                time: Some(app.frame_counter as f64 / 60.0),
                focused: true,
                events: std::iter::once(egui::Event::ModifiersChanged(modifiers))
                    .chain(events)
                    .collect(),
                ..Default::default()
            },
            |ui| app.render_fullscreen_viewport(ui),
        ),
    )
}

fn rendered_navigator(ctx: &egui::Context) -> crate::app::AppTestEnvForTest {
    let mut app = ready(ctx);
    app.settings.fullscreen_top_bar_locked = true;
    app.settings.fullscreen_navigator_visible = true;
    app.settings.fullscreen_navigator_corner = FullscreenNavigatorCorner::TopRight;
    app.settings.fullscreen_navigator_size = 520.0;
    app.fs_zoom = 2.0;
    for _ in 0..4 {
        render_frame(&mut app, ctx, vec![]);
    }
    assert!(
        app.fullscreen_page_layout.single_page().is_some(),
        "the production page painter must publish its layout"
    );
    assert!(navigator_layout(&mut app).content_rect.is_positive());
    app
}

fn navigator_layout(app: &mut App) -> FlatNavigatorLayout {
    let image_rect = app.fullscreen_media_rect(viewport(), 1, false);
    app.fs_navigator_layout(image_rect).unwrap()
}

fn drag(
    app: &mut App,
    ctx: &egui::Context,
    button_kind: egui::PointerButton,
    start: egui::Pos2,
    end: egui::Pos2,
) {
    render_frame(
        app,
        ctx,
        vec![
            egui::Event::PointerMoved(start),
            button(start, button_kind, true),
        ],
    );
    render_frame(app, ctx, vec![egui::Event::PointerMoved(end)]);
    render_frame(app, ctx, vec![button(end, button_kind, false)]);
}

#[test]
fn overflow_buttons_preempt_real_overlapping_navigator_before_raw_dispatch() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for state in [
        FsOverflowPanelState::Root,
        FsOverflowPanelState::Slideshow,
        FsOverflowPanelState::NavigatorPosition,
        FsOverflowPanelState::SpreadAdjustment,
    ] {
        for button_kind in [egui::PointerButton::Primary, egui::PointerButton::Secondary] {
            let ctx = egui::Context::default();
            let mut app = rendered_navigator(&ctx);
            app.fs_overflow_panel_state = state;
            render_frame(&mut app, &ctx, vec![]);
            let overlap = navigator_layout(&mut app)
                .content_rect
                .intersect(fs_overflow_panel_rect(viewport(), state).unwrap())
                .shrink(6.0);
            assert!(overlap.width() > 50.0 && overlap.height() > 70.0);
            let start = overlap.center() - egui::vec2(20.0, 25.0);
            let end = overlap.center() + egui::vec2(20.0, 25.0);
            let before = (app.fs_zoom, app.fs_pan);
            drag(&mut app, &ctx, button_kind, start, end);
            assert_eq!(
                (app.fs_zoom, app.fs_pan),
                before,
                "front {state:?} {button_kind:?} must not pan or select-zoom the painted navigator"
            );
            assert!(!fs_navigator_interaction_active(
                app.fullscreen_navigator_interaction
            ));
            assert_eq!(app.fullscreen_idx, Some(1));
        }
    }
}

#[test]
fn overflow_leaves_outside_real_navigator_pan_and_selection_zoom_working() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for button_kind in [egui::PointerButton::Primary, egui::PointerButton::Secondary] {
        let ctx = egui::Context::default();
        let mut app = rendered_navigator(&ctx);
        app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
        render_frame(&mut app, &ctx, vec![]);
        let layout = navigator_layout(&mut app);
        let panel = fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state).unwrap();
        let outside = egui::Rect::from_min_max(
            layout.content_rect.min,
            egui::pos2(panel.left(), layout.content_rect.bottom()),
        )
        .shrink(2.0);
        assert!(
            outside.width() > FS_NAVIGATOR_MIN_SELECTION * 2.0,
            "must expose real navigator content outside the front panel: {outside:?}"
        );
        let start = outside.lerp_inside(egui::vec2(0.2, 0.3));
        let end = outside.lerp_inside(egui::vec2(0.8, 0.6));
        let before = (app.fs_zoom, app.fs_pan);
        drag(&mut app, &ctx, button_kind, start, end);
        if button_kind == egui::PointerButton::Secondary {
            assert_ne!(app.fs_zoom, before.0, "outside right selection still zooms");
        } else {
            assert_ne!(app.fs_pan, before.1, "outside left drag still pans");
        }
        assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Slideshow);
    }
}

#[test]
fn overflow_same_frame_root_close_owns_raw_navigator_click_before_state_changes() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = rendered_navigator(&ctx);
    // Without a reserved top-bar gap, the front close button overlaps navigator canvas,
    // rather than its header. The 4pt click movement would pan that canvas if leaked.
    app.settings.fullscreen_top_bar_locked = false;
    app.fs_overflow_panel_state = FsOverflowPanelState::Root;
    for _ in 0..4 {
        render_frame(&mut app, &ctx, vec![]);
    }
    let close = ctx
        .read_response(egui::Id::unique("fs_overflow_panel_close"))
        .unwrap()
        .rect
        .center();
    assert!(navigator_layout(&mut app).canvas_rect.contains(close));
    render_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(close)]);
    let before = (app.fs_zoom, app.fs_pan);
    let start = close - egui::vec2(2.0, 0.0);
    let end = close + egui::vec2(2.0, 0.0);
    render_frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(start),
            button(start, egui::PointerButton::Primary, true),
            egui::Event::PointerMoved(end),
            button(end, egui::PointerButton::Primary, false),
        ],
    );
    assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
    assert_eq!((app.fs_zoom, app.fs_pan), before);
    assert_eq!(app.fullscreen_idx, Some(1));
}

#[test]
fn overflow_captured_buttons_preserve_outside_navigator_wheel() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for button_kind in [egui::PointerButton::Primary, egui::PointerButton::Secondary] {
        let ctx = egui::Context::default();
        let mut app = rendered_navigator(&ctx);
        app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
        render_frame(&mut app, &ctx, vec![]);
        let layout = navigator_layout(&mut app);
        let panel = fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state).unwrap();
        let inside = layout.content_rect.intersect(panel).center();
        let outside = egui::pos2(
            (layout.canvas_rect.left() + panel.left()) * 0.5,
            layout.canvas_rect.center().y,
        );
        assert!(!panel.contains(outside) && layout.canvas_rect.contains(outside));
        render_frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(inside),
                button(inside, button_kind, true),
            ],
        );
        let before = (app.fs_zoom, app.fs_pan);
        let size = app.settings.fullscreen_navigator_size;
        render_frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(outside),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -120.0),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Wheel,
                },
            ],
        );
        assert_ne!(
            app.settings.fullscreen_navigator_size, size,
            "unowned outside wheel must still resize the navigator"
        );
        assert_eq!(
            (app.fs_zoom, app.fs_pan),
            before,
            "the captured button must not pan/select or redirect the wheel to image zoom"
        );
        render_frame(&mut app, &ctx, vec![button(outside, button_kind, false)]);
        assert!(!app.fs_front_pointer.is_active());
        assert_eq!(app.fs_secondary_press, FullscreenSecondaryPress::Idle);
    }
}

#[test]
fn overflow_secondary_capture_preserves_outside_primary_navigator_pan() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = rendered_navigator(&ctx);
    app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
    render_frame(&mut app, &ctx, vec![]);
    let layout = navigator_layout(&mut app);
    let panel = fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state).unwrap();
    let outside = egui::pos2(
        (layout.canvas_rect.left() + panel.left()) * 0.5,
        layout.canvas_rect.center().y,
    );
    let inside = layout.content_rect.intersect(panel).center();
    render_frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(outside),
            button(outside, egui::PointerButton::Primary, true),
        ],
    );
    assert!(fs_navigator_interaction_active(
        app.fullscreen_navigator_interaction
    ));
    let before = app.fs_pan;
    render_frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(inside),
            button(inside, egui::PointerButton::Secondary, true),
        ],
    );
    assert_ne!(
        app.fs_pan, before,
        "the non-owned primary pan still receives pointer movement"
    );
    assert!(fs_navigator_interaction_active(
        app.fullscreen_navigator_interaction
    ));
    render_frame(
        &mut app,
        &ctx,
        vec![button(inside, egui::PointerButton::Secondary, false)],
    );
    assert!(fs_navigator_interaction_active(
        app.fullscreen_navigator_interaction
    ));
    render_frame(
        &mut app,
        &ctx,
        vec![button(inside, egui::PointerButton::Primary, false)],
    );
    assert!(!fs_navigator_interaction_active(
        app.fullscreen_navigator_interaction
    ));
    assert_eq!(app.fullscreen_idx, Some(1));
}

#[test]
fn overflow_primary_stream_keeps_front_ownership_until_physical_terminal() {
    let ctx = egui::Context::default();
    let mut owner = FsFrontPointerOwnership::primary_for_test();
    crate::test_ui::headless_paint_output(ctx.run_ui(egui::RawInput::default(), |ctx| {
        let frame = owner.capture(ctx, false, &[]);
        assert!(frame.primary_owned);
        assert!(owner.owns_primary());
    }))
    .drop_without_applying_deltas();
    crate::test_ui::headless_paint_output(ctx.run_ui(
        egui::RawInput {
            events: vec![button(
                egui::pos2(10.0, 10.0),
                egui::PointerButton::Primary,
                false,
            )],
            ..Default::default()
        },
        |ctx| {
            assert!(owner.capture(ctx, false, &[]).primary_owned);
            assert!(!owner.owns_primary());
        },
    ))
    .drop_without_applying_deltas();
}

#[test]
fn overflow_release_position_still_routes_wheel_outside_real_navigator() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = rendered_navigator(&ctx);
    app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
    render_frame(&mut app, &ctx, vec![]);
    let layout = navigator_layout(&mut app);
    let inside = layout
        .content_rect
        .intersect(fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state).unwrap())
        .center();
    let outside = egui::pos2(200.0, 600.0);
    assert!(!layout.panel_rect.contains(outside));
    render_frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(inside),
            button(inside, egui::PointerButton::Secondary, true),
        ],
    );
    let size = app.settings.fullscreen_navigator_size;
    // A release carries position even without a separate PointerMoved event.
    render_frame(
        &mut app,
        &ctx,
        vec![
            button(outside, egui::PointerButton::Secondary, false),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -120.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
            },
        ],
    );
    assert_eq!(
        app.settings.fullscreen_navigator_size, size,
        "the captured release's position must move the following wheel outside the navigator"
    );
    assert_eq!(app.fs_secondary_press, FullscreenSecondaryPress::Idle);
}

#[test]
fn overflow_touch_cancel_without_primary_release_allows_next_outside_navigator_drag() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = rendered_navigator(&ctx);
    app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
    render_frame(&mut app, &ctx, vec![]);
    let layout = navigator_layout(&mut app);
    let panel = fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state).unwrap();
    let inside = layout.content_rect.intersect(panel).center();
    render_frame(
        &mut app,
        &ctx,
        vec![
            touch(inside, egui::TouchPhase::Start),
            egui::Event::PointerMoved(inside),
            button(inside, egui::PointerButton::Primary, true),
        ],
    );
    assert!(app.fs_front_pointer.owns_primary());
    render_frame(
        &mut app,
        &ctx,
        vec![
            touch(inside, egui::TouchPhase::Cancel),
            egui::Event::PointerGone,
        ],
    );
    assert!(
        !app.fs_front_pointer.is_active(),
        "egui-winit Cancel has no primary release edge"
    );
    let outside = egui::pos2(
        (layout.canvas_rect.left() + panel.left()) * 0.5,
        layout.canvas_rect.center().y,
    );
    let end = outside + egui::vec2(0.0, 40.0);
    render_frame(
        &mut app,
        &ctx,
        vec![
            touch(outside, egui::TouchPhase::Start),
            egui::Event::PointerMoved(outside),
            button(outside, egui::PointerButton::Primary, true),
        ],
    );
    let before = app.fs_pan;
    render_frame(
        &mut app,
        &ctx,
        vec![
            touch(end, egui::TouchPhase::Move),
            egui::Event::PointerMoved(end),
        ],
    );
    assert_ne!(app.fs_pan, before);
    render_frame(
        &mut app,
        &ctx,
        vec![
            touch(end, egui::TouchPhase::End),
            button(end, egui::PointerButton::Primary, false),
        ],
    );
    assert!(!app.fs_front_pointer.is_active());
}

fn landscape_sources(ctx: &egui::Context) -> crate::app::AppTestEnvForTest {
    crate::ui_fonts::configure_fonts(ctx);
    page_turn::install(ctx);
    install_fs_navigator_input_tracking(ctx);
    // Populate the source before the first input/preparation frame. Replacing a source
    // after ready() has prepared the portrait leaves its accepted presentation material
    // live, which would test a source-replacement lifecycle instead of this input owner.
    let mut app = crate::app::setup_app_for_test();
    app.items = (0..4)
        .map(|idx| GridItem::Image(PathBuf::from(format!("c:/overflow/{idx}.png"))))
        .collect();
    app.visible_indices = vec![0, 1, 2, 3];
    app.fullscreen_idx = Some(1);
    app.items_generation = 41;
    app.spread_mode = SpreadMode::Single;
    app.reading_flow = ReadingFlow::Paged;
    app.settings.touch_still_chrome_learned = true;
    // This fixture begins with an already-focused viewer; focus-return click suppression
    // has separate coverage and must not intercept the outside brush control case.
    app.fs_prev_focused = true;
    #[cfg(windows)]
    {
        app.native_video_in_window_active = true;
    }
    app.settings.fullscreen_top_bar_locked = true;
    app.settings.fullscreen_navigator_visible = false;
    let pixels = Arc::new(egui::ColorImage::filled(
        [320, 160],
        egui::Color32::LIGHT_BLUE,
    ));
    for idx in 0..4 {
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex: ctx.load_texture(
                    format!("overflow-landscape-{idx}"),
                    (*pixels).clone(),
                    egui::TextureOptions::LINEAR,
                ),
                pixels: Arc::clone(&pixels),
                source_dims: Some([320, 160]),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.record_page_dims_for_spread(idx, (320, 160));
    }
    app
}

fn rendered_landscape(ctx: &egui::Context) -> crate::app::AppTestEnvForTest {
    let mut app = landscape_sources(ctx);
    for _ in 0..4 {
        render_frame(&mut app, ctx, vec![]);
    }
    let page = app
        .fullscreen_page_layout
        .single_page()
        .expect("real image drawing must publish a page");
    assert!(
        page.transform.paint_rect.width() > page.transform.paint_rect.height() * 1.5,
        "actual painted page: {:?}",
        page.transform.paint_rect
    );
    assert_eq!(page.page_idx(), 1);
    app
}

fn wipe_fraction(app: &App) -> f32 {
    match app.compare_view_mode {
        crate::app::CompareViewMode::Wipe { fraction, .. } => fraction,
        mode => panic!("wipe unexpectedly ended: {mode:?}"),
    }
}

#[test]
fn overflow_real_wipe_drag_respects_panel_press_origin_in_both_directions() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for inside in [true, false] {
        for direction in [-1.0, 1.0] {
            let ctx = egui::Context::default();
            let mut app = rendered_landscape(&ctx);
            let painted = app
                .fullscreen_page_layout
                .single_page()
                .unwrap()
                .transform
                .paint_rect;
            let panel =
                fs_overflow_panel_rect(viewport(), FsOverflowPanelState::Slideshow).unwrap();
            let overlap = painted.intersect(panel).shrink(15.0);
            assert!(
                overlap.width() > 90.0 && overlap.height() > 90.0,
                "paint={painted:?}, panel={panel:?}"
            );
            let pixels = Arc::new(egui::ColorImage::filled(
                [320, 160],
                egui::Color32::LIGHT_RED,
            ));
            app.pinned_compare_slot = Some(crate::app::PinnedCompareSlot {
                pixels: Arc::clone(&pixels),
                indicator_pixels: Arc::clone(&pixels),
                indicator_texture: None,
                display_name: "overflow pin".into(),
                source_idx: 42,
                external_item_key: None,
                source_size: [320, 160],
            });
            app.compare_preparation =
                crate::app::ComparePreparationState::Ready(crate::app::ComparePreparedPair {
                    key: 1,
                    current_idx: 1,
                    pinned_source_idx: 42,
                    current_input_size: [320, 160],
                    pinned_input_size: [320, 160],
                    target_size: [320, 160],
                    pixels: crate::app::ComparePreparedPixels::WipeTextures {
                        pinned: Arc::clone(&pixels),
                        current: pixels,
                    },
                    pinned_texture: None,
                    current_texture: None,
                    diff_texture: None,
                });
            let draw_rect = App::compare_image_draw_rect(
                app.fullscreen_media_rect(viewport(), 1, false),
                [320, 160],
                app.fs_zoom_pan(),
                app.fullscreen_fit_scale_limits(ctx.pixels_per_point()),
            )
            .unwrap();
            let start = if inside {
                overlap.center()
            } else {
                draw_rect.lerp_inside(egui::vec2(0.3, 0.5))
            };
            assert_eq!(panel.contains(start), inside);
            assert!(draw_rect.contains(start));
            let fraction = (start.x - draw_rect.left()) / draw_rect.width();
            app.compare_view_mode = crate::app::CompareViewMode::wipe(fraction);
            app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
            render_frame(&mut app, &ctx, vec![]);
            assert!(app.compare_prepared_pair_matches(1));
            assert!(!app.settings.fullscreen_navigator_visible);
            let end = start + egui::vec2(direction * 60.0, 0.0);
            drag(&mut app, &ctx, egui::PointerButton::Primary, start, end);
            if inside {
                assert_eq!(
                    wipe_fraction(&app),
                    fraction,
                    "panel drag direction={direction}"
                );
            } else {
                assert!(
                    (wipe_fraction(&app) - fraction) * direction > 0.02,
                    "outside drag direction={direction}"
                );
            }
            assert_eq!(app.fullscreen_idx, Some(1));
        }
    }
}

fn rendered_analysis(ctx: &egui::Context) -> crate::app::AppTestEnvForTest {
    let mut app = rendered_landscape(ctx);
    app.settings
        .ring_shortcuts
        .set_right_drag_mode(RightDragContext::ImageFullscreen, RightDragMode::Disabled);
    app.toggle_analysis_mode();
    app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
    for _ in 0..4 {
        render_frame(&mut app, ctx, vec![]);
    }
    assert!(app.analysis_mode);
    let page = app
        .fullscreen_page_layout
        .single_page()
        .expect("analysis must paint the real landscape page");
    assert!(page.transform.paint_rect.width() > page.transform.paint_rect.height() * 1.5);
    assert!(
        page.transform
            .paint_rect
            .intersect(fs_overflow_panel_rect(viewport(), FsOverflowPanelState::Slideshow).unwrap())
            .width()
            > 25.0
    );
    app
}

fn analysis_start(app: &App, inside: bool) -> egui::Pos2 {
    let paint = app
        .fullscreen_page_layout
        .single_page()
        .unwrap()
        .transform
        .paint_rect;
    let panel = fs_overflow_panel_rect(viewport(), FsOverflowPanelState::Slideshow).unwrap();
    let start = if inside {
        paint.intersect(panel).shrink(8.0).center()
    } else {
        paint.lerp_inside(egui::vec2(0.35, 0.5))
    };
    assert!(paint.contains(start));
    assert_eq!(panel.contains(start), inside);
    start
}

#[test]
fn overflow_real_analysis_primary_pan_and_guide_preserve_outside_operations() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for guide in [false, true] {
        for inside in [true, false] {
            let ctx = egui::Context::default();
            let mut app = rendered_analysis(&ctx);
            let start = analysis_start(&app, inside);
            let end = start + egui::vec2(-35.0, 30.0);
            let mods = if guide {
                egui::Modifiers::SHIFT
            } else {
                egui::Modifiers::NONE
            };
            let before = app.analysis_pan;
            let event = |pos, pressed| egui::Event::PointerButton {
                pos,
                pressed,
                button: egui::PointerButton::Primary,
                modifiers: mods,
            };
            render_frame_with_modifiers(
                &mut app,
                &ctx,
                vec![egui::Event::PointerMoved(start), event(start, true)],
                mods,
            );
            render_frame_with_modifiers(&mut app, &ctx, vec![egui::Event::PointerMoved(end)], mods);
            render_frame_with_modifiers(&mut app, &ctx, vec![event(end, false)], mods);
            if guide {
                if inside {
                    assert!(app.analysis_guide_drag.is_none());
                } else {
                    let (a, b, color) = app
                        .analysis_guide_drag
                        .expect("outside Shift drag still creates a guide");
                    assert_ne!(a, b);
                    assert_eq!(color, 0);
                }
                assert_eq!(app.analysis_pan, before);
            } else if inside {
                assert_eq!(app.analysis_pan, before);
            } else {
                assert_ne!(
                    app.analysis_pan, before,
                    "outside primary drag must still pan"
                );
            }
            assert!(app.analysis_pan_drag_start.is_none());
            assert_eq!(app.fullscreen_idx, Some(1));
        }
    }
}

#[test]
fn overflow_real_analysis_secondary_filter_is_blocked_only_for_the_owned_button() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for inside in [true, false] {
        let ctx = egui::Context::default();
        let mut app = rendered_analysis(&ctx);
        let start = analysis_start(&app, inside);
        let mods = egui::Modifiers::CTRL;
        render_frame_with_modifiers(&mut app, &ctx, vec![egui::Event::PointerMoved(start)], mods);
        let sampled = app
            .analysis_hover_color
            .expect("the actual analysis painter must sample the displayed pixels first");
        let event = |pressed| egui::Event::PointerButton {
            pos: start,
            pressed,
            button: egui::PointerButton::Secondary,
            modifiers: mods,
        };
        assert_eq!(app.analysis_filter_mag, 0);
        assert!(app.analysis_pinned_color.is_none());
        render_frame_with_modifiers(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(start), event(true)],
            mods,
        );
        if inside {
            assert_eq!(app.analysis_filter_mag, 0);
            assert!(app.analysis_pinned_color.is_none());
        } else {
            assert_eq!(app.analysis_filter_mag, 5);
            assert_eq!(app.analysis_pinned_color, Some(sampled));
        }
        render_frame_with_modifiers(&mut app, &ctx, vec![event(false)], mods);
        assert_eq!(app.fullscreen_idx, Some(1));
    }
}

#[test]
fn overflow_same_pass_text_entry_does_not_reuse_the_old_panel_press() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for inside in [true, false] {
        let ctx = egui::Context::default();
        let mut app = rendered_landscape(&ctx);
        app.editing_addon_declined_session = true;
        // F13 is unbound by default; F6 is already consumed by RatingItemClear.
        app.keymap = Keymap::from_ini_str("[FsImage]\nFsTextMode = F13\n");
        app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
        for _ in 0..3 {
            render_frame(&mut app, &ctx, vec![]);
        }
        let paint = app
            .fullscreen_page_layout
            .single_page()
            .unwrap()
            .transform
            .paint_rect;
        let panel = fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state).unwrap();
        let outside = paint.lerp_inside(egui::vec2(0.4, 0.5));
        let start = if inside {
            paint.intersect(panel).shrink(15.0).center()
        } else {
            outside
        };
        assert_eq!(panel.contains(start), inside);
        assert!(!panel.contains(outside));
        // Move out before the canvas consumer runs. This also proves that the old
        // foreground layer's hit test cannot accidentally make this test pass.
        render_frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(start),
                button(start, egui::PointerButton::Primary, true),
                egui::Event::Key {
                    key: egui::Key::F13,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(outside),
            ],
        );
        assert!(
            app.text_mode,
            "the actual key handler must enter text editing"
        );
        assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
        assert_eq!(
            app.text_marquee.is_some(),
            !inside,
            "old overflow press ownership must survive mode entry"
        );
        assert!(app.text_drag.is_none());
        render_frame(
            &mut app,
            &ctx,
            vec![button(outside, egui::PointerButton::Primary, false)],
        );
        assert_eq!(app.fullscreen_idx, Some(1));
    }
}

#[test]
fn overflow_real_bookmark_row_keeps_press_ownership_after_release_moves_outside() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 800.0));
    for inside in [true, false] {
        let ctx = egui::Context::default();
        // Install the ZIP identities before any display material is accepted.
        let mut app = landscape_sources(&ctx);
        for idx in [1, 3] {
            app.items[idx] = GridItem::ZipImage {
                zip_path: PathBuf::from("c:/overflow/book.zip"),
                entry_name: format!("{idx}.png"),
            };
        }
        let draft = app
            .current_book_bookmark_draft(3)
            .expect("ZIP page has a bookmark identity");
        let container_key = crate::book_bookmarks::container_key(&draft.container_path);
        app.current_book_bookmarks = vec![crate::book_bookmarks::BookBookmark {
            id: 139601,
            container_key: container_key.clone(),
            container_path: draft.container_path,
            container_kind: draft.container_kind,
            page_identity: draft.page_identity,
            page_index_hint: draft.page_index_hint,
            created_at_ms: 1,
            // Frame rows use their content's natural width. A realistic Japanese title
            // makes this row reach the overflow edge at 640pt; the outer 292pt panel
            // alone does not guarantee that a short ASCII row occupies the overlap.
            title: Some("入力所有を確認する移動先ページ".into()),
        }];
        app.current_book_bookmarks_key = Some(crate::app::BookBookmarkProjectionKey {
            container_key,
            visibility: app.listing_file_type_visibility().clone(),
            context_id: app.virtual_list_context_id(),
            items_generation: app.items_generation,
        });
        app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
        app.adjustment_mode = crate::ui_helpers::MetadataPanelOpenState::ByPointer;
        app.settings.fullscreen_left_panel_tab = crate::settings::FullscreenLeftPanelTab::Bookmarks;
        app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
        for _ in 0..5 {
            render_frame_in_rect(&mut app, &ctx, vec![], egui::Modifiers::NONE, rect);
        }
        let row: egui::Rect = ctx
            .data(|data| data.get_temp(egui::Id::unique(("fs_bookmark_row_test_rect", 139601_i64))))
            .expect("the production bookmark row must actually be drawn");
        let panel = fs_overflow_panel_rect(rect, FsOverflowPanelState::Slideshow).unwrap();
        let start = egui::pos2(
            panel.left() + if inside { 2.0 } else { -2.0 },
            row.center().y,
        );
        let end = start - egui::vec2(4.0, 0.0);
        assert!(
            row.contains(start) && row.contains(end),
            "row={row:?}, panel={panel:?}"
        );
        assert_eq!(panel.contains(start), inside);
        assert!(!panel.contains(end));
        render_frame_in_rect(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(start)],
            egui::Modifiers::NONE,
            rect,
        );
        render_frame_in_rect(
            &mut app,
            &ctx,
            vec![button(start, egui::PointerButton::Primary, true)],
            egui::Modifiers::NONE,
            rect,
        );
        render_frame_in_rect(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(end),
                button(end, egui::PointerButton::Primary, false),
            ],
            egui::Modifiers::NONE,
            rect,
        );
        assert_eq!(
            app.fullscreen_idx,
            Some(if inside { 1 } else { 3 }),
            "only an outside-start bookmark click may jump"
        );
    }
}

#[test]
fn overflow_same_pass_local_adjust_entry_preserves_owned_press_and_outside_brush() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for inside in [true, false] {
        let ctx = egui::Context::default();
        let mut app = rendered_landscape(&ctx);
        app.editing_addon_declined_session = true;
        app.keymap = Keymap::from_ini_str("[FsImage]\nFsLocalAdjustMode = F13\n");
        app.local_adjust_page_layers.insert(
            1,
            Arc::new(vec![local_adjust_core::LocalAdjustmentLayer::new(
                "overflow mask",
                local_adjust_core::LocalMask::RasterVector(
                    local_adjust_core::RasterVectorMask::empty(320, 160),
                ),
                local_adjust_core::LocalEffect::None,
            )]),
        );
        app.local_adjust_selected_layers.insert(1, 0);
        app.local_adjust_mask_edit_target = crate::app::LocalAdjustMaskEditTarget::Base;
        app.local_adjust_mask_tool = crate::app::LocalAdjustMaskTool::Brush;
        app.local_adjust_mask_paint_add = true;
        app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
        for _ in 0..3 {
            render_frame(&mut app, &ctx, vec![]);
        }
        let paint = app
            .fullscreen_page_layout
            .single_page()
            .unwrap()
            .transform
            .paint_rect;
        let panel = fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state).unwrap();
        let outside = paint.lerp_inside(egui::vec2(0.4, 0.5));
        assert!(!app.local_adjust_panel_rect(viewport()).contains(outside));
        assert!(
            !app.local_adjust_tool_panel_rect(viewport())
                .contains(outside)
        );
        let start = if inside {
            paint.intersect(panel).shrink(15.0).center()
        } else {
            outside
        };
        assert_eq!(panel.contains(start), inside);
        let before = Arc::clone(&app.local_adjust_page_layers[&1]);
        render_frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(start),
                button(start, egui::PointerButton::Primary, true),
                egui::Event::Key {
                    key: egui::Key::F13,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(outside),
            ],
        );
        assert!(
            app.local_adjust_mode,
            "the production key path must enter local adjustment"
        );
        assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
        if inside {
            assert!(app.local_adjust_mask_brush_stroke.is_none());
            assert_eq!(app.local_adjust_page_layers[&1].as_ref(), before.as_ref());
        } else {
            assert!(
                app.local_adjust_mask_brush_stroke.is_some(),
                "outside press must still start a real mask stroke"
            );
            assert_ne!(app.local_adjust_page_layers[&1].as_ref(), before.as_ref());
        }
        render_frame(
            &mut app,
            &ctx,
            vec![button(outside, egui::PointerButton::Primary, false)],
        );
        assert_eq!(app.fullscreen_idx, Some(1));
    }
}

#[test]
fn overflow_held_press_cannot_become_conceal_pipeline_eye_preview_after_mode_entry() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = rendered_landscape(&ctx);
    app.conceal_tool = crate::conceal::ConcealTool::Brush;
    app.keymap = Keymap::from_ini_str("[FsImage]\nFsConcealMode = F6\n");
    app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
    for _ in 0..3 {
        render_frame(&mut app, &ctx, vec![]);
    }
    let paint = app
        .fullscreen_page_layout
        .single_page()
        .unwrap()
        .transform
        .paint_rect;
    let start = paint
        .intersect(fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state).unwrap())
        .shrink(15.0)
        .center();
    render_frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(start),
            button(start, egui::PointerButton::Primary, true),
        ],
    );
    assert!(app.fs_front_pointer.owns_primary());
    render_frame(
        &mut app,
        &ctx,
        vec![egui::Event::Key {
            key: egui::Key::F6,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
    // Adopt through the real worker-completion entry. The worker's disk/decode timing
    // is unrelated to pointer ownership and must not determine this regression.
    assert!(app.complete_conceal_mode_entry(
        1,
        Arc::new(egui::ColorImage::filled(
            [320, 160],
            egui::Color32::LIGHT_BLUE
        )),
        "overflow-fixture",
        vec![false; 320 * 160],
        vec![],
    ));
    render_frame(&mut app, &ctx, vec![]);
    let canvas = app
        .fullscreen_page_layout
        .single_page()
        .unwrap()
        .transform
        .paint_rect
        .lerp_inside(egui::vec2(0.4, 0.5));
    assert!(!app.conceal_panel_rect(viewport()).contains(canvas));
    render_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(canvas)]);
    assert!(
        !app.conceal_mask
            .as_ref()
            .unwrap()
            .iter()
            .any(|pixel| *pixel),
        "the old panel hold cannot paint the new conceal canvas"
    );
    let eye = app
        .conceal_preview_button_rect
        .expect("real conceal overlay must draw the eye control")
        .center();
    let probe = || {
        ctx.data(|data| {
            data.get_temp::<bool>(egui::Id::unique("conceal_pipeline_preview_test_active"))
        })
        .expect("sample preview before the overlay can overwrite it")
    };
    render_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(eye)]);
    assert!(
        !probe(),
        "the overflow-owned physical hold cannot activate pipeline preview"
    );
    render_frame(
        &mut app,
        &ctx,
        vec![button(eye, egui::PointerButton::Primary, false)],
    );
    assert!(!app.fs_front_pointer.is_active());
    render_frame(
        &mut app,
        &ctx,
        vec![button(eye, egui::PointerButton::Primary, true)],
    );
    assert!(
        probe(),
        "a new press on the eye keeps the existing preview behavior"
    );
    render_frame(
        &mut app,
        &ctx,
        vec![button(eye, egui::PointerButton::Primary, false)],
    );
    render_frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(canvas),
            button(canvas, egui::PointerButton::Primary, true),
        ],
    );
    assert!(
        app.conceal_mask
            .as_ref()
            .unwrap()
            .iter()
            .any(|pixel| *pixel),
        "a fresh canvas press must still paint conceal masks"
    );
    render_frame(
        &mut app,
        &ctx,
        vec![button(canvas, egui::PointerButton::Primary, false)],
    );
}

#[test]
fn overflow_same_pass_erase_entry_preserves_owned_press_and_outside_brush() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for inside in [true, false] {
        let ctx = egui::Context::default();
        let mut app = rendered_landscape(&ctx);
        app.keymap = Keymap::from_ini_str("[FsImage]\nFsEraseMode = F6\n");
        app.erase_tool = crate::app::EraseTool::Brush;
        app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
        for _ in 0..3 {
            render_frame(&mut app, &ctx, vec![]);
        }
        let paint = app
            .fullscreen_page_layout
            .single_page()
            .unwrap()
            .transform
            .paint_rect;
        let panel = fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state).unwrap();
        let outside = paint.lerp_inside(egui::vec2(0.4, 0.5));
        let start = if inside {
            paint.intersect(panel).shrink(15.0).center()
        } else {
            outside
        };
        assert_eq!(panel.contains(start), inside);
        render_frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(start),
                button(start, egui::PointerButton::Primary, true),
                egui::Event::Key {
                    key: egui::Key::F6,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(outside),
            ],
        );
        assert!(
            app.erase_mode,
            "the production key path must enter erase mode"
        );
        assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
        assert_eq!(
            app.erase_mask.as_ref().unwrap().iter().any(|pixel| *pixel),
            !inside,
            "only an outside press may start painting"
        );
        render_frame(
            &mut app,
            &ctx,
            vec![button(outside, egui::PointerButton::Primary, false)],
        );
        assert_eq!(app.fullscreen_idx, Some(1));
    }
}

#[test]
fn overflow_same_pass_sns_entry_preserves_owned_press_and_outside_drag() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for inside in [true, false] {
        let ctx = egui::Context::default();
        let mut app = rendered_landscape(&ctx);
        app.keymap = Keymap::from_ini_str("[FsImage]\nFsSnsSplitMode = F6\n");
        app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
        for _ in 0..3 {
            render_frame(&mut app, &ctx, vec![]);
        }
        let paint = app
            .fullscreen_page_layout
            .single_page()
            .unwrap()
            .transform
            .paint_rect;
        let panel = fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state).unwrap();
        let outside = paint.lerp_inside(egui::vec2(0.4, 0.5));
        let start = if inside {
            paint.intersect(panel).shrink(15.0).center()
        } else {
            outside
        };
        assert_eq!(panel.contains(start), inside);
        render_frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(start),
                button(start, egui::PointerButton::Primary, true),
                egui::Event::Key {
                    key: egui::Key::F6,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(outside),
            ],
        );
        assert!(
            app.sns_split.is_some(),
            "the assigned FsSnsSplitMode key must enter SNS split"
        );
        assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
        assert_eq!(
            app.sns_split_drag.is_some(),
            !inside,
            "only an outside press may begin the split drag"
        );
        render_frame(
            &mut app,
            &ctx,
            vec![button(outside, egui::PointerButton::Primary, false)],
        );
        assert!(app.sns_split_drag.is_none());
        assert_eq!(app.fullscreen_idx, Some(1));
    }
}

#[test]
fn overflow_real_seek_ignores_panel_press_but_accepts_a_fresh_outside_click() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = rendered_landscape(&ctx);
    app.settings.set_still_seek_bar_locked(true);
    app.settings.still_seek_strip_visible = false;
    app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
    for _ in 0..4 {
        render_frame(&mut app, &ctx, vec![]);
    }
    assert!(
        app.fs_seek_overlay_visible,
        "the real seek overlay must be painted"
    );
    let panel = fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state).unwrap();
    let start = panel.center();
    let bar = app
        .still_seek_geometry_for_idx(viewport(), 1, false)
        .bar_rect(viewport())
        .unwrap();
    let target = bar.lerp_inside(egui::vec2(0.65, 0.5));
    assert!(!panel.contains(target));
    render_frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(start),
            button(start, egui::PointerButton::Primary, true),
        ],
    );
    render_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(target)]);
    render_frame(
        &mut app,
        &ctx,
        vec![button(target, egui::PointerButton::Primary, false)],
    );
    assert_eq!(
        app.fullscreen_idx,
        Some(1),
        "the panel's released press cannot become a seek click"
    );
    assert!(matches!(
        app.fs_seek_gesture,
        StillSeekGesture::Idle | StillSeekGesture::StripCommitted { .. }
    ));
    render_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(target)]);
    render_frame(
        &mut app,
        &ctx,
        vec![button(target, egui::PointerButton::Primary, true)],
    );
    render_frame(
        &mut app,
        &ctx,
        vec![button(target, egui::PointerButton::Primary, false)],
    );
    assert_ne!(
        app.fullscreen_idx,
        Some(1),
        "fresh outside press must still reach the actual seek handler"
    );
}

#[test]
fn overflow_real_touch_menu_key_dismissal_keeps_the_physical_contact_owned() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = rendered_landscape(&ctx);
    app.fs_zoom = 2.0;
    app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
    for _ in 0..4 {
        render_frame(&mut app, &ctx, vec![]);
    }
    let start = fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state)
        .unwrap()
        .center();
    let outside = egui::pos2(150.0, 400.0);
    assert!(
        !fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state)
            .unwrap()
            .contains(outside)
    );
    let before = (app.fs_zoom, app.fs_pan);
    render_frame(
        &mut app,
        &ctx,
        vec![
            touch(start, egui::TouchPhase::Start),
            egui::Event::PointerMoved(start),
            button(start, egui::PointerButton::Primary, true),
            egui::Event::Key {
                key: egui::Key::F13,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
    render_frame(
        &mut app,
        &ctx,
        vec![
            touch(outside, egui::TouchPhase::Move),
            egui::Event::PointerMoved(outside),
        ],
    );
    render_frame(
        &mut app,
        &ctx,
        vec![
            touch(outside, egui::TouchPhase::End),
            button(outside, egui::PointerButton::Primary, false),
        ],
    );
    render_frame(&mut app, &ctx, vec![egui::Event::PointerGone]);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!((app.fs_zoom, app.fs_pan), before);
    assert!(!app.fs_front_pointer.is_active());
    let end = outside + egui::vec2(50.0, 35.0);
    render_frame(
        &mut app,
        &ctx,
        vec![
            touch(outside, egui::TouchPhase::Start),
            egui::Event::PointerMoved(outside),
            button(outside, egui::PointerButton::Primary, true),
        ],
    );
    render_frame(
        &mut app,
        &ctx,
        vec![
            touch(end, egui::TouchPhase::Move),
            egui::Event::PointerMoved(end),
        ],
    );
    assert_ne!(
        app.fs_pan, before.1,
        "a fresh outside contact must still pan the zoomed page"
    );
    render_frame(
        &mut app,
        &ctx,
        vec![
            touch(end, egui::TouchPhase::End),
            button(end, egui::PointerButton::Primary, false),
        ],
    );
    assert!(!app.fs_front_pointer.is_active());
}

#[test]
fn overflow_primary_press_does_not_cancel_an_outside_secondary_ring_or_gesture() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for mode in [RightDragMode::RingShortcut, RightDragMode::MouseGesture] {
        let ctx = egui::Context::default();
        let mut app = rendered_landscape(&ctx);
        app.settings
            .ring_shortcuts
            .set_right_drag_mode(RightDragContext::ImageFullscreen, mode.clone());
        app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
        for _ in 0..3 {
            render_frame(&mut app, &ctx, vec![]);
        }
        let panel = fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state).unwrap();
        let outside = egui::pos2(350.0, 400.0);
        let inside = panel.center();
        assert!(!panel.contains(outside));
        render_frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(outside),
                button(outside, egui::PointerButton::Secondary, true),
            ],
        );
        let recognizer_active = |app: &App| match mode {
            RightDragMode::RingShortcut => app.mouse_ring_flick.is_some(),
            RightDragMode::MouseGesture => app.mouse_gesture.is_some(),
            _ => unreachable!(),
        };
        assert!(recognizer_active(&app));
        render_frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(inside),
                button(inside, egui::PointerButton::Primary, true),
            ],
        );
        assert!(
            recognizer_active(&app),
            "the front primary press cannot cancel another button's outside owner: {mode:?}"
        );
        render_frame(
            &mut app,
            &ctx,
            vec![button(inside, egui::PointerButton::Primary, false)],
        );
        assert!(recognizer_active(&app));
        render_frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(outside),
                button(outside, egui::PointerButton::Secondary, false),
            ],
        );
        assert!(app.mouse_ring_flick.is_none() && app.mouse_gesture.is_none());
    }
}

#[test]
fn overflow_real_metadata_hold_preview_requires_an_unowned_new_press() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = rendered_landscape(&ctx);
    app.fs_info_panel.locked = true;
    app.similar_panel.select_similar_tab_for_test();
    let origin = match &app.items[1] {
        GridItem::Image(path) => path.clone(),
        _ => unreachable!(),
    };
    let target = app.tmp.path().join("metadata-candidate.png");
    let target_key = crate::similar_index::item_key_for_file(&target);
    app.similar_panel
        .set_item_query_override_for_test(crate::similar_index::ItemQuery::Ready(
            crate::similar_index::ItemMatches {
                origin: crate::similar_index::OriginItem {
                    item_key: crate::similar_index::item_key_for_file(&origin),
                    kind: crate::similar_db::ItemKind::Image,
                    mtime: 1,
                    file_size: 1,
                    width: 320,
                    height: 160,
                    format: crate::similar_image::SimilarImageFormat::Png,
                    target: Some(crate::similar_index::SimilarItemTarget::File(origin)),
                },
                hits: vec![crate::similar_index::QueryHit {
                    item_id: 139601,
                    item_key: target_key.clone(),
                    kind: crate::similar_db::ItemKind::Image,
                    container_key: None,
                    page_index: None,
                    distance: 1,
                    band: crate::similar_index::MatchBand::OtherVersion,
                    mtime: 1,
                    file_size: 1,
                    width: 320,
                    height: 160,
                    format: crate::similar_image::SimilarImageFormat::Png,
                    target: Some(crate::similar_index::SimilarItemTarget::File(target)),
                }],
            },
        ));
    app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
    for _ in 0..5 {
        render_frame(&mut app, &ctx, vec![]);
    }
    let peek: egui::Rect = ctx
        .data(|data| {
            data.get_temp(egui::Id::unique((
                "similar_hit_peek_test_rect",
                target_key.clone(),
            )))
        })
        .expect("the actual Similar-tab candidate must draw its hold-preview button");
    let pos = peek.center();
    let panel = fs_overflow_panel_rect(viewport(), app.fs_overflow_panel_state).unwrap();
    assert!(
        panel.contains_rect(peek),
        "hold button={peek:?}, overflow={panel:?}"
    );
    render_frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(pos),
            button(pos, egui::PointerButton::Primary, true),
        ],
    );
    assert!(app.fs_front_pointer.owns_primary());
    assert!(
        app.similar_panel
            .preview
            .active_candidate_for_test()
            .is_none()
    );
    render_frame(
        &mut app,
        &ctx,
        vec![egui::Event::Key {
            key: egui::Key::F13,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
    assert!(
        app.similar_panel
            .preview
            .active_candidate_for_test()
            .is_none(),
        "closing the menu while held cannot reuse its old press for preview"
    );
    render_frame(
        &mut app,
        &ctx,
        vec![button(pos, egui::PointerButton::Primary, false)],
    );
    assert!(!app.fs_front_pointer.is_active());
    let _completion = app
        .similar_panel
        .preview
        .queue_test_completion_for_next_press();
    render_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(pos)]);
    render_frame(
        &mut app,
        &ctx,
        vec![button(pos, egui::PointerButton::Primary, true)],
    );
    let (candidate, _) = app
        .similar_panel
        .preview
        .active_candidate_for_test()
        .expect("a fresh unowned press on the actual metadata button must begin preview");
    assert_eq!(candidate.item_key, target_key);
    render_frame(
        &mut app,
        &ctx,
        vec![button(pos, egui::PointerButton::Primary, false)],
    );
    assert!(
        app.similar_panel
            .preview
            .active_candidate_for_test()
            .is_none()
    );
}

fn overlapping_popup_viewport() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1093.0, 614.0))
}

fn overflow_area_layer() -> egui::LayerId {
    egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("fs_overflow_panel_foreground"),
    )
}

fn seek_popup_id() -> egui::Id {
    egui::Id::unique(FULLSCREEN_STILL_SEEK_STRIP_BUTTON_ID).with("popup")
}

fn painted_text_center(output: &egui::FullOutput, expected: &str) -> egui::Pos2 {
    fn find(shape: &egui::Shape, clip: egui::Rect, expected: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == expected => {
                let center = text.galley.rect.translate(text.pos.to_vec2()).center();
                clip.contains(center).then_some(center)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, clip, expected)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, shape.clip_rect, expected))
        .unwrap_or_else(|| panic!("the actual seek popup must visibly paint {expected:?}"))
}

fn open_real_seek_popup_above_slideshow(app: &mut App, ctx: &egui::Context) -> egui::Pos2 {
    let rect = overlapping_popup_viewport();
    app.settings.set_still_seek_bar_locked(true);
    app.settings.still_seek_strip_visible = false;
    app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
    for _ in 0..4 {
        render_frame_in_rect(app, ctx, vec![], egui::Modifiers::NONE, rect);
    }
    let film = ctx
        .read_response(egui::Id::unique(FULLSCREEN_STILL_SEEK_STRIP_BUTTON_ID))
        .expect("the real locked seek bar must expose its film button")
        .rect
        .center();
    render_frame_in_rect(
        app,
        ctx,
        vec![egui::Event::PointerMoved(film)],
        egui::Modifiers::NONE,
        rect,
    );
    render_frame_in_rect(
        app,
        ctx,
        vec![button(film, egui::PointerButton::Primary, true)],
        egui::Modifiers::NONE,
        rect,
    );
    render_frame_in_rect(
        app,
        ctx,
        vec![button(film, egui::PointerButton::Primary, false)],
        egui::Modifiers::NONE,
        rect,
    );
    assert!(fs_still_seek_strip_popup_open(ctx));
    assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Slideshow);
    for _ in 0..3 {
        render_frame_in_rect(app, ctx, vec![], egui::Modifiers::NONE, rect);
    }
    let output = render_output_in_rect(app, ctx, vec![], egui::Modifiers::NONE, rect);
    // Smallest is below the overflow panel at this viewport; use the next
    // actual row so the hit genuinely lies in both visible surfaces.
    let preset = crate::settings::StillSeekStripHeight::Small;
    let labels = crate::seek_strip_menu::SeekStripMenuLabels::for_viewport(rect);
    let label = labels.preset(
        preset.label(),
        app.settings.still_seek_strip_height_values.points(preset),
    );
    let pos = painted_text_center(&output, &label);
    output.drop_without_applying_deltas();
    let overflow = fs_overflow_panel_rect(rect, app.fs_overflow_panel_state).unwrap();
    let popup = egui::AreaState::load(ctx, seek_popup_id())
        .expect("the real seek menu must register an Area")
        .rect();
    assert!(
        overflow.contains(pos) && popup.contains(pos),
        "selected row must truly overlap: row={pos:?}, overflow={overflow:?}, popup={popup:?}"
    );
    assert_ne!(
        ctx.layer_id_at(pos),
        Some(overflow_area_layer()),
        "the newer seek popup must actually be above overflow"
    );
    pos
}

#[test]
fn overflow_top_layer_seek_popup_accepts_real_mouse_and_touch_row_selection() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for promoted_touch in [false, true] {
        let ctx = egui::Context::default();
        let mut app = rendered_landscape(&ctx);
        let pos = open_real_seek_popup_above_slideshow(&mut app, &ctx);
        assert_ne!(
            app.settings.still_seek_strip_height,
            crate::settings::StillSeekStripHeight::Small
        );
        let exclusions = app
            .capture_fs_front_pointer_input(&ctx, overlapping_popup_viewport())
            .1;
        assert!(
            exclusions.iter().any(|rect| rect.contains(pos)),
            "front touch exclusion blocks canvas, while the higher seek row remains selectable"
        );
        let mut press = vec![
            egui::Event::PointerMoved(pos),
            button(pos, egui::PointerButton::Primary, true),
        ];
        if promoted_touch {
            press.insert(0, touch(pos, egui::TouchPhase::Start));
        }
        render_frame_in_rect(
            &mut app,
            &ctx,
            press,
            egui::Modifiers::NONE,
            overlapping_popup_viewport(),
        );
        assert_eq!(
            app.fs_front_pointer.owns_primary(),
            true,
            "higher popup press belongs to the front menu"
        );
        let mut release = vec![button(pos, egui::PointerButton::Primary, false)];
        if promoted_touch {
            release.insert(0, touch(pos, egui::TouchPhase::End));
        }
        render_frame_in_rect(
            &mut app,
            &ctx,
            release,
            egui::Modifiers::NONE,
            overlapping_popup_viewport(),
        );
        assert_eq!(
            app.settings.still_seek_strip_height,
            crate::settings::StillSeekStripHeight::Small,
            "the real popup row must remain selectable: touch={promoted_touch}"
        );
        assert!(!fs_still_seek_strip_popup_open(&ctx));
        assert_eq!(app.fullscreen_idx, Some(1));
    }
}

#[test]
fn overflow_top_layer_secondary_press_does_not_claim_the_higher_seek_popup() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = rendered_landscape(&ctx);
    let pos = open_real_seek_popup_above_slideshow(&mut app, &ctx);
    render_frame_in_rect(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(pos),
            button(pos, egui::PointerButton::Secondary, true),
        ],
        egui::Modifiers::NONE,
        overlapping_popup_viewport(),
    );
    assert!(
        app.fs_front_pointer.buttons[1],
        "secondary uses the same actual front layer as primary and touch"
    );
    // Releasing in another popup retains master behavior, including its existing
    // fullscreen close/gesture rules. This change only restricts overflow ownership.
    render_frame_in_rect(
        &mut app,
        &ctx,
        vec![button(pos, egui::PointerButton::Secondary, false)],
        egui::Modifiers::NONE,
        overlapping_popup_viewport(),
    );
}

#[test]
fn overflow_nonoverlapping_front_region_still_shields_the_real_wipe_with_seek_popup_open() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = rendered_landscape(&ctx);
    open_real_seek_popup_above_slideshow(&mut app, &ctx);
    let rect = overlapping_popup_viewport();
    let popup = egui::AreaState::load(&ctx, seek_popup_id()).unwrap().rect();
    let overflow = fs_overflow_panel_rect(rect, app.fs_overflow_panel_state).unwrap();
    let draw = App::compare_image_draw_rect(
        app.fullscreen_media_rect(rect, 1, false),
        [320, 160],
        app.fs_zoom_pan(),
        app.fullscreen_fit_scale_limits(ctx.pixels_per_point()),
    )
    .unwrap();
    let unoccluded =
        egui::Rect::from_min_max(overflow.min, egui::pos2(overflow.right(), popup.top()))
            .intersect(draw)
            .shrink(12.0);
    assert!(
        unoccluded.width() > 80.0 && unoccluded.height() > 50.0,
        "must expose real overflow above the seek popup: {unoccluded:?}"
    );
    let start = unoccluded.center();
    assert!(!popup.contains(start));
    assert_eq!(ctx.layer_id_at(start), Some(overflow_area_layer()));
    let pixels = Arc::new(egui::ColorImage::filled(
        [320, 160],
        egui::Color32::LIGHT_RED,
    ));
    app.pinned_compare_slot = Some(crate::app::PinnedCompareSlot {
        pixels: Arc::clone(&pixels),
        indicator_pixels: Arc::clone(&pixels),
        indicator_texture: None,
        display_name: "front ownership pin".into(),
        source_idx: 42,
        external_item_key: None,
        source_size: [320, 160],
    });
    app.compare_preparation =
        crate::app::ComparePreparationState::Ready(crate::app::ComparePreparedPair {
            key: 1,
            current_idx: 1,
            pinned_source_idx: 42,
            current_input_size: [320, 160],
            pinned_input_size: [320, 160],
            target_size: [320, 160],
            pixels: crate::app::ComparePreparedPixels::WipeTextures {
                pinned: Arc::clone(&pixels),
                current: pixels,
            },
            pinned_texture: None,
            current_texture: None,
            diff_texture: None,
        });
    let fraction = (start.x - draw.left()) / draw.width();
    app.compare_view_mode = crate::app::CompareViewMode::wipe(fraction);
    let exclusions = app.capture_fs_front_pointer_input(&ctx, rect).1;
    assert!(exclusions.iter().any(|rect| rect.contains(start)));
    render_frame_in_rect(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(start),
            button(start, egui::PointerButton::Primary, true),
        ],
        egui::Modifiers::NONE,
        rect,
    );
    assert!(app.fs_front_pointer.owns_primary());
    let end = start - egui::vec2(55.0, 0.0);
    render_frame_in_rect(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(end)],
        egui::Modifiers::NONE,
        rect,
    );
    render_frame_in_rect(
        &mut app,
        &ctx,
        vec![button(end, egui::PointerButton::Primary, false)],
        egui::Modifiers::NONE,
        rect,
    );
    assert_eq!(wipe_fraction(&app), fraction);
    assert_eq!(app.fullscreen_idx, Some(1));
}

#[test]
fn overflow_first_open_and_sizing_pass_claim_each_button_before_the_area_can_interact() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for passes in [1, 2] {
        for (kind, promoted_touch) in [
            (egui::PointerButton::Primary, false),
            (egui::PointerButton::Secondary, false),
            (egui::PointerButton::Primary, true),
        ] {
            let ctx = egui::Context::default();
            ctx.options_mut(|options| options.max_passes = passes.try_into().unwrap());
            let mut app = rendered_landscape(&ctx);
            app.fs_zoom = 2.0;
            let rect = overlapping_popup_viewport();
            for _ in 0..3 {
                render_frame_in_rect(&mut app, &ctx, vec![], egui::Modifiers::NONE, rect);
            }
            assert!(egui::AreaState::load(&ctx, overflow_area_layer().id).is_none());
            app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
            let inside = fs_overflow_panel_rect(rect, app.fs_overflow_panel_state)
                .unwrap()
                .center();
            let before = app.fs_pan;
            let mut press = vec![
                egui::Event::PointerMoved(inside),
                button(inside, kind, true),
            ];
            if promoted_touch {
                press.insert(0, touch(inside, egui::TouchPhase::Start));
            }
            render_frame_in_rect(&mut app, &ctx, press, egui::Modifiers::NONE, rect);
            let area = egui::AreaState::load(&ctx, overflow_area_layer().id)
                .expect("first production draw must register overflow's Area");
            if passes == 1 {
                assert!(
                    !area.interactable,
                    "one-pass first draw must really exercise egui's noninteractive sizing pass"
                );
            }
            if kind == egui::PointerButton::Primary {
                assert_eq!(
                    app.fs_front_pointer.owns_primary(),
                    true,
                    "first-open primary/touch ownership must not depend on last frame's area"
                );
            } else {
                assert!(app.fs_front_pointer.buttons[1]);
            }
            let end = inside - egui::vec2(70.0, 50.0);
            let mut movement = vec![egui::Event::PointerMoved(end)];
            if promoted_touch {
                movement.insert(0, touch(end, egui::TouchPhase::Move));
            }
            render_frame_in_rect(&mut app, &ctx, movement, egui::Modifiers::NONE, rect);
            let mut release = vec![button(end, kind, false)];
            if promoted_touch {
                release.insert(0, touch(end, egui::TouchPhase::End));
            }
            render_frame_in_rect(&mut app, &ctx, release, egui::Modifiers::NONE, rect);
            assert_eq!(
                app.fs_pan, before,
                "sizing/open press must not pan the backing image: passes={passes}, button={kind:?}, touch={promoted_touch}"
            );
            assert_eq!(app.fullscreen_idx, Some(1));
            assert!(!app.fs_front_pointer.is_active());
            assert_eq!(app.fs_secondary_press, FullscreenSecondaryPress::Idle);
        }
    }
}

#[test]
fn overflow_closed_state_does_not_reclaim_buttons_or_touch_from_a_stale_area() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for (kind, promoted_touch) in [
        (egui::PointerButton::Primary, false),
        (egui::PointerButton::Secondary, false),
        (egui::PointerButton::Primary, true),
    ] {
        let ctx = egui::Context::default();
        let mut app = rendered_landscape(&ctx);
        app.fs_zoom = 2.0;
        let rect = overlapping_popup_viewport();
        app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
        for _ in 0..4 {
            render_frame_in_rect(&mut app, &ctx, vec![], egui::Modifiers::NONE, rect);
        }
        let old = fs_overflow_panel_rect(rect, app.fs_overflow_panel_state).unwrap();
        let start = old.center();
        assert_eq!(ctx.layer_id_at(start), Some(overflow_area_layer()));
        assert!(egui::AreaState::load(&ctx, overflow_area_layer().id).is_some());
        // Close through the state owner before the next pass. The previous frame's
        // Area still exists in egui memory; it must not resurrect a closed menu.
        app.fs_overflow_panel_state = FsOverflowPanelState::Closed;
        let (closed_input, closed_surface) = app.capture_fs_front_pointer_input(&ctx, rect);
        assert!(!closed_input.owns(kind));
        assert!(
            closed_surface.is_empty(),
            "a closed panel has no touch exclusion despite its retained Area"
        );
        let mut press = vec![egui::Event::PointerMoved(start), button(start, kind, true)];
        if promoted_touch {
            press.insert(0, touch(start, egui::TouchPhase::Start));
        }
        render_frame_in_rect(&mut app, &ctx, press, egui::Modifiers::NONE, rect);
        assert_ne!(app.fs_front_pointer.owns_primary(), true);
        assert!(!app.fs_front_pointer.buttons[1]);
        let end = start - egui::vec2(70.0, 50.0);
        if kind == egui::PointerButton::Primary {
            let mut movement = vec![egui::Event::PointerMoved(end)];
            if promoted_touch {
                movement.insert(0, touch(end, egui::TouchPhase::Move));
            }
            render_frame_in_rect(&mut app, &ctx, movement, egui::Modifiers::NONE, rect);
        }
        // Keep the unowned right-click a short click. A release event itself
        // carries position and must not accidentally turn this into a drag.
        let release_pos = if kind == egui::PointerButton::Secondary {
            start
        } else {
            end
        };
        let mut release = vec![button(release_pos, kind, false)];
        if promoted_touch {
            release.insert(0, touch(end, egui::TouchPhase::End));
        }
        render_frame_in_rect(&mut app, &ctx, release, egui::Modifiers::NONE, rect);
        if kind == egui::PointerButton::Secondary {
            assert_eq!(
                app.fullscreen_idx, None,
                "master's fresh unowned short right-click still closes fullscreen"
            );
        } else {
            assert_eq!(app.fullscreen_idx, Some(1));
            // egui can retain its previous widget as the owner of the first
            // press at the just-closed Area. That master's Response lifetime
            // is separate from overflow's captured ownership: end that press,
            // then verify that a genuinely fresh canvas contact still works.
            render_frame_in_rect(
                &mut app,
                &ctx,
                vec![egui::Event::PointerGone],
                egui::Modifiers::NONE,
                rect,
            );
            assert_ne!(ctx.layer_id_at(start), Some(overflow_area_layer()));
            render_frame_in_rect(
                &mut app,
                &ctx,
                vec![egui::Event::PointerMoved(start)],
                egui::Modifiers::NONE,
                rect,
            );
            let before = app.fs_pan;
            let mut fresh = vec![button(start, egui::PointerButton::Primary, true)];
            if promoted_touch {
                fresh.insert(0, touch(start, egui::TouchPhase::Start));
            }
            render_frame_in_rect(&mut app, &ctx, fresh, egui::Modifiers::NONE, rect);
            let fresh_end = start + egui::vec2(45.0, 25.0);
            let mut movement = vec![egui::Event::PointerMoved(fresh_end)];
            if promoted_touch {
                movement.insert(0, touch(fresh_end, egui::TouchPhase::Move));
            }
            render_frame_in_rect(&mut app, &ctx, movement, egui::Modifiers::NONE, rect);
            assert_ne!(
                app.fs_pan, before,
                "closed overflow cannot block a fresh canvas drag after the previous egui owner retires: touch={promoted_touch}"
            );
            let mut release = vec![button(fresh_end, egui::PointerButton::Primary, false)];
            if promoted_touch {
                release.insert(0, touch(fresh_end, egui::TouchPhase::End));
            }
            render_frame_in_rect(&mut app, &ctx, release, egui::Modifiers::NONE, rect);
        }
    }
}

#[test]
fn overflow_surface_matches_top_area_at_popup_edges_and_adjacent_floats() {
    let panel = egui::Rect::from_min_max(egui::pos2(100.0, 100.0), egui::pos2(500.0, 400.0));
    let popup_rects = [
        egui::Rect::from_min_max(egui::pos2(180.0, 140.0), egui::pos2(420.0, 320.0)),
        egui::Rect::from_min_max(egui::pos2(250.0, 120.0), egui::pos2(250.0, 350.0)),
        egui::Rect::from_min_max(egui::pos2(140.0, 250.0), egui::pos2(460.0, 250.0)),
        egui::Rect::from_min_max(egui::pos2(250.0, 250.0), egui::pos2(250.0, 250.0)),
        egui::Rect::from_min_max(egui::pos2(50.0, 180.0), egui::pos2(100.0, 250.0)),
        egui::Rect::from_min_max(egui::pos2(500.0, 180.0), egui::pos2(550.0, 250.0)),
    ];
    for popup_rect in popup_rects {
        let ctx = egui::Context::default();
        let overflow = overflow_area_layer();
        let popup = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("overflow_surface_boundary_popup"),
        );
        // Register actual egui Area geometry in z-order. Zero-sized Areas are
        // intentional: egui's inclusive hit-test also owns their line/point.
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(viewport()),
                ..Default::default()
            },
            |ui| {
                ui.ctx().memory_mut(|memory| {
                    for (layer, rect) in [(overflow, panel), (popup, popup_rect)] {
                        memory.areas_mut().set_state(
                            layer,
                            egui::AreaState {
                                pivot_pos: Some(rect.min),
                                size: Some(rect.size()),
                                interactable: true,
                                ..Default::default()
                            },
                        );
                    }
                });
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(ctx.layer_id_at(popup_rect.center()), Some(popup));
        let surface = fs_overflow_pointer_surface(&ctx, Some(panel));
        let axis_points = |values: [f32; 6]| {
            values
                .into_iter()
                .flat_map(|value| [value.next_down(), value, value.next_up()])
                .collect::<Vec<_>>()
        };
        let xs = axis_points([
            panel.left(),
            panel.right(),
            panel.center().x,
            popup_rect.left(),
            popup_rect.right(),
            popup_rect.center().x,
        ]);
        let ys = axis_points([
            panel.top(),
            panel.bottom(),
            panel.center().y,
            popup_rect.top(),
            popup_rect.bottom(),
            popup_rect.center().y,
        ]);
        for x in xs {
            for &y in &ys {
                let pos = egui::pos2(x, y);
                let top = ctx.layer_id_at(pos);
                let expected = panel.contains(pos) && top == Some(overflow);
                let actual = surface.iter().any(|rect| rect.contains(pos));
                assert_eq!(
                    actual, expected,
                    "button/touch surface differs from egui at {pos:?}: popup={popup_rect:?}, top={top:?}, surface={surface:?}"
                );
            }
        }
    }
}
