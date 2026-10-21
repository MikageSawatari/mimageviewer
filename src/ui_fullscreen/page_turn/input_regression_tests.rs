//! Exercise the first keyboard turn through the production input and navigation owners.
#![cfg(test)]
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
    run: impl FnMut(&mut egui::Ui, &egui::Context),
) {
    viewer_frame_with_focus(ctx, viewport, true, events, run);
}

fn viewer_frame_with_focus(
    ctx: &egui::Context,
    viewport: egui::ViewportId,
    focused: bool,
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
        focused,
        ..Default::default()
    };
    raw.viewports.insert(
        viewport,
        egui::ViewportInfo {
            parent: (viewport != egui::ViewportId::ROOT).then_some(egui::ViewportId::ROOT),
            focused: Some(focused),
            ..Default::default()
        },
    );
    let _ = ctx
        .run_pass(raw, |ctx| {
            ctx.root_ui(|root_ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(root_ui, |ui| run(ui, ctx))
            });
        })
        .drop_without_applying_deltas();
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
    keyboard_turn_with_resident_quality(route, mode, false);
}

fn keyboard_turn_with_resident_quality(route: InputRoute, mode: SpreadMode, thumbnail: bool) {
    take_test_events();
    let ctx = egui::Context::default();
    install(&ctx);
    crate::key_input::install_operation_input_plugin(&ctx);
    ctx.data_mut(|data| data.insert_temp(egui::Id::unique("page_turn_input_enabled"), true));
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
    if thumbnail {
        app.thumbnails = (0..app.items.len())
            .map(|_| ThumbnailState::Pending)
            .collect();
        app.final_composite_cache
            .retain(|key, _| key.edit_key.idx < target);
        app.comic_cache.retain(|idx, _| *idx < target);
        app.edit_result_cache.retain(|key, _| key.idx < target);
        for idx in target..target + if mode.is_spread() { 2 } else { 1 } {
            app.fs_cache.remove(&idx);
            app.thumbnails[idx] = ThumbnailState::Loaded {
                tex: ctx.load_texture(
                    format!("input-resident-thumb-{idx}"),
                    egui::ColorImage::filled([8, 12], egui::Color32::WHITE),
                    egui::TextureOptions::LINEAR,
                ),
                origin: crate::thumb_loader::ThumbLoadOrigin::UpgradeableCache,
                from_edit_preview: false,
                rendered_at_px: 12,
                source_coverage: crate::source_coverage::SourceCoverage::FullSource,
                source_dims: Some((80, 120)),
                layout_dims: None,
            };
        }
    }

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
            if thumbnail {
                // Production pre-draw readiness must allow the captured
                // thumbnail, rather than covering it with the old stop page.
                app.resolve_fs_navigation_sequence_target(ctx, target, false);
                assert!(app.fs_nav_holdover_for_draw().is_none());
            }
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
    if let PresentationState::Turning { clock, offer, .. } = &mut app.page_turn_presentation.state {
        // Advance only this test's clock origin; no wall-clock wait or production hook.
        *clock = MotionClock::Running {
            start_at: Instant::now() - offer.duration - Duration::from_millis(1),
        };
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
        false,
        &mut operation,
    );
    (navigation, close, operation)
}

fn first_pointer_click_reaches_started_and_finished(root: bool, mode: SpreadMode) {
    take_test_events();
    let ctx = egui::Context::default();
    install(&ctx);
    install_fs_navigator_input_tracking(&ctx);
    ctx.data_mut(|data| data.insert_temp(egui::Id::unique("page_turn_input_enabled"), true));
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
    if let PresentationState::Turning { clock, offer, .. } = &mut app.page_turn_presentation.state {
        *clock = MotionClock::Running {
            start_at: Instant::now() - offer.duration - Duration::from_millis(1),
        };
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
    ctx.data_mut(|data| data.insert_temp(egui::Id::unique("page_turn_input_enabled"), true));
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

fn advance_outer_root_frames(ctx: &egui::Context) {
    // Production accepts child input after show_viewport_immediate returns to ROOT.
    // Its frame counter is independent of the child counter, even on the same Context.
    for _ in 0..7 {
        viewer_frame(ctx, egui::ViewportId::ROOT, Vec::new(), |_, _| {});
    }
}

fn accept_key_action_on_outer_root(
    app: &mut App,
    ctx: &egui::Context,
    from: usize,
    action: &FsKeyAction,
    operation: Option<ViewerOperationId>,
) {
    assert!(action.mouse_nav.is_none());
    app.handle_fs_navigation_with_operation(
        ctx,
        action.close,
        action.close_to_page_list,
        action.ctrl_nav,
        action.sibling_nav,
        None,
        action.page_nav,
        action.jump_to,
        from,
        operation,
    );
}

#[test]
fn child_boundary_navigation_skips_once_with_original_fact_after_outer_root_handoff() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    take_test_events();
    let ctx = egui::Context::default();
    let (mut app, viewport) = ready_single_viewer(&ctx);
    advance_outer_root_frames(&ctx);
    let from = *app.get_nav_indices().last().unwrap();
    app.fullscreen_idx = Some(from);
    let mut handoff = None;
    let mut child_frame = 0;
    viewer_frame(
        &ctx,
        viewport,
        vec![key_event(true), key_event(false)],
        |_, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            let mut operation = None;
            let action = app.handle_fs_key_input_with_operation(ctx, from, false, &mut operation);
            assert!(!action.page_nav.is_none());
            let fact = app.page_turn_presentation.transaction.facts[0];
            assert_eq!(Some(fact.operation_id), operation);
            child_frame = ctx.cumulative_frame_nr();
            handoff = Some((action, fact));
        },
    );
    let (action, original) = handoff.unwrap();
    viewer_frame(&ctx, egui::ViewportId::ROOT, Vec::new(), |_, ctx| {
        assert_ne!(ctx.cumulative_frame_nr(), child_frame);
        accept_key_action_on_outer_root(&mut app, ctx, from, &action, Some(original.operation_id));
        // Replaying the same accepted diagnostic must not consume a second budget.
        app.page_turn_excluded_navigation_with_operation(
            ctx,
            "navigation_boundary",
            Some(original.operation_id),
        );
    });
    assert_eq!(app.fullscreen_idx, Some(from));
    assert_eq!(original.source, viewport);
    assert_eq!(original.phase, OperationPhase::Press);
    let events = take_test_events();
    assert_eq!(events.len(), 1, "one accepted boundary: {events:?}");
    assert!(same_operation(&events[0].0, &original));
    assert_eq!(
        events[0].0.diagnostic_input_seq,
        original.diagnostic_input_seq
    );
    assert_eq!((events[0].1, events[0].2), ("skip", "navigation_boundary"));
}

#[test]
fn child_repeat_navigation_skips_once_with_original_repeat_fact_on_outer_root() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    take_test_events();
    let ctx = egui::Context::default();
    let (mut app, viewport) = ready_single_viewer(&ctx);
    advance_outer_root_frames(&ctx);
    #[cfg(windows)]
    {
        let now = Instant::now();
        crate::key_input::arm_test_synthetic_input(1, viewport);
        crate::key_input::enqueue_test_synthetic_command(
            crate::key_input::SyntheticKeyCommand::down(
                now,
                crate::key_input::SyntheticNavigationKey::Right,
                crate::key_input::SyntheticModifiers::default(),
            ),
        );
        crate::key_input::advance_test_synthetic_input(now);
    }
    let mut repeat = key_event(true);
    if let egui::Event::Key { repeat, .. } = &mut repeat {
        *repeat = true;
    }
    let mut handoff = None;
    let mut child_frame = 0;
    viewer_frame(&ctx, viewport, vec![repeat], |_, ctx| {
        #[cfg(windows)]
        crate::key_input::set_test_frame_for_viewport(
            viewport,
            vec![crate::key_input::KeyEdge {
                operation_id: crate::key_input::KeyOperationId::unassigned(),
                source_hwnd: 0x341,
                source_viewport: viewport,
                virtual_key: 0x27,
                scan_code: 0x4d,
                extended: true,
                pressed: true,
                repeat: true,
                ctrl: false,
                shift: false,
                alt: false,
            }],
        );
        app.begin_page_turn_viewer_frame(ctx, false, None);
        let mut operation = None;
        let action = app.handle_fs_key_input_with_operation(ctx, 0, false, &mut operation);
        assert!(
            !action.page_nav.is_none(),
            "a held repeat uses the real key handler"
        );
        let fact = app.page_turn_presentation.transaction.facts[0];
        assert_eq!(Some(fact.operation_id), operation);
        child_frame = ctx.cumulative_frame_nr();
        handoff = Some((action, fact));
    });
    let (action, original) = handoff.unwrap();
    viewer_frame(&ctx, egui::ViewportId::ROOT, Vec::new(), |_, ctx| {
        assert_ne!(ctx.cumulative_frame_nr(), child_frame);
        accept_key_action_on_outer_root(&mut app, ctx, 0, &action, Some(original.operation_id));
        app.page_turn_excluded_navigation_with_operation(
            ctx,
            "repeat_operation",
            Some(original.operation_id),
        );
    });
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(original.source, viewport);
    assert_eq!(original.phase, OperationPhase::Repeat);
    let events = take_test_events();
    assert_eq!(events.len(), 1, "one accepted repeat: {events:?}");
    assert!(same_operation(&events[0].0, &original));
    assert_eq!(
        events[0].0.diagnostic_input_seq,
        original.diagnostic_input_seq
    );
    assert_eq!(events[0].1, "skip");
    assert_ne!(events[0].2, "missing_operation_fact");
}

#[cfg(windows)]
#[test]
fn child_key_handler_hands_off_fresh_press_id_after_same_batch_repeat_and_release() {
    let _serial = crate::key_input::lock_test_input();
    take_test_events();
    let ctx = egui::Context::default();
    let (mut app, viewport) = ready_single_viewer(&ctx);
    advance_outer_root_frames(&ctx);
    let mut repeat = key_event(true);
    if let egui::Event::Key { repeat, .. } = &mut repeat {
        *repeat = true;
    }
    let mut handoff = None;
    viewer_frame(
        &ctx,
        viewport,
        vec![repeat, key_event(false), key_event(true), key_event(false)],
        |_, ctx| {
            let edges = [(true, true), (false, false), (true, false), (false, false)]
                .into_iter()
                .map(|(pressed, repeat)| crate::key_input::KeyEdge {
                    operation_id: crate::key_input::KeyOperationId::unassigned(),
                    source_hwnd: 0x341,
                    source_viewport: viewport,
                    virtual_key: 0x27,
                    scan_code: 0x4d,
                    extended: true,
                    pressed,
                    repeat,
                    ctrl: false,
                    shift: false,
                    alt: false,
                })
                .collect();
            crate::key_input::set_test_frame_for_viewport(viewport, edges);
            app.begin_page_turn_viewer_frame(ctx, false, None);
            let snapshot = crate::key_input::native_operation_key_edges(viewport);
            let initial = ViewerOperationId::Key(snapshot[2].operation_id);
            let mut operation = None;
            let action = app.handle_fs_key_input_with_operation(ctx, 0, false, &mut operation);
            assert!(!action.page_nav.is_none());
            assert_eq!(
                operation,
                Some(initial),
                "the consumer selects the fresh press, not the earlier repeat"
            );
            let original = *app
                .page_turn_presentation
                .transaction
                .facts
                .iter()
                .find(|fact| fact.operation_id == initial)
                .unwrap();
            handoff = Some((action, original, ctx.cumulative_frame_nr()));
        },
    );
    let (action, original, child_frame) = handoff.unwrap();
    viewer_frame(&ctx, egui::ViewportId::ROOT, Vec::new(), |_, ctx| {
        assert_ne!(ctx.cumulative_frame_nr(), child_frame);
        accept_key_action_on_outer_root(&mut app, ctx, 0, &action, Some(original.operation_id));
    });
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(original.phase, OperationPhase::Press);
    let events = take_test_events();
    assert_eq!(
        events.len(),
        1,
        "the mixed batch accepts only one navigation: {events:?}"
    );
    assert!(same_operation(&events[0].0, &original));
    assert_eq!(events[0].1, "skip");
    assert_ne!(events[0].2, "missing_operation_fact");
}

#[test]
fn child_key_and_wheel_navigation_start_and_complete_after_outer_root_acceptance() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for wheel in [false, true] {
        #[cfg(windows)]
        crate::key_input::clear_test_frame();
        take_test_events();
        let ctx = egui::Context::default();
        let (mut app, viewport) = ready_single_viewer(&ctx);
        app.settings.page_turn_animation_duration_ms = 600;
        advance_outer_root_frames(&ctx);
        let input = if wheel {
            vec![
                egui::Event::PointerMoved(egui::pos2(300.0, 180.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -120.0),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                },
            ]
        } else {
            vec![key_event(true), key_event(false)]
        };
        let mut handoff = None;
        let mut child_frame = 0;
        viewer_frame(&ctx, viewport, input, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            let (nav, operation) = if wheel {
                let (nav, close, operation) = actual_pointer_handler(&mut app, ui, ctx);
                assert!(!close);
                (nav, operation)
            } else {
                let mut operation = None;
                let action = app.handle_fs_key_input_with_operation(ctx, 0, false, &mut operation);
                (action.page_nav, operation)
            };
            assert!(!nav.is_none());
            let fact = app.page_turn_presentation.transaction.facts[0];
            assert_eq!(Some(fact.operation_id), operation);
            child_frame = ctx.cumulative_frame_nr();
            handoff = Some((nav, fact));
        });
        let (nav, original) = handoff.unwrap();
        viewer_frame(&ctx, egui::ViewportId::ROOT, Vec::new(), |_, ctx| {
            assert_ne!(ctx.cumulative_frame_nr(), child_frame);
            app.handle_fs_navigation_with_operation(
                ctx,
                false,
                false,
                None,
                None,
                None,
                nav,
                None,
                0,
                Some(original.operation_id),
            );
        });
        assert_eq!(app.fullscreen_idx, Some(1));
        viewer_frame(&ctx, viewport, Vec::new(), |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            let (_, sources) = draw_current(&mut app, ui, ctx);
            let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                panic!("paged output")
            };
            assert!(receipt.curl);
        });
        let tracker = match &mut app.page_turn_presentation.state {
            PresentationState::Turning { clock, offer, .. } => {
                assert!(same_operation(&offer.fact, &original));
                assert_eq!(offer.submission.status(), SubmissionStatus::Started);
                *clock = MotionClock::Running {
                    start_at: Instant::now() - offer.duration - Duration::from_millis(1),
                };
                offer.submission.clone()
            }
            _ => panic!("the child input must start its own turn after ROOT acceptance"),
        };
        viewer_frame(&ctx, viewport, Vec::new(), |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            let (_, sources) = draw_current(&mut app, ui, ctx);
            let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                panic!("paged output")
            };
            assert!(!receipt.curl);
        });
        assert_eq!(tracker.status(), SubmissionStatus::Finished);
        assert_completed_operation_events(original.operation_id);
    }
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
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
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

#[test]
fn real_root_and_viewer_keyboard_handlers_start_and_complete_thumbnail_turns() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        for route in [InputRoute::RootEgui, InputRoute::ViewerEgui] {
            keyboard_turn_with_resident_quality(route, mode, true);
        }
    }
}

fn raw_wheel(delta: egui::Vec2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(egui::pos2(300.0, 180.0)),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta,
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        },
    ]
}

fn ready_wheel_viewer(
    ctx: &egui::Context,
    root: bool,
    mode: SpreadMode,
) -> (crate::app::AppTestEnvForTest, egui::ViewportId) {
    install(ctx);
    install_fs_navigator_input_tracking(ctx);
    crate::key_input::install_operation_input_plugin(ctx);
    ctx.data_mut(|data| data.insert_temp(egui::Id::unique("page_turn_input_enabled"), true));
    let mut app = ready_app(ctx, mode);
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
    // Materialize the ordinary flat unit at each possible landing first.
    for anchor in [3, 2, 1, 0] {
        app.fullscreen_idx = Some(anchor);
        viewer_frame(ctx, viewport, Vec::new(), |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, root, None);
            draw_current(&mut app, ui, ctx);
        });
    }
    (app, viewport)
}

fn accept_actual_wheel(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) -> ViewerOperationId {
    let from = app.fullscreen_idx.unwrap();
    let (navigation, close, operation) = actual_pointer_handler(app, ui, ctx);
    assert!(!close);
    assert!(
        !navigation.is_none(),
        "the ordinary wheel handler must navigate"
    );
    let operation = operation.expect("the delivered wheel retains its raw identity");
    assert!(
        matches!(operation, ViewerOperationId::Pointer { source, .. } if source == ctx.viewport_id())
    );
    app.handle_fs_navigation_with_operation(
        ctx,
        false,
        false,
        None,
        None,
        None,
        navigation,
        None,
        from,
        Some(operation),
    );
    operation
}

#[test]
fn unfocused_viewer_and_root_wheels_start_and_finish_without_focus_level_cancellation() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for root in [false, true] {
        for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
            take_test_events();
            let ctx = egui::Context::default();
            let (mut app, viewport) = ready_wheel_viewer(&ctx, root, mode);
            let target = if mode.is_spread() { 2 } else { 1 };
            let mut operation = None;
            viewer_frame_with_focus(
                &ctx,
                viewport,
                false,
                raw_wheel(egui::vec2(0.0, -120.0)),
                |ui, ctx| {
                    assert!(
                        crate::keyboard_input::keyboard_input_permits(ctx)
                            .current_level
                            .is_none()
                    );
                    app.begin_page_turn_viewer_frame(ctx, root, None);
                    operation = Some(accept_actual_wheel(&mut app, ui, ctx));
                    assert_eq!(app.fullscreen_idx, Some(target));
                    let (_, sources) = draw_current(&mut app, ui, ctx);
                    let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                        panic!("paged output")
                    };
                    assert!(receipt.curl);
                },
            );
            let operation = operation.unwrap();
            let tracker = match &app.page_turn_presentation.state {
                PresentationState::Turning { offer, .. } => {
                    assert_eq!(offer.fact.operation_id, operation);
                    assert_eq!(offer.fact.source, viewport);
                    assert_eq!(offer.fact.cause, "wheel");
                    offer.submission.clone()
                }
                _ => panic!("unfocused delivered wheel must start"),
            };
            for _ in 0..2 {
                viewer_frame_with_focus(&ctx, viewport, false, Vec::new(), |ui, ctx| {
                    neutral_gamepad_poll(&mut app, ctx);
                    app.begin_page_turn_viewer_frame(ctx, root, None);
                    let (nav, close, _) = actual_pointer_handler(&mut app, ui, ctx);
                    assert!(nav.is_none());
                    assert!(!close);
                    draw_current(&mut app, ui, ctx);
                });
                assert_eq!(tracker.status(), SubmissionStatus::Started);
            }
            if let PresentationState::Turning { clock, offer, .. } =
                &mut app.page_turn_presentation.state
            {
                *clock = MotionClock::Running {
                    start_at: Instant::now() - offer.duration - Duration::from_millis(1),
                };
            } else {
                panic!("an unfocused level without a loss edge must preserve the turn");
            }
            viewer_frame_with_focus(&ctx, viewport, false, Vec::new(), |ui, ctx| {
                app.begin_page_turn_viewer_frame(ctx, root, None);
                let (_, sources) = draw_current(&mut app, ui, ctx);
                let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                    panic!("paged output")
                };
                assert!(!receipt.curl);
            });
            assert_eq!(tracker.status(), SubmissionStatus::Finished);
            assert_completed_operation_events(operation);
        }
    }
}

#[test]
fn real_focus_loss_edge_cancels_the_existing_turn_once() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    take_test_events();
    let ctx = egui::Context::default();
    let (mut app, viewport) = ready_wheel_viewer(&ctx, false, SpreadMode::Single);
    let mut operation = None;
    viewer_frame(
        &ctx,
        viewport,
        raw_wheel(egui::vec2(0.0, -120.0)),
        |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            operation = Some(accept_actual_wheel(&mut app, ui, ctx));
            draw_current(&mut app, ui, ctx);
        },
    );
    let tracker = match &app.page_turn_presentation.state {
        PresentationState::Turning { offer, .. } => offer.submission.clone(),
        _ => panic!("focused wheel starts"),
    };
    viewer_frame_with_focus(
        &ctx,
        viewport,
        false,
        vec![egui::Event::WindowFocused(false)],
        |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            assert!(app.page_turn_completion_active());
            draw_current(&mut app, ui, ctx);
        },
    );
    assert_eq!(tracker.status(), SubmissionStatus::Cancelled);
    for _ in 0..2 {
        viewer_frame_with_focus(&ctx, viewport, false, Vec::new(), |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            draw_current(&mut app, ui, ctx);
        });
    }
    let events = take_test_events();
    assert_eq!(
        events
            .iter()
            .filter(|(fact, kind, _)| Some(fact.operation_id) == operation && *kind == "cancel")
            .count(),
        1,
        "the loss delivery cancels only once: {events:?}"
    );
}

#[test]
fn unfocused_focus_edge_and_wheel_in_either_raw_order_retain_the_wheel_fact() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for loss_first in [false, true] {
        take_test_events();
        let ctx = egui::Context::default();
        let (mut app, viewport) = ready_wheel_viewer(&ctx, false, SpreadMode::Single);
        let mut input = raw_wheel(egui::vec2(0.0, -120.0));
        if loss_first {
            input.insert(0, egui::Event::WindowFocused(false));
        } else {
            input.push(egui::Event::WindowFocused(false));
        }
        viewer_frame_with_focus(&ctx, viewport, false, input, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            let operation = accept_actual_wheel(&mut app, ui, ctx);
            let fact = app.page_turn_presentation.transaction.facts[0];
            assert_eq!(fact.operation_id, operation);
            assert_eq!(fact.source, viewport);
            assert_eq!(fact.phase, OperationPhase::Press);
            assert_eq!(fact.cause, "wheel");
            draw_current(&mut app, ui, ctx);
        });
        viewer_frame_with_focus(&ctx, viewport, false, Vec::new(), |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            draw_current(&mut app, ui, ctx);
        });
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Turning { .. }
        ));
    }
}

#[test]
fn unfocused_wheel_continuation_commits_after_adopted_flat_without_keyboard_permits() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    take_test_events();
    let ctx = egui::Context::default();
    let (mut app, viewport) = ready_wheel_viewer(&ctx, false, SpreadMode::Single);
    viewer_frame_with_focus(
        &ctx,
        viewport,
        false,
        raw_wheel(egui::vec2(0.0, -120.0)),
        |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            accept_actual_wheel(&mut app, ui, ctx);
            draw_current(&mut app, ui, ctx);
        },
    );
    assert_eq!(app.fullscreen_idx, Some(1));
    let mut operation = None;
    let mut canonical = None;
    viewer_frame_with_focus(
        &ctx,
        viewport,
        false,
        raw_wheel(egui::vec2(0.0, -120.0)),
        |ui, ctx| {
            let permits = crate::keyboard_input::keyboard_input_permits(ctx);
            assert!(permits.discrete.is_none() && permits.current_level.is_none());
            app.begin_page_turn_viewer_frame(ctx, false, None);
            operation = Some(accept_actual_wheel(&mut app, ui, ctx));
            assert_eq!(
                app.fullscreen_idx,
                Some(1),
                "the canonical flat precedes navigation"
            );
            let PresentationState::Completing {
                continuation: Some(command),
                ..
            } = &app.page_turn_presentation.state
            else {
                panic!("the second real wheel must own the continuation")
            };
            assert_eq!(Some(command.fact.operation_id), operation);
            assert_eq!(command.fact.source, viewport);
            let (composition, sources) = draw_current(&mut app, ui, ctx);
            let FsDisplayUnitSubmission::Paged(receipt) = &sources.submission else {
                panic!("paged canonical output")
            };
            assert!(!receipt.curl);
            canonical = Some(receipt.clone());
            assert!(app.fs_navigation_sequence_blocks_new_target());
            // Normal rendering publishes this receipt to the navigation observer.
            // Drawing the flat alone cannot retire the previous accepted target.
            app.emit_fs_page_turn_ready_for_display_unit(
                ctx,
                &composition,
                FsPageTurnDecision::normal(),
                &sources,
            );
            assert!(!app.commit_page_turn_continuation(ctx));
        },
    );
    assert!(canonical.unwrap().was_accepted());
    viewer_frame_with_focus(&ctx, viewport, false, Vec::new(), |ui, ctx| {
        let permits = crate::keyboard_input::keyboard_input_permits(ctx);
        assert!(permits.discrete.is_none() && permits.current_level.is_none());
        let entry = app.begin_page_turn_viewer_frame(ctx, false, None);
        assert!(matches!(
            entry,
            PageTurnFrameEntry::Continue {
                committed: true,
                ..
            }
        ));
        assert_eq!(app.fullscreen_idx, Some(2));
        draw_current(&mut app, ui, ctx);
    });
    viewer_frame_with_focus(&ctx, viewport, false, Vec::new(), |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        assert_eq!(app.fullscreen_idx, Some(2));
        draw_current(&mut app, ui, ctx);
    });
    let events = take_test_events();
    let committed: Vec<_> = events
        .iter()
        .filter(|(_, kind, reason)| *kind == "skip" && *reason == "stage1_continuation_instant")
        .collect();
    assert_eq!(
        committed.len(),
        1,
        "one already-resolved continuation: {events:?}"
    );
    assert_eq!(Some(committed[0].0.operation_id), operation);
    assert_eq!(committed[0].0.source, viewport);
    assert_eq!(committed[0].0.cause, "wheel");
}

#[test]
fn horizontal_wheel_and_sibling_delivery_do_not_turn_the_owner_page() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    take_test_events();
    let ctx = egui::Context::default();
    let (mut app, viewport) = ready_wheel_viewer(&ctx, false, SpreadMode::Single);
    viewer_frame_with_focus(
        &ctx,
        viewport,
        false,
        raw_wheel(egui::vec2(-120.0, 0.0)),
        |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            assert!(
                app.page_turn_pointer_navigation_operation(ctx, "wheel")
                    .is_some()
            );
            let (nav, close, _) = actual_pointer_handler(&mut app, ui, ctx);
            assert!(nav.is_none());
            assert!(!close);
            assert_eq!(app.fullscreen_idx, Some(0));
            draw_current(&mut app, ui, ctx);
        },
    );
    let sibling = egui::ViewportId::from_hash_of("unfocused-wheel-sibling");
    viewer_frame_with_focus(
        &ctx,
        sibling,
        false,
        raw_wheel(egui::vec2(0.0, -120.0)),
        |_, ctx| {
            observe_pointer_transaction(ctx);
            let id = egui::Id::unique(("page_turn_pointer_transaction", sibling));
            let transaction = ctx
                .data(|data| data.get_temp::<PointerTransaction>(id))
                .unwrap();
            assert_eq!(transaction.facts.len(), 1);
            let operation = transaction.facts[0].0.id().unwrap();
            assert!(
                matches!(operation, ViewerOperationId::Pointer { source, .. } if source == sibling)
            );
            let destination = app.page_turn_destination();
            let delivery = transaction.facts[0].0.route(destination).unwrap();
            let mut other = destination;
            other.viewport = sibling;
            assert!(
                delivery.route(other).is_none(),
                "a delivered operation cannot be rebound"
            );
        },
    );
    viewer_frame_with_focus(&ctx, viewport, false, Vec::new(), |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        assert!(
            app.page_turn_pointer_navigation_operation(ctx, "wheel")
                .is_none()
        );
        let (nav, close, _) = actual_pointer_handler(&mut app, ui, ctx);
        assert!(nav.is_none());
        assert!(!close);
        assert_eq!(app.fullscreen_idx, Some(0));
        draw_current(&mut app, ui, ctx);
    });
    assert!(
        !take_test_events()
            .iter()
            .any(|(_, kind, _)| *kind == "start")
    );
}

#[test]
fn replayed_focus_loss_does_not_cancel_the_wheel_accepted_after_that_delivery() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    take_test_events();
    let ctx = egui::Context::default();
    let (mut app, viewport) = ready_wheel_viewer(&ctx, false, SpreadMode::Single);
    let mut input = raw_wheel(egui::vec2(0.0, -120.0));
    input.insert(0, egui::Event::WindowFocused(false));
    let mut operation = None;
    let mut passes = 0;
    viewer_frame_with_focus(&ctx, viewport, false, input, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        if operation.is_none() {
            operation = Some(accept_actual_wheel(&mut app, ui, ctx));
        }
        assert_eq!(app.fullscreen_idx, Some(1));
        let (_, sources) = draw_current(&mut app, ui, ctx);
        let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
            panic!("paged output")
        };
        assert!(
            receipt.curl,
            "replay cannot apply the same focus-loss delivery again"
        );
        passes += 1;
        if passes == 1 {
            ctx.request_discard("replay the unfocused wheel delivery");
        }
    });
    assert!(passes >= 2, "the fixture must execute an egui replay");
    let tracker = match &app.page_turn_presentation.state {
        PresentationState::Turning { offer, .. } => {
            assert_eq!(Some(offer.fact.operation_id), operation);
            offer.submission.clone()
        }
        _ => panic!("the adopted replay must retain the new turn"),
    };
    assert_eq!(tracker.status(), SubmissionStatus::Started);
    let events = take_test_events();
    assert_eq!(
        events
            .iter()
            .filter(|(fact, kind, _)| Some(fact.operation_id) == operation && *kind == "start")
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|(fact, kind, _)| Some(fact.operation_id) == operation && *kind == "cancel")
    );
}

#[test]
fn root_focus_loss_forwarding_cannot_cancel_the_child_turn() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    take_test_events();
    let ctx = egui::Context::default();
    let root_ctx = egui::Context::default();
    install(&root_ctx);
    let (mut app, viewport) = ready_wheel_viewer(&ctx, false, SpreadMode::Single);
    viewer_frame(
        &ctx,
        viewport,
        raw_wheel(egui::vec2(0.0, -120.0)),
        |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            accept_actual_wheel(&mut app, ui, ctx);
            draw_current(&mut app, ui, ctx);
        },
    );
    let tracker = match &app.page_turn_presentation.state {
        PresentationState::Turning { offer, .. } => offer.submission.clone(),
        _ => panic!("the child must own its started turn"),
    };
    viewer_frame_with_focus(
        &root_ctx,
        egui::ViewportId::ROOT,
        false,
        vec![egui::Event::WindowFocused(false)],
        |_, root| {
            app.handle_fullscreen_root_key_input(root);
            assert_eq!(tracker.status(), SubmissionStatus::Started);
            let input = FullscreenRootFrameInput {
                context: root.clone(),
                destination: app.page_turn_destination(),
            };
            viewer_frame_with_focus(&ctx, viewport, false, Vec::new(), |ui, ctx| {
                app.begin_page_turn_viewer_frame(ctx, false, Some(&input));
                let (_, sources) = draw_current(&mut app, ui, ctx);
                let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                    panic!("child paged output")
                };
                assert!(receipt.curl);
            });
        },
    );
    assert_eq!(tracker.status(), SubmissionStatus::Started);
    assert!(
        !take_test_events()
            .iter()
            .any(|(_, kind, _)| *kind == "cancel")
    );
}

#[cfg(windows)]
#[test]
fn mounted_sibling_focus_loss_cancels_only_its_own_turn() {
    let _serial = crate::key_input::lock_test_input();
    crate::key_input::clear_test_frame();
    let origin = Instant::now();
    let motion_clock = super::clock_tests::TestClock::at(origin);
    take_test_events();
    let ctx = egui::Context::default();
    let (mut app, main_viewport) = ready_wheel_viewer(&ctx, true, SpreadMode::Single);
    viewer_frame(
        &ctx,
        main_viewport,
        raw_wheel(egui::vec2(0.0, -120.0)),
        |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, true, None);
            accept_actual_wheel(&mut app, ui, ctx);
            draw_current(&mut app, ui, ctx);
        },
    );
    let (main_operation, main_tracker) = match &app.page_turn_presentation.state {
        PresentationState::Turning { offer, .. } => {
            (offer.fact.operation_id, offer.submission.clone())
        }
        _ => panic!("main started turn"),
    };
    let main_context = app.page_turn_destination().context;
    assert_eq!(main_tracker.started_at(), Some(origin));
    motion_clock.set(origin + Duration::from_millis(50));
    let items = app.items.clone();
    let sibling = app.build_window_context_for_test(13419, |owner| {
        owner.items = items;
        owner.visible_indices = vec![0, 1, 2, 3];
        owner.fullscreen_idx = Some(0);
        owner.spread_mode = SpreadMode::Single;
        owner.reading_flow = ReadingFlow::Paged;
        owner.viewer_presentation = crate::app::ViewerPresentation::DetachedWindow;
        owner.native_video_in_window_active = false;
        owner.transition_detached_window_state(
            13419,
            crate::app::DetachedWindowState::Active,
            "focus_loss_owner_test",
        );
        for idx in 0..4 {
            let image = egui::ColorImage::filled([80, 120], egui::Color32::LIGHT_BLUE);
            let tex = ctx.load_texture(
                format!("focus-sibling-{idx}"),
                image.clone(),
                egui::TextureOptions::LINEAR,
            );
            owner.fs_cache.insert(
                idx,
                FsCacheEntry::Static {
                    tex,
                    pixels: Arc::new(image),
                    source_dims: Some([80, 120]),
                    load_seq: 1,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
            owner.record_page_dims_for_spread(idx, (80, 120));
        }
    });
    assert_eq!(main_tracker.status(), SubmissionStatus::Started);
    let mut sibling_operation = None;
    app.with_window_context_for_test(sibling, |owner| {
        let destination = owner.page_turn_destination();
        assert_eq!(destination.context, sibling);
        assert_ne!(destination.context, main_context);
        assert_ne!(destination.viewport, main_viewport);
        let viewport = destination.viewport;
        for anchor in [1, 0] {
            owner.fullscreen_idx = Some(anchor);
            viewer_frame(&ctx, viewport, Vec::new(), |ui, ctx| {
                owner.begin_page_turn_viewer_frame(ctx, false, None);
                draw_current(owner, ui, ctx);
            });
        }
        viewer_frame(
            &ctx,
            viewport,
            raw_wheel(egui::vec2(0.0, -120.0)),
            |ui, ctx| {
                owner.begin_page_turn_viewer_frame(ctx, false, None);
                sibling_operation = Some(accept_actual_wheel(owner, ui, ctx));
                draw_current(owner, ui, ctx);
            },
        );
        let tracker = match &owner.page_turn_presentation.state {
            PresentationState::Turning { offer, .. } => offer.submission.clone(),
            _ => panic!("sibling started turn"),
        };
        assert_eq!(
            tracker.started_at(),
            Some(origin + Duration::from_millis(50))
        );
        assert_eq!(main_tracker.started_at(), Some(origin));
        viewer_frame_with_focus(
            &ctx,
            viewport,
            false,
            vec![egui::Event::WindowFocused(false)],
            |ui, ctx| {
                owner.begin_page_turn_viewer_frame(ctx, false, None);
                draw_current(owner, ui, ctx);
            },
        );
        assert_eq!(tracker.status(), SubmissionStatus::Cancelled);
        assert_eq!(main_tracker.status(), SubmissionStatus::Started);
    });
    assert_eq!(app.page_turn_destination().context, main_context);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(main_tracker.status(), SubmissionStatus::Started);
    motion_clock.set(origin + Duration::from_millis(100));
    viewer_frame(&ctx, main_viewport, Vec::new(), |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, true, None);
        draw_current(&mut app, ui, ctx);
    });
    let PresentationState::Turning { clock, .. } = &app.page_turn_presentation.state else {
        panic!("main turn survives sibling mount/deposit");
    };
    assert_eq!(*clock, MotionClock::Running { start_at: origin });
    assert_eq!(main_tracker.started_at(), Some(origin));
    let events = take_test_events();
    assert_eq!(
        events
            .iter()
            .filter(
                |(fact, kind, _)| Some(fact.operation_id) == sibling_operation && *kind == "cancel"
            )
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|(fact, kind, _)| fact.operation_id == main_operation && *kind == "cancel")
    );
}

fn accepted_unfocused_nonpointer_continuation_commits_once(native_key: bool) {
    take_test_events();
    let ctx = egui::Context::default();
    let (mut app, viewport) = ready_wheel_viewer(&ctx, false, SpreadMode::Single);
    viewer_frame_with_focus(
        &ctx,
        viewport,
        false,
        raw_wheel(egui::vec2(0.0, -120.0)),
        |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            accept_actual_wheel(&mut app, ui, ctx);
            draw_current(&mut app, ui, ctx);
        },
    );
    let events = if native_key {
        vec![key_event(true), key_event(false)]
    } else {
        Vec::new()
    };
    let mut operation = None;
    let mut receipt = None;
    viewer_frame_with_focus(&ctx, viewport, false, events, |ui, ctx| {
        #[cfg(windows)]
        if native_key {
            install_native_press(viewport);
        }
        app.begin_page_turn_viewer_frame(ctx, false, None);
        if native_key {
            let mut id = None;
            let action = app.handle_fs_key_input_with_operation(ctx, 1, false, &mut id);
            assert!(!action.page_nav.is_none());
            assert!(matches!(id, Some(ViewerOperationId::Key(_))));
            accept_key_action_on_outer_root(&mut app, ctx, 1, &action, id);
        } else {
            // This producer creates the accepted timer delivery and uses the
            // same canonical-flat navigation owner as an elapsed slideshow.
            app.open_fullscreen_from_slideshow_navigation(ctx, 2);
        }
        let PresentationState::Completing {
            continuation: Some(command),
            ..
        } = &app.page_turn_presentation.state
        else {
            panic!("an accepted native-key/slideshow command must be retained")
        };
        operation = Some(command.fact.operation_id);
        assert_eq!(command.fact.source, viewport);
        assert_eq!(
            command.fact.cause,
            if native_key { "key" } else { "slideshow" }
        );
        assert_eq!(app.fullscreen_idx, Some(1));
        let (composition, sources) = draw_current(&mut app, ui, ctx);
        let FsDisplayUnitSubmission::Paged(flat) = &sources.submission else {
            panic!("canonical flat")
        };
        assert!(!flat.curl);
        receipt = Some(flat.clone());
        app.emit_fs_page_turn_ready_for_display_unit(
            ctx,
            &composition,
            FsPageTurnDecision::normal(),
            &sources,
        );
        assert!(!app.commit_page_turn_continuation(ctx));
    });
    assert!(receipt.unwrap().was_accepted());
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for first in [true, false] {
        viewer_frame_with_focus(&ctx, viewport, false, Vec::new(), |ui, ctx| {
            let permits = crate::keyboard_input::keyboard_input_permits(ctx);
            assert!(permits.discrete.is_none() && permits.current_level.is_none());
            let entry = app.begin_page_turn_viewer_frame(ctx, false, None);
            assert!(
                matches!(entry, PageTurnFrameEntry::Continue { committed, .. } if committed == first)
            );
            assert_eq!(app.fullscreen_idx, Some(2));
            draw_current(&mut app, ui, ctx);
        });
    }
    let events = take_test_events();
    assert_eq!(
        events
            .iter()
            .filter(|(fact, kind, reason)| Some(fact.operation_id) == operation
                && *kind == "skip"
                && *reason == "stage1_continuation_instant")
            .count(),
        1
    );
}

#[test]
fn accepted_slideshow_continuation_commits_once_in_an_unfocused_frame() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    accepted_unfocused_nonpointer_continuation_commits_once(false);
}

#[cfg(windows)]
#[test]
fn accepted_native_key_continuation_commits_once_after_its_discrete_permit_expires() {
    let _serial = crate::key_input::lock_test_input();
    crate::key_input::clear_test_frame();
    accepted_unfocused_nonpointer_continuation_commits_once(true);
}
