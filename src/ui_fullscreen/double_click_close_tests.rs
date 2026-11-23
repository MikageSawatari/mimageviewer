//! Release-level click policy regressions using real egui input adjudication.
use super::*;
use crate::settings::FullscreenPrimaryClickMode;

fn viewport() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0))
}

fn canvas_pos() -> egui::Pos2 {
    egui::pos2(300.0, 180.0)
}

fn button(pos: egui::Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    }
}

fn click(pos: egui::Pos2, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        button(pos, true, modifiers),
        button(pos, false, modifiers),
    ]
}

fn double_click(pos: egui::Pos2) -> Vec<egui::Event> {
    click(pos, egui::Modifiers::NONE)
        .into_iter()
        .chain(click(pos, egui::Modifiers::NONE))
        .collect()
}

#[derive(Clone, Copy, Default)]
struct Options {
    video: bool,
    continuous: bool,
    side_chrome: bool,
    seek_interactive: bool,
    suppressed: bool,
    navigator: Option<egui::Rect>,
    hud: Option<egui::Rect>,
}

#[derive(Debug, PartialEq)]
enum Outcome {
    None,
    Close,
    Reset,
    Dismiss,
    Page(i32),
    Video(usize),
    Depth(egui::Pos2),
}

impl From<FsCanvasClickAction> for Outcome {
    fn from(action: FsCanvasClickAction) -> Self {
        match action {
            FsCanvasClickAction::None => Self::None,
            FsCanvasClickAction::CloseViewer => Self::Close,
            FsCanvasClickAction::ResetTransform => Self::Reset,
            FsCanvasClickAction::DismissOverflow => Self::Dismiss,
            FsCanvasClickAction::PageSide(delta) => Self::Page(delta),
            FsCanvasClickAction::ToggleVideo(idx) => Self::Video(idx),
            FsCanvasClickAction::DepthFocusAt(pos) => Self::Depth(pos),
        }
    }
}

struct Canvas {
    ctx: egui::Context,
    app: crate::app::AppTestEnvForTest,
    time: f64,
}

impl Canvas {
    fn new() -> Self {
        let ctx = egui::Context::default();
        page_turn::install(&ctx);
        install_fs_navigator_input_tracking(&ctx);
        let mut app = super::page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_primary_click = FullscreenPrimaryClickMode::DoubleClickClose;
        app.settings.fullscreen_navigator_visible = false;
        app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
        app.fs_focus_regained_at = None;
        let mut canvas = Self {
            ctx,
            app,
            time: 0.0,
        };
        canvas.plan(
            vec![egui::Event::PointerMoved(canvas_pos())],
            Options::default(),
        );
        canvas
    }

    fn plan(&mut self, events: Vec<egui::Event>, options: Options) -> Outcome {
        self.time += 0.04;
        let mut outcome = Outcome::None;
        let ctx = self.ctx.clone();
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(viewport()),
                time: Some(self.time),
                focused: true,
                events,
                ..Default::default()
            },
            |ui| {
                if let Some(hud) = options.hud {
                    egui::Area::new(egui::Id::new("close-mode-test-hud"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(hud.min)
                        .default_size(hud.size())
                        .movable(false)
                        .show(&ctx, |ui| {
                            let _ = ui.allocate_exact_size(hud.size(), egui::Sense::click());
                        });
                }
                let (front, _) = self.app.capture_fs_front_pointer_input(&ctx, viewport());
                let response = ui.interact(
                    viewport(),
                    egui::Id::new("double-click-close-canvas"),
                    egui::Sense::click_and_drag(),
                );
                self.app
                    .fs_front_pointer
                    .accept_backing_click(&response, options.suppressed);
                let zoom = self.app.fs_zoom;
                let pan = self.app.fs_pan;
                outcome = self
                    .app
                    .plan_fs_canvas_click(
                        &ctx,
                        &response,
                        viewport(),
                        StillSeekGeometry::bar_only(),
                        options.video,
                        options.suppressed,
                        options.continuous,
                        options.side_chrome,
                        options.navigator,
                        options.seek_interactive,
                        zoom,
                        pan,
                        Some(&front),
                        options.suppressed,
                    )
                    .into();
            },
        )
        .drop_without_applying_deltas();
        outcome
    }

    fn merge(&mut self, events: Vec<egui::Event>) -> (bool, bool) {
        self.time += 0.04;
        let mut facts = (false, false);
        let ctx = self.ctx.clone();
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(viewport()),
                time: Some(self.time),
                focused: true,
                events,
                ..Default::default()
            },
            |ui| {
                let (result, _) = super::wheel_frame_tests::actual_merge(&mut self.app, ui, false);
                facts = (result.close, !result.page_nav.is_none());
            },
        )
        .drop_without_applying_deltas();
        facts
    }

    fn raw_click_counts(&self) -> Vec<u32> {
        self.ctx.input(|input| {
            input
                .pointer
                .released_buttons_with_click_counts()
                .filter_map(|(button, click)| {
                    (button == egui::PointerButton::Primary)
                        .then_some(click)
                        .flatten()
                })
                .map(|(_, count)| count)
                .collect()
        })
    }
}

#[test]
fn close_mode_canvas_single_click_stays_on_both_halves_and_double_click_closes() {
    for x in [150.0, 450.0] {
        let mut canvas = Canvas::new();
        let pos = egui::pos2(x, 180.0);
        assert_eq!(
            canvas.plan(click(pos, egui::Modifiers::NONE), Options::default()),
            Outcome::None
        );
        assert_eq!(canvas.app.fullscreen_idx, Some(0));
        assert_eq!(
            canvas.plan(click(pos, egui::Modifiers::NONE), Options::default()),
            Outcome::Close
        );
    }
}

#[test]
fn close_mode_actual_capture_and_handler_accept_two_clicks_in_one_pass() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let mut canvas = Canvas::new();
    canvas.merge(vec![egui::Event::PointerMoved(canvas_pos())]);
    assert_eq!(canvas.merge(double_click(canvas_pos())), (true, false));
    assert_eq!(canvas.raw_click_counts(), [1, 2]);
    assert_eq!(
        canvas.app.fullscreen_idx,
        Some(0),
        "the result requests the existing close owner"
    );
}

#[test]
fn close_mode_actual_handler_discards_simultaneous_wheel_navigation() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for deferred in [false, true] {
        let mut canvas = Canvas::new();
        canvas.merge(vec![egui::Event::PointerMoved(canvas_pos())]);
        let mut events = double_click(canvas_pos());
        let wheel = |modifiers| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -120.0),
            modifiers,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        };
        events.push(wheel(egui::Modifiers::NONE));
        if deferred {
            events.push(wheel(egui::Modifiers::ALT));
        }
        let ctx = canvas.ctx.clone();
        canvas.time += 0.04;
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(viewport()),
                time: Some(canvas.time),
                events,
                ..Default::default()
            },
            |ui| {
                let (mut result, mut operation) =
                    super::wheel_frame_tests::collect_actual_merge(&mut canvas.app, ui, false);
                assert_eq!(result.deferred.is_some(), deferred);
                if let Some(mut proposal) = result.deferred.take() {
                    canvas.app.claim_fs_wheel_frame_after_ui(&mut proposal);
                    result =
                        canvas
                            .app
                            .apply_fs_wheel_frame_proposal(proposal, false, &mut operation);
                }
                assert!(result.close && result.primary_click_close);
                assert_eq!(
                    canvas.app.fullscreen_idx,
                    Some(0),
                    "no intermediate wheel navigation before close"
                );
                canvas.app.handle_fs_navigation_with_operation(
                    &ctx,
                    result.close,
                    false,
                    None,
                    None,
                    None,
                    result.page_nav,
                    None,
                    0,
                    operation,
                );
                assert_eq!(canvas.app.fullscreen_idx, None);
                assert!(!canvas.app.commit_page_turn_continuation(&ctx));
            },
        )
        .drop_without_applying_deltas();
    }
}

#[test]
fn close_mode_actual_handler_dismisses_overflow_without_closing_on_batched_clicks() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let mut canvas = Canvas::new();
    canvas.merge(vec![egui::Event::PointerMoved(canvas_pos())]);
    canvas.app.fs_overflow_panel_state = FsOverflowPanelState::Root;
    let pos = egui::pos2(100.0, 180.0);
    assert_eq!(canvas.merge(double_click(pos)), (false, false));
    assert_eq!(
        canvas.app.fs_overflow_panel_state,
        FsOverflowPanelState::Closed
    );
    assert_eq!(canvas.raw_click_counts(), [1, 2]);
    canvas.time += 1.0;
    assert_eq!(canvas.merge(double_click(pos)), (true, false));
}

#[test]
fn close_mode_moved_canvas_resets_before_a_later_double_click_closes() {
    for continuous in [false, true] {
        let mut canvas = Canvas::new();
        let options = Options {
            continuous,
            ..Options::default()
        };
        canvas.app.fs_zoom = 2.0;
        assert_eq!(
            canvas.plan(double_click(canvas_pos()), options),
            Outcome::Reset
        );
        // Model the existing reset consumer; use a new egui click sequence.
        canvas.app.fs_zoom = 1.0;
        canvas.app.fs_pan = egui::Vec2::ZERO;
        canvas.app.fs_free_rotation = 0.0;
        canvas.time += 1.0;
        assert_eq!(
            canvas.plan(double_click(canvas_pos()), options),
            Outcome::Close
        );
    }
}

#[test]
fn close_mode_reset_keeps_priority_over_open_overflow() {
    let mut canvas = Canvas::new();
    canvas.app.fs_zoom = 2.0;
    canvas.app.fs_overflow_panel_state = FsOverflowPanelState::Root;
    assert_eq!(
        canvas.plan(double_click(egui::pos2(100.0, 180.0)), Options::default()),
        Outcome::Reset
    );
}

#[test]
fn close_mode_actual_handler_resets_moved_view_before_dismissing_overflow() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let mut canvas = Canvas::new();
    canvas.merge(vec![egui::Event::PointerMoved(canvas_pos())]);
    canvas.app.fs_zoom = 2.0;
    canvas.app.fs_pan = egui::vec2(30.0, 15.0);
    canvas.app.fs_overflow_panel_state = FsOverflowPanelState::Root;
    assert_eq!(
        canvas.merge(double_click(egui::pos2(100.0, 180.0))),
        (false, false)
    );
    assert_eq!(canvas.app.fs_zoom, 1.0);
    assert_eq!(canvas.app.fs_pan, egui::Vec2::ZERO);
    assert_eq!(
        canvas.app.fs_overflow_panel_state,
        FsOverflowPanelState::Root
    );
    assert_eq!(canvas.app.fullscreen_idx, Some(0));
}

#[test]
fn close_mode_dismiss_overflow_then_canvas_click_does_not_close() {
    let mut canvas = Canvas::new();
    let pos = egui::pos2(100.0, 180.0);
    canvas.app.fs_overflow_panel_state = FsOverflowPanelState::Root;
    assert_eq!(
        canvas.plan(click(pos, egui::Modifiers::NONE), Options::default()),
        Outcome::Dismiss
    );
    canvas.app.fs_overflow_panel_state = FsOverflowPanelState::Closed;
    assert_eq!(
        canvas.plan(click(pos, egui::Modifiers::NONE), Options::default()),
        Outcome::None
    );
    canvas.time += 1.0;
    assert_eq!(
        canvas.plan(double_click(pos), Options::default()),
        Outcome::Close
    );
}

#[test]
fn close_mode_popup_and_chrome_clicks_cannot_start_a_close_pair() {
    for blocked in ["popup", "side", "seek", "depth_panel", "navigator"] {
        let mut canvas = Canvas::new();
        let mut options = Options::default();
        let pos = match blocked {
            "popup" => {
                canvas.app.spread_popup_open = true;
                canvas_pos()
            }
            "side" => {
                options.side_chrome = true;
                let chrome = canvas.app.fs_side_panel_click_chrome(
                    &canvas.ctx,
                    viewport(),
                    StillSeekGeometry::bar_only(),
                    false,
                    true,
                    None,
                );
                chrome
                    .panels
                    .iter()
                    .next()
                    .expect("side summon strip")
                    .center()
            }
            "seek" => {
                options.seek_interactive = true;
                StillSeekGeometry::bar_only()
                    .panel_rect(viewport())
                    .center()
            }
            "navigator" => {
                options.navigator = Some(egui::Rect::from_center_size(
                    canvas_pos(),
                    egui::vec2(60.0, 60.0),
                ));
                canvas_pos()
            }
            "depth_panel" => {
                let mut depth = crate::depth_parallax::DepthViewState::new(
                    canvas.app.edit_request_owner_context(),
                );
                depth.panel_visible = true;
                canvas.app.fs_exclusive_view =
                    crate::fs_exclusive_view::FsExclusiveView::Depth(depth);
                let panel = egui::Rect::from_center_size(canvas_pos(), egui::vec2(60.0, 60.0));
                let id = canvas.app.depth_panel_rect_id();
                canvas.ctx.data_mut(|data| data.insert_temp(id, panel));
                panel.center()
            }
            _ => unreachable!(),
        };
        assert_eq!(
            canvas.plan(click(pos, egui::Modifiers::NONE), options),
            Outcome::None,
            "{blocked}"
        );
        canvas.app.spread_popup_open = false;
        if blocked == "depth_panel" {
            if let crate::fs_exclusive_view::FsExclusiveView::Depth(depth) =
                &mut canvas.app.fs_exclusive_view
            {
                depth.panel_visible = false;
            }
        }
        assert_eq!(
            canvas.plan(click(pos, egui::Modifiers::NONE), Options::default()),
            Outcome::None,
            "{blocked}"
        );
        canvas.time += 1.0;
        assert_eq!(
            canvas.plan(double_click(canvas_pos()), Options::default()),
            Outcome::Close,
            "{blocked}"
        );
    }
}

#[test]
fn close_mode_continuous_reading_closes_only_outside_popup_and_chrome() {
    for blocked in ["none", "popup", "side", "seek"] {
        let mut canvas = Canvas::new();
        let mut options = Options {
            continuous: true,
            ..Options::default()
        };
        let pos = match blocked {
            "popup" => {
                canvas.app.fit_popup_open = true;
                canvas_pos()
            }
            "side" => {
                options.side_chrome = true;
                canvas
                    .app
                    .fs_side_panel_click_chrome(
                        &canvas.ctx,
                        viewport(),
                        StillSeekGeometry::bar_only(),
                        false,
                        true,
                        None,
                    )
                    .panels
                    .iter()
                    .next()
                    .unwrap()
                    .center()
            }
            "seek" => {
                options.seek_interactive = true;
                StillSeekGeometry::bar_only()
                    .panel_rect(viewport())
                    .center()
            }
            _ => canvas_pos(),
        };
        assert_eq!(
            canvas.plan(double_click(pos), options),
            if blocked == "none" {
                Outcome::Close
            } else {
                Outcome::None
            },
            "{blocked}"
        );
    }
}

#[test]
fn close_mode_depth_uses_release_modifiers_and_preserves_ctrl_focus() {
    for batched in [false, true] {
        let mut canvas = Canvas::new();
        canvas.app.fs_exclusive_view = crate::fs_exclusive_view::FsExclusiveView::Depth(
            crate::depth_parallax::DepthViewState::new(canvas.app.edit_request_owner_context()),
        );
        let pos = canvas_pos();
        if batched {
            let mut events = click(pos, egui::Modifiers::CTRL);
            events.extend(click(pos, egui::Modifiers::NONE));
            events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
            assert_eq!(canvas.plan(events, Options::default()), Outcome::None);
        } else {
            assert_eq!(
                canvas.plan(click(pos, egui::Modifiers::CTRL), Options::default()),
                Outcome::Depth(pos)
            );
            assert_eq!(
                canvas.plan(click(pos, egui::Modifiers::NONE), Options::default()),
                Outcome::None
            );
        }
        canvas.time += 1.0;
        assert_eq!(
            canvas.plan(double_click(pos), Options::default()),
            Outcome::Close
        );
    }
}

#[test]
fn close_mode_batched_navigator_position_then_canvas_position_does_not_close() {
    let mut canvas = Canvas::new();
    let navigator_pos = canvas_pos();
    let blank_pos = navigator_pos + egui::vec2(3.0, 0.0);
    let navigator = egui::Rect::from_center_size(navigator_pos, egui::vec2(2.0, 20.0));
    let mut events = click(navigator_pos, egui::Modifiers::NONE);
    events.extend(click(blank_pos, egui::Modifiers::NONE));
    assert_eq!(
        canvas.plan(
            events,
            Options {
                navigator: Some(navigator),
                ..Options::default()
            }
        ),
        Outcome::None
    );
    assert_eq!(canvas.raw_click_counts(), [1, 2]);
}

#[test]
fn close_mode_actual_navigator_handler_owns_its_release_in_a_mixed_pass() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    page_turn::install(&ctx);
    install_fs_navigator_input_tracking(&ctx);
    let (mut app, _, idx) = super::tests::setup_flat_navigator_input_test();
    app.settings.fullscreen_primary_click = FullscreenPrimaryClickMode::DoubleClickClose;
    app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
    app.settings.touch_still_chrome_learned = true;
    app.fs_zoom = 1.0;
    app.fs_focus_regained_at = None;
    crate::app::source_texture_test_support::seed(&mut app, &ctx, idx);
    let full = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
    let media =
        app.fullscreen_media_rect_with_geometry(full, idx, false, StillSeekGeometry::bar_only());
    let layout = app
        .fs_navigator_layout(media)
        .expect("real navigator fixture");
    let navigator_pos = layout.canvas_rect.left_center() + egui::vec2(1.0, 0.0);
    let blank_pos = layout.canvas_rect.left_center() - egui::vec2(2.0, 0.0);
    assert!(!layout.panel_rect.contains(blank_pos));
    let mut mixed = click(navigator_pos, egui::Modifiers::NONE);
    mixed.extend(click(blank_pos, egui::Modifiers::NONE));
    for (time, events, expected_close) in [
        (0.0, vec![egui::Event::PointerMoved(blank_pos)], false),
        (0.04, mixed, false),
        (0.08, click(blank_pos, egui::Modifiers::NONE), false),
        (1.0, double_click(blank_pos), true),
    ] {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(full),
                time: Some(time),
                focused: true,
                events,
                ..Default::default()
            },
            |ui| {
                let (result, _) = super::wheel_frame_tests::actual_merge(&mut app, ui, false);
                assert_eq!(result.close, expected_close, "time={time}");
                assert!(result.page_nav.is_none());
                assert_eq!(app.fullscreen_idx, Some(idx));
                if time == 0.04 {
                    assert_eq!(
                        ctx.input(|input| input
                            .pointer
                            .released_buttons_with_click_counts()
                            .filter_map(|(_, click)| click.map(|(_, count)| count))
                            .collect::<Vec<_>>()),
                        [1, 2]
                    );
                    assert_eq!(
                        app.fs_front_pointer.clicks.current_accepted, 1,
                        "the first release reached the navigator owner"
                    );
                    assert!(!app.fs_front_pointer.clicks.previous_close_candidate);
                    assert!(!app.fullscreen_navigator_interaction.is_active());
                    // Keep the third click on the canvas without resetting the click boundary.
                    app.settings.fullscreen_navigator_visible = false;
                }
            },
        )
        .drop_without_applying_deltas();
    }
}

#[test]
fn close_mode_capture_and_planner_exclude_each_hud_layer_release_in_a_mixed_pass() {
    let mut canvas = Canvas::new();
    let hud_pos = canvas_pos();
    let blank_pos = hud_pos + egui::vec2(3.0, 0.0);
    let hud = egui::Rect::from_center_size(hud_pos, egui::vec2(2.0, 20.0));
    let options = Options {
        hud: Some(hud),
        ..Options::default()
    };
    for _ in 0..2 {
        canvas.plan(vec![], options);
    }
    let hud_layer = egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("close-mode-test-hud"),
    );
    assert_eq!(canvas.ctx.layer_id_at(hud_pos), Some(hud_layer));
    assert_ne!(canvas.ctx.layer_id_at(blank_pos), Some(hud_layer));
    let mut events = click(hud_pos, egui::Modifiers::NONE);
    events.extend(click(blank_pos, egui::Modifiers::NONE));
    assert_eq!(canvas.plan(events, options), Outcome::None);
    assert_eq!(canvas.raw_click_counts(), [1, 2]);
    canvas.time += 1.0;
    assert_eq!(
        canvas.plan(double_click(blank_pos), options),
        Outcome::Close
    );
}

#[test]
fn close_mode_egui_time_and_distance_restart_the_candidate_sequence() {
    for separate_by_time in [false, true] {
        let mut canvas = Canvas::new();
        assert_eq!(
            canvas.plan(
                click(canvas_pos(), egui::Modifiers::NONE),
                Options::default()
            ),
            Outcome::None
        );
        let pos = if separate_by_time {
            canvas.time += 1.0;
            canvas_pos()
        } else {
            canvas_pos() + egui::vec2(100.0, 0.0)
        };
        assert_eq!(
            canvas.plan(click(pos, egui::Modifiers::NONE), Options::default()),
            Outcome::None
        );
        assert_eq!(
            canvas.raw_click_counts(),
            [1],
            "egui must start a fresh sequence"
        );
        assert_eq!(
            canvas.plan(click(pos, egui::Modifiers::NONE), Options::default()),
            Outcome::Close
        );
    }
}

#[test]
fn close_mode_focus_loss_between_batched_releases_restarts_the_sequence() {
    let mut canvas = Canvas::new();
    let mut events = click(canvas_pos(), egui::Modifiers::NONE);
    events.extend([
        egui::Event::WindowFocused(false),
        egui::Event::WindowFocused(true),
    ]);
    events.extend(click(canvas_pos(), egui::Modifiers::NONE));
    assert_eq!(canvas.plan(events, Options::default()), Outcome::None);
    canvas.time += 1.0;
    assert_eq!(
        canvas.plan(double_click(canvas_pos()), Options::default()),
        Outcome::Close
    );
}

#[test]
fn close_mode_suppressed_release_cannot_start_the_next_close_pair() {
    let mut canvas = Canvas::new();
    assert_eq!(
        canvas.plan(
            click(canvas_pos(), egui::Modifiers::NONE),
            Options {
                suppressed: true,
                ..Options::default()
            }
        ),
        Outcome::None
    );
    assert_eq!(
        canvas.plan(
            click(canvas_pos(), egui::Modifiers::NONE),
            Options::default()
        ),
        Outcome::None
    );
}

#[test]
fn close_mode_actual_handler_suppressed_stream_cannot_start_a_close_pair() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let mut canvas = Canvas::new();
    canvas.merge(vec![egui::Event::PointerMoved(canvas_pos())]);
    canvas.app.fs_primary_suppression.arm_pointer_stream();
    assert_eq!(
        canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)),
        (false, false)
    );
    assert_eq!(
        canvas.app.fs_primary_suppression,
        FullscreenPrimarySuppression::Idle
    );
    assert_eq!(
        canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)),
        (false, false)
    );
    canvas.time += 1.0;
    assert_eq!(canvas.merge(double_click(canvas_pos())), (true, false));
}

#[test]
fn close_mode_list_open_double_click_is_capped_at_one_canvas_release() {
    let mut canvas = Canvas::new();
    // egui saw the list click, while the new canvas owner did not accept it.
    canvas.plan(
        click(canvas_pos(), egui::Modifiers::NONE),
        Options::default(),
    );
    canvas.app.fs_front_pointer = FsFrontPointerOwnership::default();
    assert_eq!(
        canvas.plan(
            click(canvas_pos(), egui::Modifiers::NONE),
            Options::default()
        ),
        Outcome::None
    );
}

#[test]
fn close_mode_mode_switch_cannot_reuse_a_page_turn_candidate() {
    let mut canvas = Canvas::new();
    canvas.app.settings.fullscreen_primary_click = FullscreenPrimaryClickMode::PageTurn;
    assert!(matches!(
        canvas.plan(
            click(canvas_pos(), egui::Modifiers::NONE),
            Options::default()
        ),
        Outcome::Page(_)
    ));
    canvas.app.settings.fullscreen_primary_click = FullscreenPrimaryClickMode::DoubleClickClose;
    assert_eq!(
        canvas.plan(
            click(canvas_pos(), egui::Modifiers::NONE),
            Options::default()
        ),
        Outcome::None
    );
    canvas.time += 1.0;
    assert_eq!(
        canvas.plan(double_click(canvas_pos()), Options::default()),
        Outcome::Close
    );
}

#[test]
fn close_mode_switching_away_retires_an_already_accepted_close_candidate() {
    let mut canvas = Canvas::new();
    canvas.plan(
        click(canvas_pos(), egui::Modifiers::NONE),
        Options::default(),
    );
    assert!(canvas.app.fs_front_pointer.clicks.previous_close_candidate);
    canvas.time += 1.0;
    canvas.app.settings.fullscreen_primary_click = FullscreenPrimaryClickMode::PageTurn;
    assert!(matches!(
        canvas.plan(
            click(canvas_pos(), egui::Modifiers::NONE),
            Options::default()
        ),
        Outcome::Page(_)
    ));
    assert!(!canvas.app.fs_front_pointer.clicks.previous_close_candidate);
    canvas.app.settings.fullscreen_primary_click = FullscreenPrimaryClickMode::DoubleClickClose;
    assert_eq!(
        canvas.plan(
            click(canvas_pos(), egui::Modifiers::NONE),
            Options::default()
        ),
        Outcome::None
    );
}

#[test]
fn close_mode_excluded_edit_analysis_panorama_and_video_keep_released_actions() {
    for excluded in [
        "erase",
        "local_adjust",
        "conceal",
        "crop",
        "text",
        "analysis",
        "panorama",
        "video",
    ] {
        let mut canvas = Canvas::new();
        let mut options = Options::default();
        match excluded {
            "erase" => canvas.app.erase_mode = true,
            "local_adjust" => canvas.app.local_adjust_mode = true,
            "conceal" => canvas.app.conceal_mode = true,
            "crop" => canvas.app.export_crop_mode = true,
            "text" => canvas.app.text_mode = true,
            "analysis" => canvas.app.analysis_mode = true,
            "panorama" => {
                if let Some(FsCacheEntry::Static { source_dims, .. }) =
                    canvas.app.fs_cache.get_mut(&0)
                {
                    *source_dims = Some([2000, 1000]);
                }
                canvas.app.fs_exclusive_view = crate::fs_exclusive_view::FsExclusiveView::Panorama(
                    crate::panorama::PanoramaState::new(
                        0.0,
                        0.0,
                        crate::panorama::PanoProjection::Perspective,
                    ),
                );
                assert!(canvas.app.is_panorama_mode_active(0));
            }
            "video" => options.video = true,
            _ => unreachable!(),
        }
        assert_eq!(
            canvas.plan(double_click(canvas_pos()), options),
            if excluded == "video" {
                Outcome::Video(0)
            } else {
                Outcome::None
            },
            "{excluded}"
        );
    }
}

#[test]
fn close_mode_default_page_turn_still_acts_on_each_separate_release() {
    let mut canvas = Canvas::new();
    canvas.app.settings.fullscreen_primary_click = FullscreenPrimaryClickMode::PageTurn;
    for _ in 0..2 {
        assert!(matches!(
            canvas.plan(
                click(canvas_pos(), egui::Modifiers::NONE),
                Options::default()
            ),
            Outcome::Page(_)
        ));
    }
}

fn fold_frame(
    ctx: &egui::Context,
    owner: &mut FsFrontPointerOwnership,
    time: f64,
    events: Vec<egui::Event>,
    front_rects: &[egui::Rect],
    candidate: impl Fn(egui::Pos2, egui::Modifiers) -> bool,
) -> FsCloseModeClicks {
    let mut facts = FsCloseModeClicks::default();
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(viewport()),
            time: Some(time),
            focused: true,
            events,
            ..Default::default()
        },
        |ui| {
            let front = owner.capture(ctx, false, front_rects);
            let response = ui.interact(
                viewport(),
                egui::Id::new("close-fold-canvas"),
                egui::Sense::click_and_drag(),
            );
            facts = owner.accept_close_mode_clicks(ctx, &response, &front, false, &candidate);
        },
    )
    .drop_without_applying_deltas();
    facts
}

#[test]
fn close_mode_capture_preserves_each_front_owned_release_in_a_mixed_pass() {
    for front_first in [false, true] {
        let ctx = egui::Context::default();
        let mut owner = FsFrontPointerOwnership::default();
        let front_pos = egui::pos2(100.0, 180.0);
        let canvas_pos = front_pos + egui::vec2(3.0, 0.0);
        let front_rect = egui::Rect::from_center_size(front_pos, egui::vec2(2.0, 20.0));
        fold_frame(&ctx, &mut owner, 0.0, vec![], &[front_rect], |_, _| true);
        let (first, second) = if front_first {
            (front_pos, canvas_pos)
        } else {
            (canvas_pos, front_pos)
        };
        let mut events = click(first, egui::Modifiers::NONE);
        events.extend(click(second, egui::Modifiers::NONE));
        let facts = fold_frame(&ctx, &mut owner, 0.04, events, &[front_rect], |_, _| true);
        assert!(!facts.close_double_clicked, "front_first={front_first}");
        assert_eq!(facts.last_click.map(|(pos, _)| pos), Some(canvas_pos));
        assert_eq!(owner.clicks.previous_close_candidate, front_first);
        let facts = fold_frame(
            &ctx,
            &mut owner,
            1.0,
            double_click(canvas_pos),
            &[front_rect],
            |_, _| true,
        );
        assert!(facts.close_double_clicked);
    }
}

#[test]
fn close_mode_capture_classifies_each_candidate_in_a_batched_sequence() {
    let ctx = egui::Context::default();
    let mut owner = FsFrontPointerOwnership::default();
    let used_pos = egui::pos2(100.0, 180.0);
    let blank_pos = used_pos + egui::vec2(3.0, 0.0);
    fold_frame(&ctx, &mut owner, 0.0, vec![], &[], |_, _| true);
    let mut events = click(used_pos, egui::Modifiers::NONE);
    events.extend(click(blank_pos, egui::Modifiers::NONE));
    let facts = fold_frame(&ctx, &mut owner, 0.04, events, &[], |pos, _| {
        pos == blank_pos
    });
    assert!(
        facts.double_clicked,
        "both releases belong to the backing canvas"
    );
    assert!(
        !facts.close_double_clicked,
        "a noncandidate release cannot form the close pair"
    );
    let facts = fold_frame(
        &ctx,
        &mut owner,
        1.0,
        double_click(blank_pos),
        &[],
        |pos, _| pos == blank_pos,
    );
    assert!(facts.close_double_clicked);
}

#[test]
fn close_mode_navigator_release_retires_the_previous_close_candidate() {
    let ctx = egui::Context::default();
    let mut owner = FsFrontPointerOwnership::default();
    fold_frame(&ctx, &mut owner, 0.0, vec![], &[], |_, _| true);
    // Route the first real adjudicated release to the existing navigator owner.
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(viewport()),
            time: Some(0.04),
            events: click(canvas_pos(), egui::Modifiers::NONE),
            ..Default::default()
        },
        |ui| {
            owner.capture(&ctx, false, &[]);
            let click = ctx.input(|input| {
                input
                    .pointer
                    .released_buttons_with_click_counts()
                    .find_map(|(button, click)| {
                        (button == egui::PointerButton::Primary)
                            .then_some(click)
                            .flatten()
                    })
            });
            let accepted = owner.accept_navigator_click(&ctx, click);
            assert_eq!(accepted, Some((canvas_pos(), 1)));
            let _ = ui.interact(
                viewport(),
                egui::Id::new("close-fold-canvas"),
                egui::Sense::click_and_drag(),
            );
        },
    )
    .drop_without_applying_deltas();
    let facts = fold_frame(
        &ctx,
        &mut owner,
        0.08,
        click(canvas_pos(), egui::Modifiers::NONE),
        &[],
        |_, _| true,
    );
    assert!(facts.double_clicked);
    assert!(!facts.close_double_clicked);
}

#[test]
fn close_mode_focus_loss_after_a_release_clears_the_persistent_candidate() {
    let ctx = egui::Context::default();
    let mut owner = FsFrontPointerOwnership::default();
    fold_frame(&ctx, &mut owner, 0.0, vec![], &[], |_, _| true);
    let mut events = click(canvas_pos(), egui::Modifiers::NONE);
    events.push(egui::Event::WindowFocused(false));
    let facts = fold_frame(&ctx, &mut owner, 0.04, events, &[], |_, _| true);
    assert!(
        facts.last_click.is_some(),
        "capture retains the release preceding focus loss"
    );
    assert!(!owner.clicks.previous_close_candidate);
    let mut events = vec![egui::Event::WindowFocused(true)];
    events.extend(click(canvas_pos(), egui::Modifiers::NONE));
    let facts = fold_frame(&ctx, &mut owner, 0.08, events, &[], |_, _| true);
    assert!(!facts.close_double_clicked);
}
