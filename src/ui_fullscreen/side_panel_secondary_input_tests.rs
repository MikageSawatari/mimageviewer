//! Backlog §1.840 (1): side panels own egui clicks, but not right drags.
use super::*;
use crate::ring_shortcut::{
    MouseFlickOutcome, MouseGestureBinding, MouseGestureDirection, RightDragCommand,
    RightDragContext, RightDragMode, RingActionId, ViewerShortRightClickAction,
};

pub(crate) fn viewport() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 800.0))
}

fn modes() -> [RightDragMode; 3] {
    [
        RightDragMode::RingShortcut,
        RightDragMode::MouseGesture,
        RightDragMode::Disabled,
    ]
}

pub(crate) fn ready(mode: RightDragMode) -> crate::app::AppTestEnvForTest {
    let mut app = crate::app::setup_app_for_test();
    app.items = vec![GridItem::Image(std::path::PathBuf::from(
        "C:/section1840.png",
    ))];
    app.fullscreen_idx = Some(0);
    app.settings.fullscreen_navigator_visible = false;
    app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
    app.fs_info_panel.open = crate::ui_helpers::MetadataPanelOpenState::ByPointer;
    app.settings
        .ring_shortcuts
        .set_right_drag_mode(RightDragContext::ImageFullscreen, mode);
    app.settings
        .ring_shortcuts
        .set_viewer_short_right_click_action(
            RightDragContext::ImageFullscreen,
            ViewerShortRightClickAction::CloseFullscreen,
        );
    assert!(app.metadata_panel_click_shown());
    app
}

pub(crate) fn right(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Secondary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[derive(Default, Debug)]
pub(crate) struct FrameFacts {
    pub(crate) short_tap_at: Option<egui::Pos2>,
    pub(crate) egui_click_at: Option<egui::Pos2>,
    pub(crate) command: Option<RightDragCommand>,
    pub(crate) close_requested: bool,
    pub(crate) continued: bool,
    pub(crate) long_press: bool,
}

/// Run the production front-input capture -> fullscreen recognizer -> application
/// path, followed by panel widgets in the same order as the fullscreen renderer.
/// Do not fabricate FsSecondaryAction: the three real recognizers must classify it.
pub(crate) fn frame(
    app: &mut App,
    ctx: &egui::Context,
    time: f64,
    events: Vec<egui::Event>,
    mut draw_panel: impl FnMut(&mut egui::Ui),
) -> (FrameFacts, egui::FullOutput) {
    let mut facts = FrameFacts::default();
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(viewport()),
            time: Some(time),
            focused: true,
            events,
            ..Default::default()
        },
        |ui| {
            let ctx = ui.ctx().clone();
            facts.egui_click_at = ctx.input(|i| {
                i.pointer
                    .button_click_position(egui::PointerButton::Secondary)
            });
            let (front, _) = app.capture_fs_front_pointer_input(&ctx, viewport());
            let action = app.recognize_fs_secondary_input(
                &ctx,
                viewport(),
                false,
                true,
                StillSeekGeometry::bar_only().panel_rect(viewport()),
                true,
                0,
                front,
            );
            match &action {
                FsSecondaryAction::ShortTap { pos, .. } => facts.short_tap_at = Some(*pos),
                FsSecondaryAction::Recognized {
                    pos, recognition, ..
                } => {
                    if recognition.outcome == MouseFlickOutcome::ShortTap
                        && recognition.command.is_none()
                    {
                        facts.short_tap_at = Some(*pos);
                    }
                    facts.command = recognition.command.clone();
                }
                FsSecondaryAction::LongPress(_) => facts.long_press = true,
                FsSecondaryAction::None => {}
            }
            let chrome = app.fs_side_panel_click_chrome(
                &ctx,
                viewport(),
                StillSeekGeometry::bar_only(),
                false,
                true,
                None,
            );
            let outcome = app.apply_fs_secondary_input(&ctx, action, chrome);
            facts.close_requested = matches!(outcome, FsSecondaryInputOutcome::CloseRequested);
            facts.continued = matches!(outcome, FsSecondaryInputOutcome::Continue);
            if facts.continued {
                draw_panel(ui);
            }
        },
    );
    (facts, output)
}

fn input_frame(
    app: &mut App,
    ctx: &egui::Context,
    time: f64,
    events: Vec<egui::Event>,
) -> FrameFacts {
    let (facts, output) = frame(app, ctx, time, events, |_| {});
    output.drop_without_applying_deltas();
    facts
}

fn press(app: &mut App, ctx: &egui::Context, pos: egui::Pos2) {
    let facts = input_frame(
        app,
        ctx,
        0.0,
        vec![egui::Event::PointerMoved(pos), right(pos, true)],
    );
    assert!(facts.continued && facts.short_tap_at.is_none(), "{facts:?}");
}

fn assert_panel_tap(mode: RightDragMode) {
    let ctx = egui::Context::default();
    let mut app = ready(mode);
    let pos = metadata_panel_rect(viewport()).center();
    press(&mut app, &ctx, pos);
    let facts = input_frame(&mut app, &ctx, 0.02, vec![right(pos, false)]);
    assert_eq!(facts.short_tap_at, Some(pos), "{facts:?}");
    assert_eq!(facts.egui_click_at, Some(pos), "{facts:?}");
    assert!(
        !facts.close_requested,
        "panel click must not close fullscreen: {facts:?}"
    );
    assert!(facts.continued);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert!(app.fs_context_menu_idx.is_none());
}

#[test]
fn section1840_metadata_panel_short_tap_stays_open_ring() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    assert_panel_tap(RightDragMode::RingShortcut);
}

#[test]
fn section1840_metadata_panel_short_tap_stays_open_gesture() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    assert_panel_tap(RightDragMode::MouseGesture);
}

#[test]
fn section1840_metadata_panel_short_tap_stays_open_disabled() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    assert_panel_tap(RightDragMode::Disabled);
}

#[test]
fn section1840_short_tap_outside_visible_panel_still_closes() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for mode in modes() {
        let ctx = egui::Context::default();
        let mut app = ready(mode.clone());
        let pos = viewport().center();
        assert!(!metadata_panel_rect(viewport()).contains(pos));
        press(&mut app, &ctx, pos);
        let facts = input_frame(&mut app, &ctx, 0.02, vec![right(pos, false)]);
        assert_eq!(facts.short_tap_at, Some(pos), "{mode:?}: {facts:?}");
        assert_eq!(facts.egui_click_at, Some(pos));
        assert!(facts.close_requested, "{mode:?}: {facts:?}");
    }
}

#[test]
fn section1840_drag_starting_on_panel_executes_ring_and_gesture_commands() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for mode in [RightDragMode::RingShortcut, RightDragMode::MouseGesture] {
        let ctx = egui::Context::default();
        let mut app = ready(mode.clone());
        // A visible, reversible effect without disk writes or page navigation.
        app.fs_pixel_grid_enabled = false;
        app.settings
            .ring_shortcuts
            .image
            .slots
            .fill(RingActionId::ImagePixelGrid);
        app.settings.ring_shortcuts.mouse_gestures_image.bindings = vec![MouseGestureBinding::new(
            vec![MouseGestureDirection::Left],
            RingActionId::ImagePixelGrid,
        )];
        let start = metadata_panel_rect(viewport()).center();
        let end = start - egui::vec2(100.0, 0.0);
        press(&mut app, &ctx, start);
        match mode {
            RightDragMode::RingShortcut => assert!(app.mouse_ring_flick.is_some()),
            RightDragMode::MouseGesture => assert!(app.mouse_gesture.is_some()),
            _ => unreachable!(),
        }
        input_frame(&mut app, &ctx, 0.01, vec![egui::Event::PointerMoved(end)]);
        let facts = input_frame(&mut app, &ctx, 0.02, vec![right(end, false)]);
        assert!(facts.command.is_some(), "{mode:?}: {facts:?}");
        assert!(facts.short_tap_at.is_none() && facts.egui_click_at.is_none());
        assert!(
            facts.continued && !facts.close_requested,
            "{mode:?}: {facts:?}"
        );
        assert!(
            app.fs_pixel_grid_enabled,
            "{mode:?}: recognized command must execute"
        );
        assert_eq!(app.fullscreen_idx, Some(0));
    }
}

#[test]
fn section1840_panel_motion_beyond_egui_click_distance_keeps_short_tap_action() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for mode in modes() {
        let ctx = egui::Context::default();
        ctx.options_mut(|o| o.input_options.max_click_dist = 6.0);
        let mut app = ready(mode.clone());
        let start = metadata_panel_rect(viewport()).center();
        let end = start - egui::vec2(12.0, 0.0);
        assert!(metadata_panel_rect(viewport()).contains(end));
        press(&mut app, &ctx, start);
        input_frame(&mut app, &ctx, 0.01, vec![egui::Event::PointerMoved(end)]);
        let facts = input_frame(&mut app, &ctx, 0.02, vec![right(end, false)]);
        assert_eq!(facts.short_tap_at, Some(end), "{mode:?}: {facts:?}");
        assert!(facts.egui_click_at.is_none(), "egui must reject this click");
        assert!(
            facts.close_requested,
            "no dead zone between egui and ring tolerances: {mode:?}"
        );
    }
}

#[test]
fn section1840_panel_hold_beyond_egui_click_duration_keeps_short_tap_action() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for mode in modes() {
        let ctx = egui::Context::default();
        // egui uses RawInput.time; viewer recognizers use Instant. Lower egui's
        // limit to exercise disagreement deterministically without sleeping.
        ctx.options_mut(|o| o.input_options.max_click_duration = 0.005);
        let mut app = ready(mode.clone());
        let pos = metadata_panel_rect(viewport()).center();
        press(&mut app, &ctx, pos);
        let facts = input_frame(&mut app, &ctx, 0.02, vec![right(pos, false)]);
        assert_eq!(facts.short_tap_at, Some(pos), "{mode:?}: {facts:?}");
        assert!(
            facts.egui_click_at.is_none(),
            "egui must reject this held click"
        );
        assert!(facts.close_requested, "{mode:?}: {facts:?}");
    }
}

#[test]
fn section1840_short_tap_uses_release_position_even_with_later_motion() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for mode in modes() {
        for release_inside in [false, true] {
            let ctx = egui::Context::default();
            let mut app = ready(mode.clone());
            let panel = metadata_panel_rect(viewport());
            let inside = egui::pos2(panel.left() + 2.0, panel.center().y);
            let outside = egui::pos2(panel.left() - 2.0, panel.center().y);
            let (start, end) = if release_inside {
                (outside, inside)
            } else {
                (inside, outside)
            };
            press(&mut app, &ctx, start);
            let facts = input_frame(
                &mut app,
                &ctx,
                0.02,
                vec![
                    egui::Event::PointerMoved(end),
                    right(end, false),
                    egui::Event::PointerMoved(start),
                ],
            );
            assert_eq!(facts.short_tap_at, Some(end), "{mode:?}: {facts:?}");
            assert_eq!(facts.egui_click_at, Some(end));
            assert_eq!(
                facts.close_requested, !release_inside,
                "{mode:?}: {facts:?}"
            );
        }
    }
}

#[test]
fn section1840_disabled_panel_long_press_keeps_fullscreen_context_menu() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = ready(RightDragMode::Disabled);
    let pos = metadata_panel_rect(viewport()).center();
    press(&mut app, &ctx, pos);
    let FullscreenSecondaryPress::Armed { started_at, .. } = &mut app.fs_secondary_press else {
        panic!("disabled mode must retain its press");
    };
    *started_at = std::time::Instant::now()
        - crate::ring_shortcut::mouse_flick_menu_delay()
        - std::time::Duration::from_millis(1);
    let facts = input_frame(&mut app, &ctx, 0.02, vec![right(pos, false)]);
    assert!(
        facts.long_press && facts.continued && !facts.close_requested,
        "{facts:?}"
    );
    assert_eq!(app.fs_context_menu_idx, Some(0));
    assert_eq!(app.fs_context_menu_pos, pos);
}

#[test]
fn section1840_batched_taps_use_the_matching_egui_release_click() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for mode in modes() {
        for final_is_click in [false, true] {
            let ctx = egui::Context::default();
            ctx.options_mut(|o| o.input_options.max_click_dist = 6.0);
            let mut app = ready(mode.clone());
            let pos = metadata_panel_rect(viewport()).center();
            let moved = pos - egui::vec2(12.0, 0.0);
            press(&mut app, &ctx, pos);
            let mut events = Vec::new();
            if final_is_click {
                events.extend([
                    egui::Event::PointerMoved(moved),
                    egui::Event::PointerMoved(pos),
                ]);
            }
            events.extend([right(pos, false), right(pos, true)]);
            if !final_is_click {
                events.extend([
                    egui::Event::PointerMoved(moved),
                    egui::Event::PointerMoved(pos),
                ]);
            }
            events.push(right(pos, false));
            let facts = input_frame(&mut app, &ctx, 0.02, events);
            assert_eq!(facts.short_tap_at, Some(pos), "{mode:?}: {facts:?}");
            // There is an egui click at this same coordinate either way. Only
            // the release that produced the final short tap may own its action.
            assert_eq!(facts.egui_click_at, Some(pos));
            assert_eq!(
                facts.close_requested, !final_is_click,
                "{mode:?}, final_is_click={final_is_click}: {facts:?}"
            );
        }
    }
}

#[test]
fn section1840_panel_short_tap_does_not_open_viewer_context_menu() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for mode in modes() {
        let ctx = egui::Context::default();
        let mut app = ready(mode.clone());
        app.settings
            .ring_shortcuts
            .set_viewer_short_right_click_action(
                RightDragContext::ImageFullscreen,
                ViewerShortRightClickAction::ContextMenu,
            );
        let pos = metadata_panel_rect(viewport()).center();
        press(&mut app, &ctx, pos);
        let facts = input_frame(&mut app, &ctx, 0.02, vec![right(pos, false)]);
        assert_eq!(facts.short_tap_at, Some(pos));
        assert_eq!(facts.egui_click_at, Some(pos));
        assert!(
            facts.continued && !facts.close_requested,
            "{mode:?}: {facts:?}"
        );
        assert!(
            app.fs_context_menu_idx.is_none(),
            "panel must own the menu: {mode:?}"
        );
    }
}

fn lock_only_metadata_panel(app: &mut App) {
    app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::Hover;
    app.settings.fullscreen_chrome_suppression.info = false;
    app.fs_info_panel.open = crate::ui_helpers::MetadataPanelOpenState::Closed;
    app.fs_info_panel.hover_active = false;
    app.fs_info_panel.locked = true;
    assert!(!app.metadata_panel_click_shown());
    assert!(!app.fullscreen_tag_picker_open);
    assert!(app.still_info_panel_lock_effective_for_idx(0, false));
}

#[test]
fn section1840_lock_only_metadata_panel_short_tap_stays_open() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for mode in modes() {
        let ctx = egui::Context::default();
        let mut app = ready(mode.clone());
        lock_only_metadata_panel(&mut app);
        // Inside the drawn panel, away from the right-edge summon strip.
        let pos = metadata_panel_rect(viewport()).center();
        press(&mut app, &ctx, pos);
        let facts = input_frame(&mut app, &ctx, 0.02, vec![right(pos, false)]);
        assert_eq!(facts.short_tap_at, Some(pos), "{mode:?}: {facts:?}");
        assert_eq!(facts.egui_click_at, Some(pos));
        assert!(
            facts.continued && !facts.close_requested,
            "{mode:?}: {facts:?}"
        );
        assert!(!app.fs_info_panel.hover_active);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(app.fs_context_menu_idx.is_none());
    }
}

#[test]
fn section1840_lock_only_metadata_panel_left_click_does_not_turn_page() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for locked in [false, true] {
        let ctx = egui::Context::default();
        let mut app = ready(RightDragMode::Disabled);
        lock_only_metadata_panel(&mut app);
        app.fs_info_panel.locked = locked;
        let pos = metadata_panel_rect(viewport()).center();
        let mut action = FsCanvasClickAction::None;
        for pass in 0..3 {
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if pass > 0 {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: pass == 1,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(viewport()),
                    time: Some(pass as f64 * 0.02),
                    focused: true,
                    events,
                    ..Default::default()
                },
                |ui| {
                    app.capture_fs_front_pointer_input(&ctx, viewport());
                    let response = ui.interact(
                        viewport(),
                        egui::Id::unique("section1840_lock_only_canvas"),
                        egui::Sense::click_and_drag(),
                    );
                    app.fs_front_pointer.accept_backing_click(&response, false);
                    if pass == 2 {
                        assert!(
                            response.clicked(),
                            "the real canvas click must reach its handler"
                        );
                    }
                    action = app.plan_fs_canvas_click(
                        &ctx,
                        &response,
                        viewport(),
                        StillSeekGeometry::bar_only(),
                        false,
                        false,
                        false,
                        true,
                        None,
                        false,
                        1.0,
                        egui::Vec2::ZERO,
                        None,
                        false,
                    );
                },
            )
            .drop_without_applying_deltas();
        }
        assert!(!app.fs_info_panel.hover_active);
        assert!(
            if locked {
                matches!(action, FsCanvasClickAction::None)
            } else {
                matches!(action, FsCanvasClickAction::PageSide(_))
            },
            "only the lock makes this panel visible: locked={locked}"
        );
    }
}
