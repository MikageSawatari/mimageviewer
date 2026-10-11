//! Exercise the first keyboard turn through the production input and navigation owners.
use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, ready_app};

#[derive(Clone, Copy)]
enum InputRoute {
    ViewerEgui,
    RootEgui,
    #[cfg(windows)]
    ViewerNative,
    #[cfg(windows)]
    RootNative,
}

impl InputRoute {
    fn is_root(self) -> bool {
        matches!(self, Self::RootEgui) || {
            #[cfg(windows)]
            {
                matches!(self, Self::RootNative)
            }
            #[cfg(not(windows))]
            {
                false
            }
        }
    }

    fn is_native(self) -> bool {
        #[cfg(windows)]
        {
            matches!(self, Self::ViewerNative | Self::RootNative)
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}

fn key_event(pressed: bool) -> egui::Event {
    egui::Event::Key {
        key: egui::Key::ArrowRight,
        physical_key: Some(egui::Key::ArrowRight),
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn viewer_frame(
    ctx: &egui::Context,
    viewport: egui::ViewportId,
    events: Vec<egui::Event>,
    mut run: impl FnMut(&mut egui::Ui, &egui::Context),
) {
    let mut raw = egui::RawInput {
        viewport_id: viewport,
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(600.0, 400.0),
        )),
        events,
        ..Default::default()
    };
    raw.viewports.insert(
        viewport,
        egui::ViewportInfo {
            parent: (viewport != egui::ViewportId::ROOT).then_some(egui::ViewportId::ROOT),
            focused: Some(true),
            ..Default::default()
        },
    );
    let _ = ctx.run(raw, |ctx| {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| run(ui, ctx));
    });
}

fn neutral_gamepad_poll(app: &mut App, ctx: &egui::Context) {
    #[cfg(windows)]
    app.dispatch_neutral_gamepad_poll_for_test(ctx);
    #[cfg(not(windows))]
    let _ = (app, ctx);
}

fn assert_completed_operation_events(operation: ViewerOperationId) {
    let events = take_test_events();
    let events: Vec<_> = events
        .iter()
        .filter(|(fact, _, _)| fact.operation_id == operation)
        .collect();
    for kind in ["opportunity", "offered", "start", "finish", "fully_visible"] {
        assert_eq!(
            events
                .iter()
                .filter(|(_, actual, _)| *actual == kind)
                .count(),
            1,
            "one {kind} must belong to the original operation: {events:?}"
        );
    }
    assert!(
        events
            .iter()
            .all(|(_, kind, _)| !matches!(*kind, "skip" | "cancel")),
        "neutral polls must not manufacture skip/cancel events: {events:?}"
    );
}

fn assert_neutral_polls_preserve_turn(
    app: &mut App,
    ctx: &egui::Context,
    viewport: egui::ViewportId,
    tracker: &Arc<SubmissionTracker>,
) {
    // The backend alternates ROOT and child passes with independent frame counters.
    viewer_frame(ctx, egui::ViewportId::ROOT, Vec::new(), |_, ctx| {
        neutral_gamepad_poll(app, ctx);
    });
    assert_eq!(tracker.status(), SubmissionStatus::Started);
    viewer_frame(ctx, viewport, Vec::new(), |ui, ctx| {
        neutral_gamepad_poll(app, ctx);
        app.begin_page_turn_viewer_frame(ctx, false, None);
        let (_, sources) = draw_current(app, ui, ctx);
        let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
            panic!("paged output")
        };
        assert!(
            receipt.curl,
            "a neutral gamepad poll must not cancel the turn"
        );
    });
    assert_eq!(tracker.status(), SubmissionStatus::Started);
}

#[cfg(windows)]
fn install_native_press(viewport: egui::ViewportId) {
    // The down/up batch tests discrete attribution without sampling a real held key.
    let edges = [true, false]
        .into_iter()
        .map(|pressed| crate::key_input::KeyEdge {
            operation_id: crate::key_input::KeyOperationId::unassigned(),
            source_hwnd: 0x341,
            source_viewport: viewport,
            virtual_key: 0x27,
            scan_code: 0x4d,
            extended: true,
            pressed,
            repeat: false,
            ctrl: false,
            shift: false,
            alt: false,
        })
        .collect();
    crate::key_input::set_test_frame_for_viewport(viewport, edges);
}

fn first_keyboard_turn_reaches_started_and_finished(route: InputRoute, mode: SpreadMode) {
    take_test_events();
    let ctx = egui::Context::default();
    install(&ctx);
    crate::key_input::install_operation_input_plugin(&ctx);
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("page_turn_input_enabled"), true));
    let mut app = ready_app(&ctx, mode);
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    #[cfg(windows)]
    {
        app.native_video_in_window_active = false;
    }
    let viewport = if route.is_root() {
        egui::ViewportId::ROOT
    } else {
        app.fullscreen_viewport_id()
    };
    assert!(route.is_root() || viewport != egui::ViewportId::ROOT);
    let target = if mode.is_spread() { 2 } else { 1 };

    // Ordinary preparation makes the target final-ready, then adopts the actual outgoing flat.
    // No synthetic operation, direct offer, or hand-built presentation state primes the turn.
    for anchor in [target, 0] {
        app.fullscreen_idx = Some(anchor);
        viewer_frame(&ctx, viewport, Vec::new(), |ui, ctx| {
            neutral_gamepad_poll(&mut app, ctx);
            draw_current(&mut app, ui, ctx);
        });
    }
    assert!(matches!(
        app.page_turn_presentation.state,
        PresentationState::Flat(_)
    ));
    assert!(!app.page_turn_barrier_active());

    let mut curl_receipt = None;
    let mut ingress_operation = None;
    viewer_frame(
        &ctx,
        viewport,
        vec![key_event(true), key_event(false)],
        |ui, ctx| {
            neutral_gamepad_poll(&mut app, ctx);
            #[cfg(windows)]
            if route.is_native() {
                install_native_press(viewport);
            }
            assert!(crate::keyboard_input::cached_keyboard_owner(ctx).is_none());
            ingress_operation = crate::key_input::operation_key_edges(ctx, viewport)
                .into_iter()
                .find(|edge| edge.pressed())
                .map(|edge| ViewerOperationId::Key(edge.operation_id()));
            assert!(ingress_operation.is_some());
            if route.is_root() {
                assert!(app.handle_fullscreen_root_key_input(ctx));
            } else {
                assert!(matches!(
                    app.begin_page_turn_viewer_frame(ctx, false, None),
                    PageTurnFrameEntry::Continue {
                        committed: false,
                        ..
                    }
                ));
                let mut operation = None;
                let action = app.handle_fs_key_input_with_operation(
                    ctx,
                    0,
                    mode.is_spread(),
                    &mut operation,
                );
                assert!(!action.page_nav.is_none());
                app.handle_fs_navigation_with_operation(
                    ctx,
                    action.close,
                    action.close_to_page_list,
                    action.ctrl_nav,
                    action.sibling_nav,
                    action.mouse_nav,
                    action.page_nav,
                    action.jump_to,
                    0,
                    operation,
                );
            }
            assert_eq!(app.fullscreen_idx, Some(target));
            let (_, sources) = draw_current(&mut app, ui, ctx);
            let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                panic!("paged output")
            };
            assert!(
                receipt.curl,
                "the first real keyboard navigation must submit the curl"
            );
            curl_receipt = Some(receipt);
        },
    );
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    assert!(curl_receipt.unwrap().was_accepted());
    let tracker = match &mut app.page_turn_presentation.state {
        PresentationState::Turning { offer, .. } => {
            assert!(matches!(offer.fact.operation_id, ViewerOperationId::Key(_)));
            assert_eq!(Some(offer.fact.operation_id), ingress_operation);
            assert_eq!(offer.fact.source, viewport);
            assert_eq!(offer.fact.phase, OperationPhase::Press);
            assert_eq!(offer.submission.status(), SubmissionStatus::Started);
            offer.submission.clone()
        }
        _ => panic!("adopted keyboard curl must be Turning"),
    };
    assert_neutral_polls_preserve_turn(&mut app, &ctx, viewport, &tracker);
    if let PresentationState::Turning { offer, .. } = &mut app.page_turn_presentation.state {
        // Advance only this test's clock origin; no wall-clock wait or production hook.
        offer.accepted_at -= offer.duration + Duration::from_millis(1);
    } else {
        panic!("neutral polls must preserve Turning");
    }
    let mut flat_receipt = None;
    viewer_frame(&ctx, viewport, Vec::new(), |ui, ctx| {
        neutral_gamepad_poll(&mut app, ctx);
        app.begin_page_turn_viewer_frame(ctx, false, None);
        let (_, sources) = draw_current(&mut app, ui, ctx);
        let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
            panic!("paged output")
        };
        assert!(!receipt.curl);
        flat_receipt = Some(receipt);
    });
    assert!(flat_receipt.unwrap().was_accepted());
    assert_eq!(tracker.status(), SubmissionStatus::Finished);
    assert_eq!(app.fullscreen_idx, Some(target));
    assert_completed_operation_events(ingress_operation.unwrap());
}

#[test]
fn first_viewer_and_root_egui_keyboard_turns_start_and_complete() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for route in [InputRoute::ViewerEgui, InputRoute::RootEgui] {
        for mode in [SpreadMode::Single, SpreadMode::Ltr] {
            first_keyboard_turn_reaches_started_and_finished(route, mode);
        }
    }
}

#[cfg(windows)]
#[test]
fn first_viewer_and_root_native_keyboard_turns_start_and_complete() {
    let _serial = crate::key_input::lock_test_input();
    crate::key_input::clear_test_frame();
    for route in [InputRoute::ViewerNative, InputRoute::RootNative] {
        for mode in [SpreadMode::Single, SpreadMode::Ltr] {
            first_keyboard_turn_reaches_started_and_finished(route, mode);
        }
    }
}

fn pointer_button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn actual_pointer_handler(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
) -> (FsPageNav, bool, Option<ViewerOperationId>) {
    let idx = app.fullscreen_idx.unwrap();
    let state = app.prepare_fullscreen_state(ctx, idx, FsPageTurnDecision::normal());
    let mut operation = None;
    let full_rect = ui.max_rect();
    let is_spread = app.spread_mode.is_spread();
    let (navigation, close) = app.handle_fs_wheel_and_click_with_geometry_and_operation(
        ui,
        ctx,
        full_rect,
        &state,
        StillSeekGeometry::bar_only(),
        is_spread,
        0,
        &mut None,
        None,
        &mut operation,
    );
    (navigation, close, operation)
}

fn first_pointer_click_reaches_started_and_finished(root: bool, mode: SpreadMode) {
    take_test_events();
    let ctx = egui::Context::default();
    install(&ctx);
    install_fs_navigator_input_tracking(&ctx);
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("page_turn_input_enabled"), true));
    let mut app = ready_app(&ctx, mode);
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    #[cfg(windows)]
    {
        app.native_video_in_window_active = root;
    }
    let viewport = if root {
        egui::ViewportId::ROOT
    } else {
        app.fullscreen_viewport_id()
    };
    let target = if mode.is_spread() { 2 } else { 1 };
    let point = egui::pos2(450.0, 180.0);
    for anchor in [target, 0] {
        app.fullscreen_idx = Some(anchor);
        viewer_frame(&ctx, viewport, Vec::new(), |ui, ctx| {
            neutral_gamepad_poll(&mut app, ctx);
            app.begin_page_turn_viewer_frame(ctx, root, None);
            let (nav, close, _) = actual_pointer_handler(&mut app, ui, ctx);
            assert!(nav.is_none());
            assert!(!close);
            draw_current(&mut app, ui, ctx);
        });
    }
    // Publish the real fs_click click-and-drag response, then deliver the mouse stream.
    viewer_frame(
        &ctx,
        viewport,
        vec![egui::Event::PointerMoved(point)],
        |ui, ctx| {
            neutral_gamepad_poll(&mut app, ctx);
            app.begin_page_turn_viewer_frame(ctx, root, None);
            actual_pointer_handler(&mut app, ui, ctx);
            draw_current(&mut app, ui, ctx);
        },
    );
    let mut press_operation = None;
    viewer_frame(
        &ctx,
        viewport,
        vec![pointer_button(point, true)],
        |ui, ctx| {
            neutral_gamepad_poll(&mut app, ctx);
            app.begin_page_turn_viewer_frame(ctx, root, None);
            press_operation = app.page_turn_pointer_navigation_operation(ctx, "pointer");
            assert!(matches!(
                press_operation,
                Some(ViewerOperationId::Pointer { .. })
            ));
            let (nav, close, _) = actual_pointer_handler(&mut app, ui, ctx);
            assert!(nav.is_none());
            assert!(!close);
            draw_current(&mut app, ui, ctx);
        },
    );
    let release_point = point + egui::vec2(1.0, 0.0);
    viewer_frame(
        &ctx,
        viewport,
        vec![egui::Event::PointerMoved(release_point)],
        |ui, ctx| {
            neutral_gamepad_poll(&mut app, ctx);
            app.begin_page_turn_viewer_frame(ctx, root, None);
            let (nav, close, _) = actual_pointer_handler(&mut app, ui, ctx);
            assert!(nav.is_none());
            assert!(!close);
            draw_current(&mut app, ui, ctx);
        },
    );
    let mut curl_receipt = None;
    viewer_frame(
        &ctx,
        viewport,
        vec![pointer_button(release_point, false)],
        |ui, ctx| {
            neutral_gamepad_poll(&mut app, ctx);
            app.begin_page_turn_viewer_frame(ctx, root, None);
            let (nav, close, operation) = actual_pointer_handler(&mut app, ui, ctx);
            assert!(!nav.is_none(), "the real fs_click release must navigate");
            assert!(!close);
            assert_eq!(
                operation, press_operation,
                "release must retain the original press identity"
            );
            app.handle_fs_navigation_with_operation(
                ctx, false, false, None, None, None, nav, None, 0, operation,
            );
            assert_eq!(app.fullscreen_idx, Some(target));
            let (_, sources) = draw_current(&mut app, ui, ctx);
            let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                panic!("paged output")
            };
            assert!(receipt.curl);
            curl_receipt = Some(receipt);
        },
    );
    assert!(curl_receipt.unwrap().was_accepted());
    let tracker = match &app.page_turn_presentation.state {
        PresentationState::Turning { offer, .. } => {
            assert_eq!(Some(offer.fact.operation_id), press_operation);
            assert_eq!(offer.fact.cause, "pointer");
            assert_eq!(offer.submission.status(), SubmissionStatus::Started);
            offer.submission.clone()
        }
        _ => panic!("real click must submit a turn"),
    };
    assert_neutral_polls_preserve_turn(&mut app, &ctx, viewport, &tracker);
    if let PresentationState::Turning { offer, .. } = &mut app.page_turn_presentation.state {
        offer.accepted_at -= offer.duration + Duration::from_millis(1);
    } else {
        panic!("neutral poll canceled the real click");
    }
    let mut flat_receipt = None;
    viewer_frame(&ctx, viewport, Vec::new(), |ui, ctx| {
        neutral_gamepad_poll(&mut app, ctx);
        app.begin_page_turn_viewer_frame(ctx, root, None);
        let (_, sources) = draw_current(&mut app, ui, ctx);
        let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
            panic!("paged output")
        };
        assert!(!receipt.curl);
        flat_receipt = Some(receipt);
    });
    assert!(flat_receipt.unwrap().was_accepted());
    assert_eq!(tracker.status(), SubmissionStatus::Finished);
    assert_completed_operation_events(press_operation.unwrap());
}

#[test]
fn first_viewer_and_root_pointer_clicks_start_and_complete_despite_neutral_gamepad_polls() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for root in [false, true] {
        for mode in [SpreadMode::Single, SpreadMode::Ltr] {
            first_pointer_click_reaches_started_and_finished(root, mode);
        }
    }
}

fn ready_single_viewer(ctx: &egui::Context) -> (crate::app::AppTestEnvForTest, egui::ViewportId) {
    install(ctx);
    install_fs_navigator_input_tracking(ctx);
    crate::key_input::install_operation_input_plugin(ctx);
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("page_turn_input_enabled"), true));
    let mut app = ready_app(ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    let viewport = app.fullscreen_viewport_id();
    for anchor in [1, 0] {
        app.fullscreen_idx = Some(anchor);
        viewer_frame(ctx, viewport, Vec::new(), |ui, ctx| {
            neutral_gamepad_poll(&mut app, ctx);
            draw_current(&mut app, ui, ctx);
        });
    }
    (app, viewport)
}

#[test]
fn mixed_key_and_wheel_batch_skips_once_for_the_actual_accepted_key_navigation() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    take_test_events();
    let ctx = egui::Context::default();
    let (mut app, viewport) = ready_single_viewer(&ctx);
    let events = vec![
        key_event(true),
        key_event(false),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -1.0),
            modifiers: egui::Modifiers::NONE,
        },
    ];
    let mut accepted_operation = None;
    viewer_frame(&ctx, viewport, events, |ui, ctx| {
        neutral_gamepad_poll(&mut app, ctx);
        app.begin_page_turn_viewer_frame(ctx, false, None);
        let original_key_edges = crate::key_input::operation_key_edges(ctx, viewport);
        let action = app.handle_fs_key_input_with_operation(ctx, 0, false, &mut accepted_operation);
        assert!(!action.page_nav.is_none());
        assert!(
            matches!(accepted_operation, Some(ViewerOperationId::Key(_))),
            "accepted navigation lost its input identity: original={original_key_edges:?}, after_consume={:?}, facts={:?}, barrier={}",
            crate::key_input::operation_key_edges(ctx, viewport),
            app.page_turn_presentation.transaction.facts,
            app.page_turn_barrier_active(),
        );
        app.handle_fs_navigation_with_operation(
            ctx,
            action.close,
            action.close_to_page_list,
            action.ctrl_nav,
            action.sibling_nav,
            action.mouse_nav,
            action.page_nav,
            action.jump_to,
            0,
            accepted_operation,
        );
        assert_eq!(app.fullscreen_idx, Some(1));
        // A replay of the same accepted handoff cannot duplicate its diagnostic budget.
        app.offer_page_turn_with_operation(ctx, 1, true, accepted_operation);
        let (_, sources) = draw_current(&mut app, ui, ctx);
        let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
            panic!("paged output")
        };
        assert!(!receipt.curl);
    });
    let events = take_test_events();
    assert_eq!(
        events.len(),
        1,
        "one accepted navigation has one skip: {events:?}"
    );
    assert_eq!(Some(events[0].0.operation_id), accepted_operation);
    assert_eq!((events[0].1, events[0].2), ("skip", "superseded_animation"));
    assert_eq!(app.fullscreen_idx, Some(1));
}

#[test]
fn actual_navigation_without_ingress_fact_skips_once_without_delaying_the_target() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    take_test_events();
    let ctx = egui::Context::default();
    let (mut app, viewport) = ready_single_viewer(&ctx);
    viewer_frame(&ctx, viewport, Vec::new(), |ui, ctx| {
        neutral_gamepad_poll(&mut app, ctx);
        app.begin_page_turn_viewer_frame(ctx, false, None);
        app.handle_fs_navigation_with_operation(
            ctx,
            false,
            false,
            None,
            None,
            None,
            FsPageNav::Delta(1),
            None,
            0,
            None,
        );
        assert_eq!(app.fullscreen_idx, Some(1));
        app.offer_page_turn_with_operation(ctx, 1, true, None);
        let (_, sources) = draw_current(&mut app, ui, ctx);
        let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
            panic!("paged output")
        };
        assert!(!receipt.curl);
    });
    let events = take_test_events();
    assert_eq!(
        events.len(),
        1,
        "missing ingress still has one diagnostic: {events:?}"
    );
    assert_eq!(
        (events[0].1, events[0].2),
        ("skip", "missing_operation_fact")
    );
    assert_eq!(events[0].0.cause, "unidentified_navigation");
    assert_eq!(app.fullscreen_idx, Some(1));
}
