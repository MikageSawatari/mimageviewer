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
fn depth_collapsed_panel_next_pass_routes_mouse_ctrl_hover_and_touch_in_both_presentations() {
    // The child renderer is thread-local; neither presentation creates a native window.
    std::thread::spawn(|| {
        let _input = crate::key_input::lock_test_input();
        for presentation in [crate::app::ViewerPresentation::MainWindow,
            crate::app::ViewerPresentation::Fullscreen, crate::app::ViewerPresentation::DetachedWindow] {
            let fullscreen = presentation != crate::app::ViewerPresentation::MainWindow;
            for case in [
                "mouse",
                "ctrl",
                "hover",
                "touch",
                "header_mouse",
                "header_touch",
                "expanded_mouse",
                "expanded_ctrl",
                "expanded_touch",
                "expanded_hover",
            ] {
                let mut app = crate::app::setup_app_for_test();
                let viewer = if presentation == crate::app::ViewerPresentation::DetachedWindow {
                    RootDepthViewer::linked(&mut app)
                } else { RootDepthViewer::new(&mut app) };
                viewer.ready(&mut app);
                let ctx = viewer.ctx;
                assert_eq!(ctx.options(|options| options.max_passes.get()), 2);
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, viewer.size);
                app.rotation_cache
                    .insert(0, crate::rotation_db::Rotation::Cw90);
                app.items[1] = GridItem::Image("collapsed-next.jpg".into());
                crate::app::source_texture_test_support::seed(&mut app, &ctx, 1);
                app.settings.touch_still_chrome_learned = true;
                app.window_visible = false;
                app.settings.depth_parallax.focus = 0.2;
                if fullscreen {
                    app.viewer_presentation = presentation;
                    app.native_video_in_window_active = false;
                    app.settings.video_in_window_mode = false;
                }
                let child_events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
                let queue = child_events.clone();
                let panel = std::rc::Rc::new(std::cell::Cell::new(egui::Rect::NOTHING));
                let child_panel = panel.clone();
                let observed_pass = std::rc::Rc::new(std::cell::Cell::new(0_u64));
                let child_pass = observed_pass.clone();
                let completed_passes = std::rc::Rc::new(std::cell::Cell::new(0));
                let child_completed = completed_passes.clone();
                ctx.set_embed_viewports(false);
                egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
                    let pass = child_pass.get() + 1;
                    child_pass.set(pass);
                    assert_eq!(ctx.options(|options| options.max_passes.get()), 2);
                    let output = ctx.run_pass(
                        egui::RawInput {
                            viewport_id: viewport.ids.this,
                            screen_rect: Some(rect),
                            time: Some(pass as f64 / 60.),
                            events: std::mem::take(&mut *queue.borrow_mut()),
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
                        |ctx| {
                            if ctx.current_pass_index() > 0 {
                                assert!(ctx.input(|input| input.raw.events.is_empty()));
                            }
                            ctx.root_ui(|ui| (viewport.viewport_ui_cb)(ui));
                            child_panel.set(
                                ctx.data(|d| d.get_temp(egui::Id::unique("depth_panel_rect")))
                                    .unwrap(),
                            );
                        },
                    );
                    child_completed.set(output.platform_output.num_completed_passes);
                    output.drop_without_applying_deltas();
                });
                let frame = |app: &mut crate::app::App, events: Vec<egui::Event>| {
                    app.frame_counter += 1;
                    let events = if fullscreen {
                        *child_events.borrow_mut() = events;
                        vec![]
                    } else {
                        events
                    };
                    let output = ctx.run_pass(
                        egui::RawInput {
                            screen_rect: Some(rect),
                            events,
                            time: Some(app.frame_counter as f64 / 60.),
                            viewports: [(
                                egui::ViewportId::ROOT,
                                egui::ViewportInfo {
                                    focused: Some(true),
                                    ..Default::default()
                                },
                            )]
                            .into_iter()
                            .collect(),
                            ..Default::default()
                        },
                        |ctx| {
                            if !fullscreen && ctx.current_pass_index() > 0 {
                                assert!(ctx.input(|input| input.raw.events.is_empty()));
                            }
                            ctx.root_ui(|ui| app.render_fullscreen_viewport(ui));
                            if !fullscreen {
                                panel.set(
                                    ctx.data(|d| d.get_temp(egui::Id::unique("depth_panel_rect")))
                                        .unwrap(),
                                );
                            }
                        },
                    );
                    if !fullscreen {
                        completed_passes.set(output.platform_output.num_completed_passes);
                    }
                    output.drop_without_applying_deltas();
                };
                for _ in 0..3 {
                    frame(&mut app, vec![]);
                }
                app.fs_focus_regained_at = None;
                let expanded = panel.get();
                let header = expanded.min + egui::vec2(60., 18.);
                let body = expanded.min + egui::vec2(25., 125.);
                assert!(
                    app.fullscreen_page_layout
                        .single_page()
                        .unwrap()
                        .transform
                        .paint_rect
                        .contains(body),
                    "focus fixture must sample the image"
                );
                let click = |app: &mut crate::app::App, pos| {
                    frame(
                        app,
                        vec![egui::Event::PointerMoved(pos), pointer(pos, true, false)],
                    );
                    frame(app, vec![pointer(pos, false, false)]);
                };
                let save_before = crate::settings::save_generation();
                click(&mut app, header);
                assert!(
                    app.settings.depth_parallax.panel_collapsed,
                    "{fullscreen}, {case}"
                );
                assert_eq!(
                    app.fullscreen_idx,
                    Some(0),
                    "collapse click must not navigate"
                );
                assert_eq!(crate::settings::save_generation(), save_before + 1);
                assert_eq!(completed_passes.get(), 2, "collapse must rebuild in the same frame");
                assert!(panel.get().height() < 45.);
                if case.starts_with("expanded_") {
                    click(&mut app, header);
                    assert!(!app.settings.depth_parallax.panel_collapsed);
                    assert_eq!(completed_passes.get(), 2, "expand must rebuild in the same frame");
                    assert_eq!(crate::settings::save_generation(), save_before + 2);
                    assert!(panel.get().contains(body));
                }
                let before = crate::settings::save_generation();
                if let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view {
                    state.mouse.target = egui::Vec2::ZERO;
                    state.mouse.current = egui::Vec2::ZERO;
                }
                let pos = if case.starts_with("header_") {
                    header
                } else {
                    body
                };
                let ctrl = case.ends_with("ctrl");
                let touch = case.ends_with("touch");
                if case.ends_with("hover") {
                    // This is the very next pass after the collapsing release.
                    frame(&mut app, vec![egui::Event::PointerMoved(pos)]);
                    let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                        unreachable!()
                    };
                    assert_eq!(
                        state.mouse.target == egui::Vec2::ZERO,
                        case.starts_with("expanded_"),
                        "{fullscreen}, {case}"
                    );
                } else {
                    for pressed in [true, false] {
                        let mut events = vec![];
                        if touch {
                            events.push(egui::Event::Touch {
                                device_id: egui::TouchDeviceId(766),
                                id: egui::TouchId(1),
                                phase: if pressed {
                                    egui::TouchPhase::Start
                                } else {
                                    egui::TouchPhase::End
                                },
                                pos,
                                force: None,
                            });
                        }
                        if pressed {
                            events.push(egui::Event::PointerMoved(pos));
                        }
                        events.push(pointer(pos, pressed, ctrl));
                        if touch && !pressed {
                            events.push(egui::Event::PointerGone);
                        }
                        frame(&mut app, events);
                    }
                    let turns_page = matches!(case, "mouse" | "touch");
                    assert_eq!(
                        app.fullscreen_idx,
                        Some(usize::from(turns_page)),
                        "{fullscreen}, {case}"
                    );
                    assert_eq!(
                        app.settings.depth_parallax.focus,
                        if case == "ctrl" { 0.5 } else { 0.2 },
                        "{fullscreen}, {case}"
                    );
                }
                if case == "hover" {
                    frame(&mut app, vec![]);
                    assert_eq!(
                        crate::settings::save_generation(),
                        before,
                        "repaint must not save"
                    );
                    app.feedback_toasts_for_test.clear();
                    for (key, visible) in [(egui::Key::I, false), (egui::Key::Tab, true)] {
                        for pressed in [true, false] {
                            frame(&mut app, vec![egui::Event::Key {
                                key, physical_key: Some(key), pressed, repeat: false,
                                modifiers: egui::Modifiers::NONE,
                            }]);
                        }
                        assert!(matches!(&app.fs_exclusive_view, FsExclusiveView::Depth(state) if state.panel_visible == visible));
                        assert!(app.settings.depth_parallax.panel_collapsed);
                        assert_eq!(app.feedback_toasts_for_test.len(), 1, "hide once; show silently");
                    }
                    assert!(panel.get().height() < 45.);
                }
            }
        }
        crate::key_input::clear_test_frame();
    })
    .join()
    .unwrap();
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
fn depth_focus_click_saves_once_across_discard_and_highlight_repaints() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let _input = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = RootDepthViewer::new(&mut app);
    viewer.ready(&mut app);
    app.settings.touch_still_chrome_learned = true;
    app.settings.depth_parallax.focus = 0.2;
    for _ in 0..2 {
        viewer.frame(&mut app, vec![]);
    }
    app.fs_focus_regained_at = None;
    // Establish that the isolated real Settings writer lands successfully, so
    // its generation also detects an unwanted second synchronous save.
    let before_save = crate::settings::save_generation();
    app.settings.save();
    assert_eq!(crate::settings::save_generation(), before_save + 1);
    let baseline = crate::settings::save_generation();
    let image = app
        .fullscreen_page_layout
        .single_page()
        .unwrap()
        .transform
        .paint_rect;
    let pos = image.min + image.size() * egui::vec2(0.3, 0.4);
    viewer.frame(
        &mut app,
        vec![egui::Event::PointerMoved(pos), pointer(pos, true, true)],
    );
    assert_eq!(crate::settings::save_generation(), baseline);

    let retry = Arc::new(AtomicBool::new(true));
    let passes = Arc::new(AtomicUsize::new(0));
    let retry_once = retry.clone();
    let observed_passes = passes.clone();
    viewer.ctx.on_end_pass(
        "depth_focus_save_repass",
        Arc::new(move |ui| {
            observed_passes.fetch_add(1, Ordering::Relaxed);
            if retry_once.swap(false, Ordering::Relaxed) {
                ui.ctx()
                    .request_discard("check one focus save for a physical click");
            }
        }),
    );
    viewer.frame(&mut app, vec![pointer(pos, false, true)]);
    assert!(
        passes.load(Ordering::Relaxed) >= 2,
        "the click frame really discarded and ran again"
    );
    assert_eq!(app.settings.depth_parallax.focus, 0.5);
    assert_eq!(
        crate::settings::save_generation(),
        baseline + 1,
        "one click saves once across all passes"
    );
    for _ in 0..8 {
        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
            unreachable!()
        };
        assert!(state.focus_highlight.is_some());
        assert!(state.pending_focus_click.is_none());
        // Keep the presentation highlighted without depending on test speed.
        state.focus_highlight =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
        viewer.frame(&mut app, vec![]);
        assert_eq!(
            viewer.callbacks().len(),
            1,
            "the highlighted image really painted"
        );
        assert_eq!(
            crate::settings::save_generation(),
            baseline + 1,
            "highlight repaint must not save settings"
        );
    }
    crate::key_input::clear_test_frame();
}

#[derive(Clone, Copy, Debug)]
enum FocusGeometryChange {
    QuarterTurn,
    Resize,
}

fn assert_focus_click_uses_current_transform(change: FocusGeometryChange) {
    let _input = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = RootDepthViewer::new(&mut app);
    let size = [64_u32, 96_u32];
    let values: Vec<_> = (0..size[1])
        .flat_map(|_| (0..size[0]).map(|x| half::f16::from_f32(x as f32 / (size[0] - 1) as f32)))
        .collect();
    let resource_id = viewer.ready_with_depth_values(&mut app, size, values);
    app.settings.depth_parallax.shape_depth = false;
    app.settings.depth_parallax.focus = 0.1;
    app.settings.touch_still_chrome_learned = true;
    // No inference during the test; input and the real renderer still run.
    app.window_visible = false;
    for _ in 0..2 {
        viewer.frame(&mut app, vec![]);
    }
    app.fs_focus_regained_at = None;
    let initial = app.fullscreen_page_layout.single_page().unwrap().transform;
    let pos = initial.paint_rect.min + initial.paint_rect.size() * egui::vec2(0.2, 0.55);
    viewer.frame(
        &mut app,
        vec![egui::Event::PointerMoved(pos), pointer(pos, true, true)],
    );
    let previous = app.fullscreen_page_layout.single_page().unwrap().transform;
    assert_eq!(previous.rotation, crate::rotation_db::Rotation::None);
    assert_eq!(app.settings.depth_parallax.focus, 0.1);
    if matches!(change, FocusGeometryChange::Resize) {
        // begin_pass receives the new screen rectangle with the release event;
        // no intervening frame paints or records this size before the click.
        viewer.size = egui::vec2(820., 450.);
    }
    let mut release_passes = Vec::new();
    viewer.frame_with_before(&mut app, vec![pointer(pos, false, true)], |app, ctx| {
        release_passes.push(ctx.cumulative_pass_nr());
        if matches!(change, FocusGeometryChange::QuarterTurn) {
            // Publish the rotation inside the release pass, before input planning
            // and drawing, just as the accepted cached rotation is read by them.
            app.rotation_cache
                .insert(0, crate::rotation_db::Rotation::Cw90);
        }
    });
    let current = app.fullscreen_page_layout.single_page().unwrap().transform;
    assert!(!release_passes.is_empty());
    assert_eq!(
        viewer.callbacks(),
        vec![current.paint_rect],
        "use the transform of the actual Depth callback"
    );
    assert_ne!(current.paint_rect, previous.paint_rect);
    assert!(previous.paint_rect.contains(pos) && current.paint_rect.contains(pos));
    match change {
        FocusGeometryChange::QuarterTurn => {
            assert_eq!(current.rotation, crate::rotation_db::Rotation::Cw90)
        }
        FocusGeometryChange::Resize => {
            assert_eq!(viewer.ctx.content_rect().size(), egui::vec2(820., 450.));
            assert_ne!(current.viewport_rect.size(), previous.viewport_rect.size());
            assert_ne!(current.paint_rect.size(), previous.paint_rect.size());
        }
    }
    let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
        unreachable!()
    };
    let resource = state.ready().unwrap();
    assert_eq!(
        resource.resource_id, resource_id,
        "geometry changes keep the same source resource"
    );
    assert!(state.pending_focus_click.is_none());
    let sample = |transform: &crate::displayed_image_transform::DisplayedImageTransform| {
        let params = crate::depth_parallax_wgpu::DepthRenderParams::from_settings(
            &app.settings.depth_parallax,
            resource.depth.median,
            egui::Vec2::ZERO,
            transform.uv_rect,
            transform.rotation.degrees() as u32 / 90,
            transform.paint_rect.aspect_ratio(),
        );
        crate::depth_parallax_wgpu::focus_depth_at(
            pos,
            transform.paint_rect,
            params,
            resource.depth.width,
            resource.depth.height,
            &resource.depth.values,
        )
        .unwrap()
    };
    let expected_current = sample(&current);
    let expected_previous = sample(&previous);
    let tolerance = 1e-4;
    let agrees_with_current = |focus: f32| (focus - expected_current).abs() <= tolerance;
    assert!(
        (expected_current - expected_previous).abs() > 0.1,
        "{change:?}: stale geometry must differ far beyond tolerance: current={expected_current}, previous={expected_previous}, tolerance={tolerance}"
    );
    assert!(
        !agrees_with_current(expected_previous),
        "the acceptance check must reject the old transform's result"
    );
    assert!(
        agrees_with_current(app.settings.depth_parallax.focus),
        "{change:?}: actual={}, current={expected_current}, previous={expected_previous}",
        app.settings.depth_parallax.focus
    );
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_focus_click_uses_same_pass_rotated_transform_with_gradient() {
    assert_focus_click_uses_current_transform(FocusGeometryChange::QuarterTurn);
}

#[test]
fn depth_focus_click_uses_same_pass_resized_transform_with_gradient() {
    assert_focus_click_uses_current_transform(FocusGeometryChange::Resize);
}

#[test]
fn depth_focus_paint_checks_pass_resource_bounds_lru_and_media_radius() {
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
