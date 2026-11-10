//! Real ordered canvas input and paint passes; generated data, no product window.
use super::*;
use crate::app::depth_main_window_tests::RootDepthViewer;
use crate::depth_parallax::{DepthSourcePhase, DepthSourceWait, DepthViewState};
use crate::fs_exclusive_view::FsExclusiveView;

fn pointer(pos: egui::Pos2, pressed: bool, ctrl: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: if ctrl {
            egui::Modifiers::CTRL
        } else {
            egui::Modifiers::NONE
        },
    }
}

#[test]
fn depth_focus_canvas_click_uses_release_modifiers_and_preserves_prior_exclusions() {
    let _input = crate::key_input::lock_test_input();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800., 600.));
    let pos = egui::pos2(400., 300.);
    for depth in [false, true] {
        for ctrl in [false, true] {
            for exclusion in ["none", "suppressed", "popup", "overflow", "panel", "double"] {
                let ctx = egui::Context::default();
                let mut app = crate::app::setup_app_for_test();
                app.items = vec![GridItem::Image("focus-click.jpg".into())];
                app.fullscreen_idx = Some(0);
                if depth {
                    app.fs_exclusive_view = FsExclusiveView::Depth(DepthViewState::new(
                        app.edit_request_owner_context(),
                    ));
                }
                app.fit_popup_open = exclusion == "popup";
                if exclusion == "overflow" {
                    app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
                }
                if exclusion == "panel" && depth {
                    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("depth_panel_rect"), rect));
                }
                let mut clicked = false;
                for frame in 0..if exclusion == "double" { 5 } else { 3 } {
                    let mut events = vec![egui::Event::PointerMoved(pos)];
                    if frame > 0 {
                        events.push(pointer(pos, frame % 2 == 1, ctrl));
                    }
                    events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
                    ctx.run_ui(egui::RawInput { screen_rect: Some(rect), events,
                        // Ctrl is already up in the current input level.
                        time: Some(frame as f64 * 0.05), ..Default::default() }, |ui| {
                        app.capture_fs_front_pointer_input(&ctx, rect);
                        let response = ui.interact(rect, egui::Id::unique("focus-canvas"), egui::Sense::click_and_drag());
                        app.fs_front_pointer.accept_backing_click(&response, false);
                        let action = app.plan_fs_canvas_click(&ctx, &response, rect, StillSeekGeometry::bar_only(),
                            false, exclusion == "suppressed", false, false, None, false, 1., egui::Vec2::ZERO);
                        if response.clicked() {
                            clicked = true;
                            if exclusion == "suppressed" || exclusion == "popup" || exclusion == "panel" && depth {
                                assert!(matches!(action, FsCanvasClickAction::None));
                            } else if exclusion == "overflow" {
                                assert!(matches!(action, FsCanvasClickAction::DismissOverflow));
                            } else if depth && ctrl {
                                assert!(matches!(action, FsCanvasClickAction::DepthFocusAt(point) if point == pos));
                            } else {
                                assert!(matches!(action, FsCanvasClickAction::PageSide(_)));
                            }
                        }
                    }).drop_without_applying_deltas();
                }
                assert!(clicked, "{exclusion}");
            }
        }
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_focus_batched_clicks_use_the_last_primary_release_after_pointer_move() {
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800., 600.));
    let pos = rect.center();
    for ctrl in [false, true] {
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        app.fs_exclusive_view =
            FsExclusiveView::Depth(DepthViewState::new(app.edit_request_owner_context()));
        for click in [false, true] {
            let mut events = if click {
                vec![
                    egui::Event::PointerMoved(pos),
                    pointer(pos, true, !ctrl),
                    pointer(pos, false, !ctrl),
                    pointer(pos, true, ctrl),
                    pointer(pos, false, ctrl),
                    egui::Event::ModifiersChanged(egui::Modifiers::NONE),
                    egui::Event::PointerMoved(pos + egui::vec2(20., 0.)),
                ]
            } else {
                vec![egui::Event::PointerMoved(pos)]
            };
            if click
                && ctrl
                && let egui::Event::PointerButton { modifiers, .. } = &mut events[4]
            {
                modifiers.shift = true;
                modifiers.alt = true;
            }
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let response = ui.interact(
                        rect,
                        egui::Id::unique("batched-focus"),
                        egui::Sense::click_and_drag(),
                    );
                    if click {
                        assert!(response.clicked());
                        let action = app.plan_fs_canvas_click(
                            &ctx,
                            &response,
                            rect,
                            StillSeekGeometry::bar_only(),
                            false,
                            false,
                            false,
                            false,
                            None,
                            false,
                            1.,
                            egui::Vec2::ZERO,
                        );
                        assert_eq!(matches!(action, FsCanvasClickAction::DepthFocusAt(_)), ctrl);
                        if ctrl { assert!(matches!(action, FsCanvasClickAction::DepthFocusAt(point) if point == pos)); }
                        assert_eq!(matches!(action, FsCanvasClickAction::PageSide(_)), !ctrl);
                    }
                },
            )
            .drop_without_applying_deltas();
        }
    }
}

#[test]
fn depth_focus_touch_panel_is_excluded_and_ctrl_taps_outside_still_turn_pages() {
    let _input = crate::key_input::lock_test_input();
    for in_panel in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        viewer.ready(&mut app);
        app.window_visible = false;
        app.settings.touch_still_chrome_learned = true;
        app.settings.depth_parallax.focus = 0.2;
        app.items[1] = GridItem::Image("touch-next.jpg".into());
        crate::app::source_texture_test_support::seed(&mut app, &viewer.ctx, 1);
        viewer.frame(&mut app, vec![]);
        app.fs_focus_regained_at = None;
        let panel = viewer
            .ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::unique("depth_panel_rect")))
            .unwrap();
        let pos = if in_panel {
            panel.center_bottom() - egui::vec2(0., 10.)
        } else {
            egui::pos2(650., 500.)
        };
        for phase in [egui::TouchPhase::Start, egui::TouchPhase::End] {
            let mut events = vec![egui::Event::Touch {
                device_id: egui::TouchDeviceId(751),
                id: egui::TouchId(1),
                phase,
                pos,
                force: None,
            }];
            if phase == egui::TouchPhase::Start {
                events.push(egui::Event::PointerMoved(pos));
            }
            events.push(pointer(pos, phase == egui::TouchPhase::Start, true));
            if phase == egui::TouchPhase::End {
                events.push(egui::Event::PointerGone);
            }
            viewer.frame(&mut app, events);
        }
        assert_eq!(app.fullscreen_idx, Some(if in_panel { 0 } else { 1 }));
        assert_eq!(app.settings.depth_parallax.focus, 0.2);
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_focus_modal_remote_and_mouse_drag_keep_canvas_unmodified() {
    let _input = crate::key_input::lock_test_input();
    for case in ["modal", "remote", "drag"] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        viewer.ready(&mut app);
        viewer.frame(&mut app, vec![]);
        app.settings.depth_parallax.focus = 0.2;
        app.fs_focus_regained_at = None;
        if case == "modal" {
            let capture = app.capture_depth_export(&viewer.ctx, false).unwrap();
            app.admit_depth_export(&viewer.ctx, capture);
            assert!(app.depth_export_operation.is_some());
        } else if case == "remote" {
            let handle = crate::remote_ipc::session::SessionHandle::new();
            app.set_remote_session_handle(handle.clone());
            let result = handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
                client_id: "focus-test".into(),
                peer: mimageviewer_ipc::SessionPeerInfo {
                    connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                    device_name: None,
                },
            });
            assert_eq!(result.status, mimageviewer_ipc::SessionStatus::Active);
        }
        let start = egui::pos2(550., 300.);
        viewer.frame(
            &mut app,
            vec![egui::Event::PointerMoved(start), pointer(start, true, true)],
        );
        let end = if case == "drag" {
            start + egui::vec2(20., 0.)
        } else {
            start
        };
        viewer.frame(&mut app, vec![egui::Event::PointerMoved(end)]);
        viewer.frame(&mut app, vec![pointer(end, false, true)]);
        assert_eq!(app.settings.depth_parallax.focus, 0.2, "{case}");
        assert_eq!(app.fullscreen_idx, Some(0), "{case}");
        if let FsExclusiveView::Depth(state) = &app.fs_exclusive_view {
            assert!(state.focus_highlight.is_none(), "{case}");
            assert!(state.pending_focus_click.is_none(), "{case}");
        }
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_focus_highlight_repaints_stationary_mouse_but_not_hidden_minimized_or_export_modal() {
    let _input = crate::key_input::lock_test_input();
    for case in ["visible", "hidden", "minimized", "export"] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        viewer.ready(&mut app);
        viewer.frame(&mut app, vec![]);
        let transform = app
            .fullscreen_page_layout
            .single_page()
            .unwrap()
            .transform
            .clone();
        let payload = FullscreenPaintPayload::from(
            app.ensure_final_composite_material(&viewer.ctx, 0).unwrap(),
        );
        if case == "export" {
            let capture = app.capture_depth_export(&viewer.ctx, false).unwrap();
            app.admit_depth_export(&viewer.ctx, capture);
            assert!(app.depth_export_operation.is_some());
        }
        app.window_visible = case != "hidden";
        let mut delay = std::time::Duration::ZERO;
        for _ in 0..8 {
            let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
                unreachable!()
            };
            state.focus_highlight = Some(std::time::Instant::now());
            state.mouse.current = egui::Vec2::ZERO;
            state.mouse.target = egui::Vec2::ZERO;
            let mut input = egui::RawInput::default();
            input
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .minimized = Some(case == "minimized");
            let output = viewer.ctx.run_ui(input, |ui| {
                assert!(app.try_paint_depth(
                    ui.painter(),
                    &viewer.ctx,
                    &payload,
                    &transform,
                    transform.viewport_rect,
                    &FsBgStyle::Solid(egui::Color32::BLACK)
                ));
            });
            delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
            output.drop_without_applying_deltas();
        }
        if case == "visible" {
            assert_eq!(delay, std::time::Duration::ZERO);
        } else {
            assert!(
                delay > std::time::Duration::from_secs(1),
                "{case}: {delay:?}"
            );
        }
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_focus_click_pending_failed_and_original_hold_never_navigate() {
    let _input = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    app.items = vec![GridItem::Image("focus-state.jpg".into())];
    app.fullscreen_idx = Some(0);
    let key = crate::depth_estimation::DepthSourceKey {
        texture_id: egui::TextureId::Managed(500),
    };
    for phase in ["waiting", "estimating", "failed", "original"] {
        let mut state = DepthViewState::new(app.edit_request_owner_context());
        match phase {
            "failed" => {
                state.source = DepthSourcePhase::Failed {
                    key,
                    reason: "fixture".into(),
                }
            }
            "original" => state.wait_for_source(DepthSourceWait::OriginalHold),
            "estimating" => {
                let (pending, _sender) = crate::depth_estimation::DepthPending::deferred_for_test(
                    state.context,
                    501,
                    key,
                );
                state.source = DepthSourcePhase::Estimating { key, pending };
            }
            _ => {}
        }
        app.fs_exclusive_view = FsExclusiveView::Depth(state);
        app.feedback_toasts_for_test.clear();
        app.plan_depth_focus_click(&ctx, egui::pos2(200., 200.));
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(app.settings.depth_parallax.focus, 0.5);
        assert_eq!(
            app.feedback_toasts_for_test.len(),
            usize::from(matches!(phase, "waiting" | "estimating")),
            "{phase}"
        );
        if let FsExclusiveView::Depth(state) = &app.fs_exclusive_view {
            assert!(state.pending_focus_click.is_none());
        }
    }
    let viewer = RootDepthViewer::new(&mut app);
    viewer.ready(&mut app);
    let render = app.wgpu_render_state.clone().unwrap();
    let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
        unreachable!()
    };
    let key = state.ready().unwrap().key;
    let backing = render
        .renderer
        .read()
        .texture(&key.texture_id)
        .unwrap()
        .texture
        .clone()
        .unwrap();
    state.wait_for_source(DepthSourceWait::TexturePending);
    state.lru = Default::default();
    state
        .select_source(&render.device, &render.queue, &backing, key)
        .unwrap();
    assert!(matches!(state.source, DepthSourcePhase::ReadingBack { .. }));
    app.feedback_toasts_for_test.clear();
    app.plan_depth_focus_click(&viewer.ctx, egui::pos2(200., 200.));
    assert_eq!(app.feedback_toasts_for_test.len(), 1);
    assert_eq!(app.fullscreen_idx, Some(0));
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_focus_real_presentations_register_before_paint_with_deferred_wheel_and_discard() {
    std::thread::spawn(|| {
        let _input = crate::key_input::lock_test_input();
        for fullscreen in [false, true] {
            for mixed in [false, true] {
                for discard in [false, true] {
                    let mut app = crate::app::setup_app_for_test();
                    let viewer = RootDepthViewer::new(&mut app);
                    viewer.ready(&mut app);
                    let ctx = viewer.ctx;
                    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, viewer.size);
                    app.items[1] = GridItem::Image("focus-next.jpg".into());
                    crate::app::source_texture_test_support::seed(&mut app, &ctx, 1);
                    app.settings.touch_still_chrome_learned = true;
                    app.window_visible = false; // Keep resource selection fixed; no model starts.
                    app.settings.depth_parallax.focus = 0.2;
                    if fullscreen {
                        app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
                        app.native_video_in_window_active = false;
                        app.settings.video_in_window_mode = false;
                    }
                    let child_events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
                    let child_queue = child_events.clone();
                    let request_discard = std::rc::Rc::new(std::cell::Cell::new(false));
                    let child_discard = request_discard.clone();
                    let observed = std::rc::Rc::new(std::cell::Cell::new(0));
                    let child_observed = observed.clone();
                    ctx.set_embed_viewports(false);
                    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
                        ctx.run_ui(
                            egui::RawInput {
                                viewport_id: viewport.ids.this,
                                screen_rect: Some(rect),
                                events: std::mem::take(&mut *child_queue.borrow_mut()),
                                viewports: [(
                                    viewport.ids.this,
                                    egui::ViewportInfo {
                                        parent: Some(viewport.ids.parent),
                                        focused: Some(true),
                                        ..Default::default()
                                    },
                                )]
                                .into_iter()
                                .collect(),
                                ..Default::default()
                            },
                            |ui| {
                                (viewport.viewport_ui_cb)(ui);
                                child_observed.set(child_observed.get() + 1);
                                if child_discard.replace(false) {
                                    ctx.request_discard("focus child input replay");
                                }
                            },
                        )
                        .drop_without_applying_deltas();
                    });
                    let frame = |app: &mut App, events: Vec<egui::Event>, retry: bool| {
                        app.frame_counter += 1;
                        let root_events = if fullscreen {
                            *child_events.borrow_mut() = events;
                            vec![]
                        } else {
                            events
                        };
                        request_discard.set(fullscreen && retry);
                        let mut root_retry = !fullscreen && retry;
                        let mut passes = 0;
                        ctx.run_ui(
                            egui::RawInput {
                                screen_rect: Some(rect),
                                events: root_events,
                                time: Some(app.frame_counter as f64 / 60.),
                                ..Default::default()
                            },
                            |ui| {
                                app.render_fullscreen_viewport(ui);
                                passes += 1;
                                if root_retry {
                                    root_retry = false;
                                    ctx.request_discard("focus root input replay");
                                }
                            },
                        )
                        .drop_without_applying_deltas();
                        passes
                    };
                    frame(&mut app, vec![], false);
                    app.fs_focus_regained_at = None;
                    let pos = rect.center();
                    frame(
                        &mut app,
                        vec![egui::Event::PointerMoved(pos), pointer(pos, true, true)],
                        false,
                    );
                    let mut events = vec![
                        pointer(pos, false, true),
                        egui::Event::ModifiersChanged(egui::Modifiers::NONE),
                        egui::Event::PointerMoved(pos + egui::vec2(20., 0.)),
                    ];
                    if mixed {
                        for (y, modifiers) in [
                            (-120., egui::Modifiers::NONE),
                            (120., egui::Modifiers::CTRL),
                        ] {
                            events.push(egui::Event::MouseWheel {
                                unit: egui::MouseWheelUnit::Point,
                                delta: egui::vec2(0., y),
                                modifiers,
                                phase: egui::TouchPhase::Move,
                                source: egui::MouseWheelSource::Wheel,
                            });
                        }
                    }
                    let before = observed.get();
                    let passes = frame(&mut app, events, discard);
                    assert_eq!(
                        app.settings.depth_parallax.focus, 0.5,
                        "fullscreen={fullscreen}, mixed={mixed}, discard={discard}"
                    );
                    assert_eq!(app.fullscreen_idx, Some(if mixed { 1 } else { 0 }));
                    let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                        panic!("depth remains active");
                    };
                    assert!(state.focus_highlight.is_some());
                    assert!(
                        state.pending_focus_click.is_none(),
                        "no after-paint request may remain"
                    );
                    if discard {
                        assert!(if fullscreen {
                            observed.get() - before >= 2
                        } else {
                            passes >= 2
                        });
                    }
                }
            }
        }
        crate::key_input::clear_test_frame();
    })
    .join()
    .unwrap();
}

#[test]
fn depth_focus_paint_consumes_only_same_pass_matching_resource_and_current_transform() {
    let _input = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = RootDepthViewer::new(&mut app);
    viewer.ready(&mut app);
    viewer.frame(&mut app, vec![]);
    let transform = app
        .fullscreen_page_layout
        .single_page()
        .unwrap()
        .transform
        .clone();
    let payload =
        FullscreenPaintPayload::from(app.ensure_final_composite_material(&viewer.ctx, 0).unwrap());
    let other =
        FullscreenPaintPayload::from(app.ensure_final_composite_material(&viewer.ctx, 2).unwrap());
    for case in ["same", "old", "mismatch", "outside", "lru"] {
        app.settings.depth_parallax.focus = 0.2;
        if case == "lru" {
            let render = app.wgpu_render_state.clone().unwrap();
            let backing = render
                .renderer
                .read()
                .texture(&payload.source_texture_id())
                .unwrap()
                .texture
                .clone()
                .unwrap();
            let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
                unreachable!()
            };
            let previous = state.ready().unwrap().depth.clone();
            let key = state.ready().unwrap().key;
            state.wait_for_source(DepthSourceWait::OriginalHold);
            state
                .select_source(&render.device, &render.queue, &backing, key)
                .unwrap();
            assert!(Arc::ptr_eq(&previous, &state.ready().unwrap().depth));
        }
        viewer
            .ctx
            .run_ui(egui::RawInput::default(), |ui| {
                let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
                    unreachable!()
                };
                let pass = viewer.ctx.cumulative_pass_nr();
                state.pending_focus_click = Some((
                    if case == "outside" {
                        transform.paint_rect.min - egui::vec2(1., 0.)
                    } else {
                        transform.paint_rect.center()
                    },
                    if case == "old" { pass - 1 } else { pass },
                ));
                let painted = app.try_paint_depth(
                    ui.painter(),
                    &viewer.ctx,
                    if case == "mismatch" { &other } else { &payload },
                    &transform,
                    transform.viewport_rect,
                    &FsBgStyle::Solid(egui::Color32::BLACK),
                );
                assert_eq!(painted, case != "mismatch");
                assert_eq!(
                    app.settings.depth_parallax.focus,
                    if matches!(case, "same" | "lru") {
                        0.5
                    } else {
                        0.2
                    }
                );
                let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                    unreachable!()
                };
                assert!(state.pending_focus_click.is_none());
            })
            .drop_without_applying_deltas();
    }
    // A larger painter clip must not change sensitivity of the media canvas.
    let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800., 400.));
    for clip in [
        canvas,
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2000., 2000.)),
    ] {
        viewer
            .ctx
            .run_ui(
                egui::RawInput {
                    events: vec![egui::Event::PointerMoved(
                        transform.paint_rect.center() - egui::vec2(50., 0.),
                    )],
                    ..Default::default()
                },
                |ui| {
                    app.try_paint_depth(
                        &ui.painter().with_clip_rect(clip),
                        &viewer.ctx,
                        &payload,
                        &transform,
                        canvas,
                        &FsBgStyle::Solid(egui::Color32::BLACK),
                    );
                    let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                        unreachable!()
                    };
                    assert!((state.mouse.target - egui::vec2(0.01, 0.)).length() < 1e-6);
                },
            )
            .drop_without_applying_deltas();
    }
    crate::key_input::clear_test_frame();
}
