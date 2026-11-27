//! Release-level click policy regressions using real egui input adjudication.
use super::*;
use crate::settings::FullscreenPrimaryClickMode;

mod close_900_tests;
mod scrub_907_tests;
mod stall_919_tests;
mod suppression_906_tests;
mod timestamp_919_tests;

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
    focus_restore: bool,
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
        let mut canvas = Self::without_pointer_pass();
        canvas.plan(
            vec![egui::Event::PointerMoved(canvas_pos())],
            Options::default(),
        );
        canvas
    }

    /// Let a child viewport own the first pointer pass instead of priming ROOT.
    fn without_pointer_pass() -> Self {
        let ctx = egui::Context::default();
        page_turn::install(&ctx);
        install_fs_navigator_input_tracking(&ctx);
        let mut app = super::page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_primary_click = FullscreenPrimaryClickMode::DoubleClickClose;
        app.settings.fullscreen_navigator_visible = false;
        app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
        app.fs_focus_regained_at = None;
        Self {
            ctx,
            app,
            time: 0.0,
        }
    }

    fn plan(&mut self, events: Vec<egui::Event>, options: Options) -> Outcome {
        self.time += 0.04;
        let mut outcome = Outcome::None;
        let ctx = self.ctx.clone();
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(viewport()),
                time: Some(self.time),
                input_clock: Some(self.time),
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
                self.app.fs_front_pointer.accept_backing_click(
                    &response,
                    options.suppressed,
                    &front,
                );
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
                        if options.suppressed {
                            FullscreenPrimarySuppressionDecision::Suppress(
                                FullscreenPrimarySuppressionReason::Other,
                            )
                        } else if options.focus_restore {
                            FullscreenPrimarySuppressionDecision::Suppress(
                                FullscreenPrimarySuppressionReason::FocusRestore,
                            )
                        } else {
                            FullscreenPrimarySuppressionDecision::Allow
                        },
                        false,
                    )
                    .into();
            },
        )
        .drop_without_applying_deltas();
        outcome
    }

    fn merge(&mut self, events: Vec<egui::Event>) -> (bool, bool) {
        self.merge_using(events, true, None, |app, ui| {
            let (result, _) = super::wheel_frame_tests::actual_merge(app, ui, false);
            (result.close, !result.page_nav.is_none())
        })
    }

    fn redraw_canvas_after_release(&mut self) {
        // F2 returns before registering fs_click in PageTurn mode. Real UI
        // frames redraw it before the next physical click; egui hit-tests the
        // previous pass's widgets. Do not synthesize another release here:
        // the old stranded PointerStream must remain detectable by this test.
        assert_eq!(
            self.merge_using(vec![], true, None, |app, ui| {
                assert!(ui.ctx().input(|i| {
                    !i.pointer.primary_pressed()
                        && !i.pointer.primary_released()
                        && !i.pointer.primary_down()
                }));
                let (result, _) = super::wheel_frame_tests::actual_merge(app, ui, false);
                (result.close, !result.page_nav.is_none())
            }),
            (false, false)
        );
        assert_eq!(
            self.app.fs_primary_suppression,
            FullscreenPrimarySuppression::Idle
        );
    }

    #[cfg(windows)]
    fn for_focus_restore() -> Self {
        let mut canvas = Self::new();
        // Establish the real handler's fs_click widget before pointer input,
        // including PageTurn's Response-based click recognition.
        assert_eq!(
            canvas.merge(vec![egui::Event::PointerMoved(canvas_pos())]),
            (false, false)
        );
        canvas
    }

    fn merge_using(
        &mut self,
        events: Vec<egui::Event>,
        focused: bool,
        hud: Option<egui::Rect>,
        run: impl FnMut(&mut App, &mut egui::Ui) -> (bool, bool),
    ) -> (bool, bool) {
        self.merge_using_on(egui::ViewportId::ROOT, events, focused, hud, run)
    }

    fn merge_using_on(
        &mut self,
        viewport_id: egui::ViewportId,
        events: Vec<egui::Event>,
        focused: bool,
        hud: Option<egui::Rect>,
        mut run: impl FnMut(&mut App, &mut egui::Ui) -> (bool, bool),
    ) -> (bool, bool) {
        self.time += 0.04;
        let mut facts = (false, false);
        let ctx = self.ctx.clone();
        let mut input = egui::RawInput {
            viewport_id,
            screen_rect: Some(viewport()),
            time: Some(self.time),
            input_clock: Some(self.time),
            focused,
            events,
            ..Default::default()
        };
        input.viewports.insert(
            viewport_id,
            egui::ViewportInfo {
                parent: (viewport_id != egui::ViewportId::ROOT).then_some(egui::ViewportId::ROOT),
                inner_rect: Some(viewport()),
                focused: Some(focused),
                ..Default::default()
            },
        );
        ctx.run_ui(input, |ui| {
            if let Some(hud) = hud {
                egui::Area::new(egui::Id::new("restore-close-test-hud"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(hud.min)
                    .default_size(hud.size())
                    .movable(false)
                    .show(&ctx, |ui| {
                        let _ = ui.allocate_exact_size(hud.size(), egui::Sense::click());
                    });
            }
            facts = run(&mut self.app, ui);
        })
        .drop_without_applying_deltas();
        facts
    }

    fn split_click(&mut self, modifiers: egui::Modifiers) -> (bool, bool) {
        self.split_click_at(canvas_pos(), modifiers)
    }

    fn split_click_at(&mut self, pos: egui::Pos2, modifiers: egui::Modifiers) -> (bool, bool) {
        assert_eq!(
            self.merge(vec![
                egui::Event::PointerMoved(pos),
                button(pos, true, modifiers),
            ]),
            (false, false)
        );
        self.merge(vec![button(pos, false, modifiers)])
    }

    #[cfg(windows)]
    fn merge_window(
        &mut self,
        context: crate::app::ViewerContextId,
        window: u64,
        events: Vec<egui::Event>,
        finish: bool,
    ) -> (bool, bool) {
        self.merge_using(events, true, None, |app, ui| {
            app.with_window_context_for_test(context, |owner| {
                owner.begin_active_detached_session(window, crate::app::DetachedSource::Image);
                let (result, operation) = super::wheel_frame_tests::actual_merge(owner, ui, false);
                if finish {
                    let idx = owner.fullscreen_idx.unwrap();
                    owner.handle_fs_navigation_with_operation(
                        ui.ctx(),
                        result.close,
                        false,
                        None,
                        None,
                        None,
                        result.page_nav,
                        None,
                        idx,
                        operation,
                    );
                    assert!(owner.fullscreen_idx.is_none());
                }
                (result.close, !result.page_nav.is_none())
            })
        })
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
fn close_mode_transformed_depth_preserves_ctrl_focus_and_resets_before_closing() {
    for (zoom, rotation) in [(2.0, 0.0), (1.0, 0.25)] {
        let mut canvas = Canvas::new();
        canvas.app.fs_exclusive_view = crate::fs_exclusive_view::FsExclusiveView::Depth(
            crate::depth_parallax::DepthViewState::new(canvas.app.edit_request_owner_context()),
        );
        canvas.app.fs_zoom = zoom;
        canvas.app.fs_free_rotation = rotation;
        let pos = canvas_pos();
        let mut events = click(pos, egui::Modifiers::CTRL);
        events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        assert_eq!(canvas.plan(events, Options::default()), Outcome::Depth(pos));
        canvas.time += 1.0;
        assert_eq!(
            canvas.plan(click(pos, egui::Modifiers::NONE), Options::default()),
            Outcome::None
        );
        canvas.time += 1.0;
        assert_eq!(
            canvas.plan(double_click(pos), Options::default()),
            Outcome::Reset
        );
        // Model the existing reset consumer; use a new egui click sequence.
        canvas.app.fs_zoom = 1.0;
        canvas.app.fs_pan = egui::Vec2::ZERO;
        canvas.app.fs_free_rotation = 0.0;
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
    canvas.app.fs_primary_suppression = FullscreenPrimarySuppression::pointer_stream_for_test(
        FullscreenPrimarySuppressionReason::Other,
    );
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
        "audio",
        "video_audio",
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
            "video" | "video_audio" => {
                canvas.app.items[0] = GridItem::Video(PathBuf::from("c:/471/excluded.mp4"));
                options.video = true;
                if excluded == "video_audio" {
                    canvas.app.video_audio_mode = Some(0);
                    assert!(canvas.app.fs_music_view_active(0));
                }
            }
            "audio" => {
                canvas.app.items[0] = GridItem::Audio(PathBuf::from("c:/471/excluded.flac"));
                assert!(canvas.app.fs_music_view_active(0));
            }
            _ => unreachable!(),
        }
        assert_eq!(
            canvas.plan(double_click(canvas_pos()), options),
            match excluded {
                "video" | "video_audio" => Outcome::Video(0),
                // This is the existing planner route for an Audio item; its
                // music surface owns the visible playback widgets separately.
                "audio" => Outcome::Page(1),
                _ => Outcome::None,
            },
            "{excluded}"
        );
    }
}

#[test]
fn close_mode_audio_and_video_audio_handler_match_page_turn_policy() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for video_audio in [false, true] {
        let mut baseline = None;
        for mode in [
            FullscreenPrimaryClickMode::PageTurn,
            FullscreenPrimaryClickMode::DoubleClickClose,
        ] {
            let mut canvas = Canvas::new();
            let path = PathBuf::from(if video_audio {
                "c:/471/music.mp4"
            } else {
                "c:/471/music.flac"
            });
            canvas.app.items[0] = if video_audio {
                GridItem::Video(path.clone())
            } else {
                GridItem::Audio(path.clone())
            };
            if video_audio {
                canvas.app.video_audio_mode = Some(0);
            }
            canvas.app.fs_cache.insert(
                0,
                FsCacheEntry::Video {
                    player: Box::new(
                        crate::video::VideoPlayer::stream_ready_disconnected_for_test(path),
                    ),
                    load_seq: 1,
                },
            );
            canvas.app.settings.fullscreen_primary_click = mode;
            assert!(canvas.app.fs_music_view_active(0));
            canvas.merge(vec![egui::Event::PointerMoved(canvas_pos())]);
            let mut outcomes = Vec::new();
            for _ in 0..2 {
                outcomes.push(canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)));
                assert_eq!(canvas.app.fullscreen_idx, Some(0));
                assert!(!outcomes.last().unwrap().0);
                assert!(!canvas.app.fs_front_pointer.clicks.previous_close_candidate);
            }
            if let Some(expected) = &baseline {
                assert_eq!(&outcomes, expected);
            } else {
                baseline = Some(outcomes);
            }
        }
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

#[test]
fn page_turn_batched_double_and_triple_click_reset_the_moved_canvas() {
    for clicks in [2, 3] {
        let mut canvas = Canvas::new();
        canvas.app.settings.fullscreen_primary_click = FullscreenPrimaryClickMode::PageTurn;
        canvas.app.fs_zoom = 2.0;
        let events = (0..clicks)
            .flat_map(|_| click(canvas_pos(), egui::Modifiers::NONE))
            .collect();
        assert_eq!(canvas.plan(events, Options::default()), Outcome::Reset);
        assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, clicks);
    }
}

#[test]
fn page_turn_list_open_double_click_still_counts_only_the_canvas_release() {
    let mut canvas = Canvas::new();
    canvas.app.settings.fullscreen_primary_click = FullscreenPrimaryClickMode::PageTurn;
    canvas.app.fs_zoom = 2.0;
    assert_eq!(
        canvas.plan(
            click(canvas_pos(), egui::Modifiers::NONE),
            Options::default()
        ),
        Outcome::None
    );
    canvas.app.fs_front_pointer = FsFrontPointerOwnership::default();
    assert_eq!(
        canvas.plan(
            click(canvas_pos(), egui::Modifiers::NONE),
            Options::default()
        ),
        Outcome::None
    );
    assert_eq!(canvas.raw_click_counts(), [2]);
    assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, 1);
}

/// Exercise the normal canvas acceptance boundary without native windows or App workers.
fn fold_backing_frame(
    ctx: &egui::Context,
    owner: &mut FsFrontPointerOwnership,
    time: f64,
    events: Vec<egui::Event>,
    front_rects: &[egui::Rect],
) -> bool {
    let mut double = false;
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
            // Supply egui's admitted response to test this acceptance boundary;
            // the production caller separately applies aggregate front suppression.
            let response = ui.interact(
                viewport(),
                egui::Id::unique("page-turn-fold-canvas"),
                egui::Sense::click_and_drag(),
            );
            owner.accept_backing_click(&response, false, &front);
            let accepted = owner.clicks.current_accepted;
            // Replanning the same owning pass must not count its releases again.
            owner.accept_backing_click(&response, false, &front);
            assert_eq!(owner.clicks.current_accepted, accepted);
            double = owner.backing_double_clicked(&response);
        },
    )
    .drop_without_applying_deltas();
    double
}

#[test]
fn page_turn_batched_canvas_click_acceptance_is_idempotent() {
    let ctx = egui::Context::default();
    let mut owner = FsFrontPointerOwnership::default();
    fold_backing_frame(&ctx, &mut owner, 0.0, vec![], &[]);
    assert!(fold_backing_frame(
        &ctx,
        &mut owner,
        0.04,
        double_click(canvas_pos()),
        &[]
    ));
    assert_eq!(owner.clicks.current_accepted, 2);
    assert!(!fold_backing_frame(
        &ctx,
        &mut owner,
        0.08,
        click(canvas_pos(), egui::Modifiers::NONE),
        &[]
    ));
    assert_eq!(
        owner.clicks.current_accepted, 3,
        "PageTurn keeps the saturated third-click policy"
    );
}

#[test]
fn page_turn_batched_front_and_canvas_clicks_cannot_form_a_double_click() {
    for front_first in [false, true] {
        let ctx = egui::Context::default();
        let mut owner = FsFrontPointerOwnership::default();
        let front_pos = egui::pos2(100.0, 180.0);
        let canvas_pos = front_pos + egui::vec2(3.0, 0.0);
        let front_rect = egui::Rect::from_center_size(front_pos, egui::vec2(2.0, 20.0));
        fold_backing_frame(&ctx, &mut owner, 0.0, vec![], &[front_rect]);
        let (first, second) = if front_first {
            (front_pos, canvas_pos)
        } else {
            (canvas_pos, front_pos)
        };
        let mut events = click(first, egui::Modifiers::NONE);
        events.extend(click(second, egui::Modifiers::NONE));
        assert!(
            !fold_backing_frame(&ctx, &mut owner, 0.04, events, &[front_rect]),
            "front_first={front_first}"
        );
        assert_eq!(owner.clicks.current_accepted, u32::from(front_first));
        assert!(fold_backing_frame(
            &ctx,
            &mut owner,
            1.0,
            double_click(canvas_pos),
            &[front_rect]
        ));
    }
}

#[test]
fn page_turn_batched_clicks_on_both_sides_of_focus_loss_cannot_form_a_double_click() {
    let ctx = egui::Context::default();
    let mut owner = FsFrontPointerOwnership::default();
    fold_backing_frame(&ctx, &mut owner, 0.0, vec![], &[]);
    let mut events = click(canvas_pos(), egui::Modifiers::NONE);
    events.extend([
        egui::Event::WindowFocused(false),
        egui::Event::WindowFocused(true),
    ]);
    events.extend(click(canvas_pos(), egui::Modifiers::NONE));
    assert!(!fold_backing_frame(&ctx, &mut owner, 0.04, events, &[]));
    assert_eq!(owner.clicks.current_accepted, 1);
    assert!(fold_backing_frame(
        &ctx,
        &mut owner,
        1.0,
        double_click(canvas_pos()),
        &[]
    ));
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
            facts = owner.accept_close_mode_clicks(
                ctx,
                &response,
                &front,
                FullscreenPrimarySuppressionDecision::Allow,
                &candidate,
            );
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

// §1.899: these use the production capture, suppression and merge handlers.
// F1 split frames and both F2 deliveries fail on the pre-fix implementation.
#[cfg(windows)]
#[test]
fn close_mode_focus_restore_899_f1_split_frames_close_on_second_click() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = Canvas::for_focus_restore();
    canvas.app.fs_focus_regained_at = Some(std::time::Instant::now());
    assert_eq!(canvas.split_click(egui::Modifiers::NONE), (false, false));
    assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, 1);
    assert!(canvas.app.fs_front_pointer.clicks.previous_close_candidate);
    assert_eq!(
        canvas.app.fs_primary_suppression,
        FullscreenPrimarySuppression::Idle
    );
    assert!(canvas.app.fs_canvas_drag.pan_start().is_none());
    assert_eq!(canvas.app.fullscreen_idx, Some(0));
    assert_eq!(canvas.split_click(egui::Modifiers::NONE), (true, false));
}

#[cfg(windows)]
#[test]
fn close_mode_focus_restore_899_f1_expired_and_same_frame_controls() {
    let _input = crate::key_input::lock_test_input();
    for split in [false, true] {
        let mut canvas = Canvas::for_focus_restore();
        canvas.app.fs_focus_regained_at = Some(if split {
            std::time::Instant::now() - std::time::Duration::from_secs(1)
        } else {
            std::time::Instant::now()
        });
        let first = if split {
            assert_eq!(
                canvas.merge(vec![button(canvas_pos(), true, egui::Modifiers::NONE)]),
                (false, false)
            );
            assert!(canvas.app.fs_focus_regained_at.is_none());
            assert_eq!(
                canvas.app.fs_primary_suppression,
                FullscreenPrimarySuppression::Idle
            );
            canvas.merge(vec![button(canvas_pos(), false, egui::Modifiers::NONE)])
        } else {
            canvas.merge(click(canvas_pos(), egui::Modifiers::NONE))
        };
        assert_eq!(first, (false, false));
        assert_eq!(
            canvas.app.fs_primary_suppression,
            FullscreenPrimarySuppression::Idle
        );
        assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, 1);
        assert_eq!(
            canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)),
            (true, false)
        );
    }
}

#[cfg(windows)]
#[test]
fn close_mode_focus_restore_899_f2_split_and_same_frames_close_on_second_click() {
    let _input = crate::key_input::lock_test_input();
    for split in [false, true] {
        let mut canvas = Canvas::for_focus_restore();
        canvas.app.settings.vst3_enabled = true;
        canvas.app.settings.vst3_gui_visible = true;
        let first = if split {
            canvas.split_click(egui::Modifiers::NONE)
        } else {
            canvas.merge(click(canvas_pos(), egui::Modifiers::NONE))
        };
        assert_eq!(first, (false, false));
        assert_eq!(canvas.app.fullscreen_idx, Some(0));
        assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, 1);
        assert!(canvas.app.fs_front_pointer.clicks.previous_close_candidate);
        let second = if split {
            canvas.split_click(egui::Modifiers::NONE)
        } else {
            canvas.merge(click(canvas_pos(), egui::Modifiers::NONE))
        };
        assert_eq!(second, (true, false), "split={split}");
    }
}

#[cfg(windows)]
#[test]
fn close_mode_focus_restore_899_f2_then_normal_and_two_releases_in_one_pass() {
    let _input = crate::key_input::lock_test_input();
    for same_pass in [false, true] {
        let mut canvas = Canvas::for_focus_restore();
        canvas.app.settings.vst3_enabled = true;
        canvas.app.settings.vst3_gui_visible = true;
        if same_pass {
            assert_eq!(canvas.merge(double_click(canvas_pos())), (true, false));
            assert_eq!(canvas.raw_click_counts(), [1, 2]);
        } else {
            assert_eq!(
                canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)),
                (false, false)
            );
            canvas.app.settings.vst3_gui_visible = false;
            assert_eq!(canvas.split_click(egui::Modifiers::NONE), (true, false));
        }
    }
    // F1 also folds a restoring release followed by a whole click in one pass.
    let mut canvas = Canvas::for_focus_restore();
    canvas.app.fs_focus_regained_at = Some(std::time::Instant::now());
    assert_eq!(
        canvas.merge(vec![button(canvas_pos(), true, egui::Modifiers::NONE)]),
        (false, false),
    );
    let mut events = vec![button(canvas_pos(), false, egui::Modifiers::NONE)];
    events.extend(click(canvas_pos(), egui::Modifiers::NONE));
    assert_eq!(canvas.merge(events), (true, false));
    assert_eq!(canvas.raw_click_counts(), [1, 2]);
}

#[cfg(windows)]
#[test]
fn close_mode_focus_restore_899_other_cancellation_and_terminal_priority() {
    let _input = crate::key_input::lock_test_input();
    for terminal_f2 in [false, true] {
        let mut canvas = Canvas::for_focus_restore();
        canvas.app.settings.ring_shortcuts.set_right_drag_mode(
            crate::ring_shortcut::RightDragContext::ImageFullscreen,
            crate::ring_shortcut::RightDragMode::RingShortcut,
        );
        let mut secondary = button(canvas_pos(), true, egui::Modifiers::NONE);
        if let egui::Event::PointerButton { button, .. } = &mut secondary {
            *button = egui::PointerButton::Secondary;
        }
        assert_eq!(canvas.merge(vec![secondary]), (false, false));
        assert!(canvas.app.mouse_ring_flick.is_some());
        if terminal_f2 {
            canvas.app.settings.vst3_enabled = true;
            canvas.app.settings.vst3_gui_visible = true;
        } else {
            canvas.app.fs_focus_regained_at = Some(std::time::Instant::now());
        }
        assert_eq!(
            canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)),
            (false, false)
        );
        assert!(canvas.app.mouse_ring_flick.is_none());
        assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, 0);
        assert!(!canvas.app.fs_front_pointer.clicks.previous_close_candidate);
        // The next release may be focus-only; it still cannot pair with Other.
        assert_eq!(
            canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)),
            (false, false)
        );
    }
    let mut canvas = Canvas::for_focus_restore();
    canvas.app.fs_focus_regained_at = Some(std::time::Instant::now());
    assert_eq!(
        canvas.merge(vec![button(canvas_pos(), true, egui::Modifiers::NONE)]),
        (false, false),
    );
    assert_eq!(
        canvas.app.fs_primary_suppression,
        FullscreenPrimarySuppression::PointerStream(
            FullscreenPrimarySuppressionReason::FocusRestore
        ),
    );
    // Another producer takes over the in-flight focus stream before its release.
    canvas.app.fs_primary_suppression = FullscreenPrimarySuppression::pointer_stream_for_test(
        FullscreenPrimarySuppressionReason::Other,
    );
    assert_eq!(
        canvas.merge(vec![button(canvas_pos(), false, egui::Modifiers::NONE)]),
        (false, false),
    );
    assert_eq!(
        canvas.app.fs_primary_suppression,
        FullscreenPrimarySuppression::Idle
    );
    assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, 0);
    assert_eq!(
        canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)),
        (false, false)
    );
}

#[cfg(windows)]
#[test]
fn close_mode_focus_restore_899_f2_modal_cannot_close_or_reset() {
    let _input = crate::key_input::lock_test_input();
    for zoom in [1.0, 2.0] {
        let mut canvas = Canvas::for_focus_restore();
        canvas.app.settings.vst3_enabled = true;
        canvas.app.settings.vst3_gui_visible = true;
        canvas.app.show_context_shortcuts_help = true;
        canvas.app.fs_zoom = zoom;
        let pan = if zoom == 1.0 {
            egui::Vec2::ZERO
        } else {
            egui::vec2(12.0, 8.0)
        };
        canvas.app.fs_pan = pan;
        assert!(canvas.app.any_modal_dialog_open_for_fullscreen_keys());
        assert_eq!(canvas.merge(double_click(canvas_pos())), (false, false));
        assert_eq!(canvas.app.fs_zoom, zoom);
        assert_eq!(canvas.app.fs_pan, pan);
        assert_eq!(canvas.app.fullscreen_idx, Some(0));
        assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, 0);
        assert!(!canvas.app.fs_front_pointer.clicks.previous_close_candidate);
    }
}

#[cfg(windows)]
#[test]
fn close_mode_focus_restore_899_no_single_click_use_and_reset_keeps_priority() {
    let _input = crate::key_input::lock_test_input();
    for f2 in [false, true] {
        let mut canvas = Canvas::for_focus_restore();
        canvas.app.fs_overflow_panel_state = FsOverflowPanelState::Root;
        if f2 {
            canvas.app.settings.vst3_enabled = true;
            canvas.app.settings.vst3_gui_visible = true;
        } else {
            canvas.app.fs_focus_regained_at = Some(std::time::Instant::now());
        }
        assert_eq!(
            canvas.split_click_at(egui::pos2(100.0, 180.0), egui::Modifiers::NONE),
            (false, false),
        );
        assert_eq!(
            canvas.app.fs_overflow_panel_state,
            FsOverflowPanelState::Root
        );
        assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, 1);
        assert!(!canvas.app.fs_front_pointer.clicks.previous_close_candidate);
        // App fixtures hold the non-reentrant data-dir test lock until drop.
        drop(canvas);

        let mut canvas = Canvas::for_focus_restore();
        canvas.app.fs_zoom = 2.0;
        canvas.app.fs_pan = egui::vec2(30.0, 15.0);
        canvas.app.fs_free_rotation = 0.2;
        if f2 {
            canvas.app.settings.vst3_enabled = true;
            canvas.app.settings.vst3_gui_visible = true;
        } else {
            canvas.app.fs_focus_regained_at = Some(std::time::Instant::now());
        }
        assert_eq!(canvas.split_click(egui::Modifiers::NONE), (false, false));
        assert_eq!(
            canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)),
            (false, false)
        );
        assert_eq!(canvas.app.fs_zoom, 1.0);
        assert_eq!(canvas.app.fs_pan, egui::Vec2::ZERO);
        assert_eq!(canvas.app.fs_free_rotation, 0.0);
        assert!(canvas.app.fs_automatic_initial_view.is_none());
        assert_eq!(canvas.app.fullscreen_idx, Some(0));
        // §12.7: end egui's triple-click sequence before the closing pair.
        canvas.time += 1.0;
        assert_eq!(canvas.merge(double_click(canvas_pos())), (true, false));
    }
    // Planner observes last_click directly: Ctrl focus must not be produced.
    let mut canvas = Canvas::for_focus_restore();
    canvas.app.fs_exclusive_view = crate::fs_exclusive_view::FsExclusiveView::Depth(
        crate::depth_parallax::DepthViewState::new(canvas.app.edit_request_owner_context()),
    );
    assert_eq!(
        canvas.plan(
            click(canvas_pos(), egui::Modifiers::CTRL),
            Options {
                focus_restore: true,
                ..Default::default()
            }
        ),
        Outcome::None
    );
    assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, 1);
    assert!(!canvas.app.fs_front_pointer.clicks.previous_close_candidate);
    canvas.time += 1.0;
    assert_eq!(
        canvas.plan(
            click(canvas_pos(), egui::Modifiers::CTRL),
            Options::default()
        ),
        Outcome::Depth(canvas_pos()),
    );
}

#[cfg(windows)]
#[test]
fn close_mode_focus_restore_899_front_layers_and_top_bar_do_not_seed_pair() {
    let _input = crate::key_input::lock_test_input();
    for f2 in [false, true] {
        for front in ["layer", "top_bar", "menu"] {
            let mut canvas = Canvas::for_focus_restore();
            let pos = if front == "top_bar" {
                egui::pos2(300.0, 12.0)
            } else {
                canvas_pos()
            };
            let hud = (front == "layer")
                .then(|| egui::Rect::from_center_size(pos, egui::vec2(40.0, 40.0)));
            if front == "menu" {
                canvas.app.fs_context_menu_idx = Some(0);
            }
            let run = |app: &mut App, ui: &mut egui::Ui| {
                let (result, _) = super::wheel_frame_tests::actual_merge(app, ui, false);
                (result.close, !result.page_nav.is_none())
            };
            // egui resolves foreground areas from their established layer geometry.
            canvas.merge_using(vec![egui::Event::PointerMoved(pos)], true, hud, run);
            if f2 {
                canvas.app.settings.vst3_enabled = true;
                canvas.app.settings.vst3_gui_visible = true;
            } else {
                canvas.app.fs_focus_regained_at = Some(std::time::Instant::now());
            }
            assert_eq!(
                canvas.merge_using(
                    vec![button(pos, true, egui::Modifiers::NONE)],
                    true,
                    hud,
                    run
                ),
                (false, false),
            );
            assert_eq!(
                canvas.merge_using(
                    vec![button(pos, false, egui::Modifiers::NONE)],
                    true,
                    hud,
                    run
                ),
                (false, false),
            );
            assert!(!canvas.app.fs_front_pointer.clicks.previous_close_candidate);
            assert_eq!(
                canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)),
                (false, false)
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn close_mode_focus_restore_899_list_open_carryover_remains_capped() {
    let _input = crate::key_input::lock_test_input();
    for f2 in [false, true] {
        let mut canvas = Canvas::for_focus_restore();
        canvas.merge(click(canvas_pos(), egui::Modifiers::NONE));
        // Same egui sequence, new canvas owner after opening from the list.
        canvas.app.fs_front_pointer = FsFrontPointerOwnership::default();
        if f2 {
            canvas.app.settings.vst3_enabled = true;
            canvas.app.settings.vst3_gui_visible = true;
        } else {
            canvas.app.fs_focus_regained_at = Some(std::time::Instant::now());
        }
        assert_eq!(canvas.split_click(egui::Modifiers::NONE), (false, false));
        assert_eq!(canvas.raw_click_counts(), [2]);
        assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, 1);
    }
}

#[cfg(windows)]
#[test]
fn close_mode_focus_restore_899_page_turn_still_suppresses_restoring_click() {
    let _input = crate::key_input::lock_test_input();
    for f2 in [false, true] {
        for split in [false, true] {
            let mut canvas = Canvas::for_focus_restore();
            canvas.app.settings.fullscreen_primary_click = FullscreenPrimaryClickMode::PageTurn;
            if f2 {
                canvas.app.settings.vst3_enabled = true;
                canvas.app.settings.vst3_gui_visible = true;
            } else {
                canvas.app.fs_focus_regained_at = Some(std::time::Instant::now());
            }
            let first = if split {
                canvas.split_click(egui::Modifiers::NONE)
            } else {
                canvas.merge(click(canvas_pos(), egui::Modifiers::NONE))
            };
            // F1 has never suppressed a press+release delivered in one frame.
            assert_eq!(first, (false, !f2 && !split));
            if f2 || split {
                assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, 0);
            }
            canvas.app.settings.vst3_gui_visible = false;
            canvas.app.fs_focus_regained_at = None;
            if f2 {
                canvas.redraw_canvas_after_release();
            }
            assert_eq!(
                canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)),
                (false, true)
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn close_mode_focus_restore_899_focus_loss_event_and_level_retire_candidate() {
    let _input = crate::key_input::lock_test_input();
    for event in [false, true] {
        let mut canvas = Canvas::for_focus_restore();
        canvas.app.fs_focus_regained_at = Some(std::time::Instant::now());
        assert_eq!(canvas.split_click(egui::Modifiers::NONE), (false, false));
        assert!(canvas.app.fs_front_pointer.clicks.previous_close_candidate);
        let events = if event {
            vec![egui::Event::WindowFocused(false)]
        } else {
            vec![]
        };
        canvas.merge_using(events, false, None, |app, ui| {
            let (result, _) = super::wheel_frame_tests::actual_merge(app, ui, false);
            (result.close, !result.page_nav.is_none())
        });
        assert!(!canvas.app.fs_front_pointer.clicks.previous_close_candidate);
        assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, 0);
        canvas.app.fs_focus_regained_at = Some(std::time::Instant::now());
        assert_eq!(
            canvas.merge(vec![egui::Event::WindowFocused(true)]),
            (false, false)
        );
        assert_eq!(canvas.split_click(egui::Modifiers::NONE), (false, false));
    }
}

#[test]
fn close_mode_focus_restore_899_reason_priority_and_terminal_idle() {
    use FullscreenPrimarySuppressionDecision::{Allow, Preserve, Suppress};
    use FullscreenPrimarySuppressionReason::{FocusRestore, Other};
    let mut suppression = FullscreenPrimarySuppression::Idle;
    assert_eq!(suppression.arm(true, FocusRestore), Suppress(FocusRestore));
    assert!(suppression.is_active());
    let _ = suppression.arm(true, Other);
    assert_eq!(suppression.arm(true, FocusRestore), Suppress(Other));
    let terminal =
        suppression.apply(FullscreenPrimarySuppressionEvent::PointerFrame { released: true });
    assert_eq!(suppression, FullscreenPrimarySuppression::Idle);
    assert!(!suppression.is_active());
    assert_eq!(terminal, Suppress(Other));
    assert_eq!(
        terminal.overridden_by(suppression.arm(true, FocusRestore)),
        Suppress(Other),
    );
    assert_eq!(
        Suppress(FocusRestore).overridden_by(Suppress(Other)),
        Suppress(Other)
    );
    assert_eq!(Suppress(Other).overridden_by(Preserve), Suppress(Other));
    assert_eq!(
        Suppress(Other).overridden_by(Allow),
        Allow,
        "touch Allow remains authoritative"
    );
    assert!(Suppress(FocusRestore).suppresses_current_frame());
    assert!(Suppress(Other).suppresses_current_frame());
    assert_eq!(
        suppression.apply(FullscreenPrimarySuppressionEvent::PointerFrame { released: true }),
        Suppress(FocusRestore),
    );
    assert_eq!(suppression, FullscreenPrimarySuppression::Idle);
    let _ = suppression.apply(FullscreenPrimarySuppressionEvent::TouchFrame {
        suppresses_current_frame: true,
        active: true,
        completed: false,
        cancelled: false,
    });
    assert_eq!(suppression.arm(true, FocusRestore), Suppress(Other));
}

#[cfg(windows)]
#[test]
fn close_mode_focus_restore_899_navigator_owns_restoring_release() {
    let _input = crate::key_input::lock_test_input();
    for f2 in [false, true] {
        // Build the navigator fixture as the only live App: App fixtures hold
        // the non-reentrant data-dir test lock until drop.
        let ctx = egui::Context::default();
        page_turn::install(&ctx);
        install_fs_navigator_input_tracking(&ctx);
        let (app, _, idx) = super::tests::setup_flat_navigator_input_test();
        let mut canvas = Canvas {
            ctx,
            app,
            time: 0.0,
        };
        canvas.app.fs_focus_regained_at = None;
        canvas.app.settings.fullscreen_primary_click = FullscreenPrimaryClickMode::DoubleClickClose;
        canvas.app.settings.fullscreen_side_panel_mode =
            crate::settings::FsSidePanelMode::ClickToShow;
        canvas.app.settings.touch_still_chrome_learned = true;
        canvas.app.fs_zoom = 1.0;
        crate::app::source_texture_test_support::seed(&mut canvas.app, &canvas.ctx, idx);
        let media = canvas.app.fullscreen_media_rect_with_geometry(
            viewport(),
            idx,
            false,
            StillSeekGeometry::bar_only(),
        );
        let layout = canvas
            .app
            .fs_navigator_layout(media)
            .expect("navigator fixture");
        let pos = layout.canvas_rect.left_center() + egui::vec2(1.0, 0.0);
        let blank = layout.canvas_rect.left_center() - egui::vec2(2.0, 0.0);
        assert!(!layout.panel_rect.contains(blank));
        canvas.merge(vec![egui::Event::PointerMoved(blank)]);
        canvas.app.fs_focus_regained_at = Some(std::time::Instant::now());
        canvas.app.settings.vst3_enabled = f2;
        canvas.app.settings.vst3_gui_visible = f2;
        assert_eq!(
            canvas.split_click_at(pos, egui::Modifiers::NONE),
            (false, false)
        );
        assert!(!canvas.app.fs_front_pointer.clicks.previous_close_candidate);
        canvas.app.settings.fullscreen_navigator_visible = false;
        // Still close enough for egui to report a double click. The navigator
        // release must not become the first candidate of that pair.
        assert_eq!(
            canvas.merge(click(blank, egui::Modifiers::NONE)),
            (false, false)
        );
        canvas.time += 1.0;
        assert_eq!(canvas.merge(double_click(blank)), (true, false));
    }
}

#[cfg(windows)]
#[test]
fn close_mode_focus_restore_899_window_context_closes_only_its_viewer() {
    let _input = crate::key_input::lock_test_input();
    for f2 in [false, true] {
        let mut canvas = Canvas::for_focus_restore();
        let main_items = canvas.app.items.clone();
        let main_generation = canvas.app.items_generation;
        let sibling = canvas.app.build_window_context_for_test(18992, |owner| {
            owner.items = vec![GridItem::Image(PathBuf::from("c:/899/sibling.png"))];
            owner.fullscreen_idx = Some(0);
            owner.fs_zoom = 2.5;
            owner.fs_pan = egui::vec2(10.0, 20.0);
            owner.items_generation = 8992;
        });
        let closing = canvas.app.build_window_context_for_test(18991, |owner| {
            owner.items = vec![GridItem::Image(PathBuf::from("c:/899/closing.png"))];
            owner.visible_indices = vec![0];
            owner.fullscreen_idx = Some(0);
            owner.viewer_presentation = ViewerPresentation::DetachedWindow;
            owner.fs_zoom = 1.0;
            owner.fs_pan = egui::Vec2::ZERO;
        });
        canvas.merge_window(
            closing,
            18991,
            vec![egui::Event::PointerMoved(canvas_pos())],
            false,
        );
        canvas.app.fs_focus_regained_at = Some(std::time::Instant::now());
        canvas.app.settings.vst3_enabled = f2;
        canvas.app.settings.vst3_gui_visible = f2;
        assert_eq!(
            canvas.merge_window(
                closing,
                18991,
                vec![button(canvas_pos(), true, egui::Modifiers::NONE)],
                false,
            ),
            (false, false),
        );
        assert_eq!(
            canvas.merge_window(
                closing,
                18991,
                vec![button(canvas_pos(), false, egui::Modifiers::NONE)],
                false,
            ),
            (false, false),
        );
        assert!(canvas.app.fs_front_pointer.clicks.previous_close_candidate);
        assert_eq!(
            canvas.merge_window(
                closing,
                18991,
                click(canvas_pos(), egui::Modifiers::NONE),
                true
            ),
            (true, false),
        );
        assert_eq!(canvas.app.items, main_items);
        assert_eq!(canvas.app.items_generation, main_generation);
        assert_eq!(canvas.app.fullscreen_idx, Some(0));
        canvas.app.with_window_context_for_test(sibling, |owner| {
            assert_eq!(owner.fullscreen_idx, Some(0));
            assert_eq!(owner.items_generation, 8992);
            assert_eq!(owner.fs_zoom, 2.5);
            assert_eq!(owner.fs_pan, egui::vec2(10.0, 20.0));
            assert_eq!(
                owner.items,
                vec![GridItem::Image(PathBuf::from("c:/899/sibling.png"))]
            );
        });
    }
}

#[cfg(windows)]
#[test]
fn close_mode_focus_restore_899_context_focus_roundtrip_does_not_reuse_pair() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = Canvas::for_focus_restore();
    let build = |owner: &mut App| {
        owner.items = vec![GridItem::Image(PathBuf::from("c:/899/page.png"))];
        owner.visible_indices = vec![0];
        owner.fullscreen_idx = Some(0);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        owner.fs_zoom = 1.0;
        owner.fs_pan = egui::Vec2::ZERO;
    };
    let a = canvas.app.build_window_context_for_test(18993, build);
    let b = canvas.app.build_window_context_for_test(18994, build);
    canvas.app.with_window_context_for_test(b, |owner| {
        owner.fs_zoom = 2.5;
        owner.fs_pan = egui::vec2(10.0, 20.0);
    });
    canvas.merge_window(
        a,
        18993,
        vec![egui::Event::PointerMoved(canvas_pos())],
        false,
    );
    canvas.app.fs_focus_regained_at = Some(std::time::Instant::now());
    // Leave A with a candidate and suppression for the remainder of this pass.
    // The next advance retires the ended flow; focus loss still retires the candidate.
    canvas.app.settings.vst3_enabled = true;
    canvas.app.settings.vst3_gui_visible = true;
    assert_eq!(
        canvas.merge_window(a, 18993, click(canvas_pos(), egui::Modifiers::NONE), false),
        (false, false),
    );
    assert_eq!(
        canvas.app.fs_primary_suppression,
        FullscreenPrimarySuppression::ReleasedThisPass(
            FullscreenPrimarySuppressionReason::FocusRestore
        ),
    );
    assert!(canvas.app.fs_front_pointer.clicks.previous_close_candidate);
    canvas.merge_window(a, 18993, vec![], false);
    assert_eq!(
        canvas.app.fs_primary_suppression,
        FullscreenPrimarySuppression::Idle
    );
    assert!(canvas.app.fs_front_pointer.clicks.previous_close_candidate);
    for (context, window) in [(b, 18994), (a, 18993)] {
        assert_eq!(
            canvas.merge_window(
                context,
                window,
                vec![
                    egui::Event::WindowFocused(false),
                    egui::Event::WindowFocused(true)
                ],
                false,
            ),
            (false, false),
        );
        assert!(!canvas.app.fs_front_pointer.clicks.previous_close_candidate);
        assert_eq!(
            canvas.merge_window(
                context,
                window,
                click(canvas_pos(), egui::Modifiers::NONE),
                false
            ),
            (false, false),
        );
        assert!(canvas.app.fs_front_pointer.clicks.previous_close_candidate);
    }
    for (context, zoom, pan) in [(a, 1.0, egui::Vec2::ZERO), (b, 2.5, egui::vec2(10.0, 20.0))] {
        canvas.app.with_window_context_for_test(context, |owner| {
            assert_eq!(owner.fullscreen_idx, Some(0));
            assert_eq!(owner.fs_zoom, zoom);
            assert_eq!(owner.fs_pan, pan);
        });
    }
    assert_eq!(canvas.app.fullscreen_idx, Some(0));
}
