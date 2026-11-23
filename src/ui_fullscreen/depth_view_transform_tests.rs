//! Headless regression coverage for the shared 2D/Depth transform owner.
use super::*;
use crate::app::depth_main_window_tests::RootDepthViewer;
use crate::fs_exclusive_view::FsExclusiveView;

std::thread_local! {
    pub(super) static PAGED_PREPARES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn hide_panel(app: &mut App) {
    if let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view {
        state.panel_visible = false;
    }
}

#[test]
fn depth_transform_inputs_keep_the_ready_source_and_share_the_exit_view() {
    let _input = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = RootDepthViewer::new(&mut app);
    let resource_id = viewer.ready(&mut app);
    let source_key = original_texture(&app);
    hide_panel(&mut app);
    viewer.frame(&mut app, vec![]);
    let original = app.fullscreen_page_layout.single_page().unwrap().transform;
    app.fs_zoom_active = true;
    assert_eq!(app.fs_zoom_owner(), FsZoomOwner::Image);
    assert!(app.apply_fs_key_zoom_step(&viewer.ctx, true));
    assert!(app.apply_touch_zoom_factor_about_pivot(
        1.3,
        original.paint_rect.center(),
        original.paint_rect.center()
    ));
    app.apply_touch_pan_delta(egui::vec2(14., -17.), original.viewport_rect);
    assert!(app.apply_gamepad_fullscreen_zoom(0.5, 0.2));
    assert!(app.apply_gamepad_fullscreen_pan(egui::vec2(-8., 12.)));
    app.fs_free_rotation = 0.3;
    viewer.frame(&mut app, vec![]);
    let transformed = app.fullscreen_page_layout.single_page().unwrap().transform;
    assert_ne!(transformed.paint_rect, original.paint_rect);
    assert_eq!(transformed.free_rotation_rad, 0.3);
    assert!(!viewer.callbacks().is_empty());
    let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
        unreachable!()
    };
    assert_eq!(state.ready().unwrap().resource_id, resource_id);
    assert_eq!(state.ready().unwrap().key.texture_id, source_key);
    let saved = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
    app.set_fs_exclusive_view(FsExclusiveView::None, "shared_transform_exit_test");
    assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), saved);
    assert_eq!(app.fs_zoom_owner(), FsZoomOwner::ZoomMode);
    crate::key_input::clear_test_frame();
}

fn original_texture(app: &App) -> egui::TextureId {
    let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
        unreachable!()
    };
    state.ready().unwrap().key.texture_id
}

#[test]
fn depth_transform_click_classification_focus_and_double_reset_use_the_shared_view() {
    let _input = crate::key_input::lock_test_input();
    for ctrl in [false, true] {
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        app.items = vec![GridItem::Image("transformed-focus.jpg".into())];
        app.fullscreen_idx = Some(0);
        app.fs_exclusive_view = FsExclusiveView::Depth(crate::depth_parallax::DepthViewState::new(
            app.edit_request_owner_context(),
        ));
        app.fs_zoom = 2.;
        app.fs_pan = egui::vec2(12., -8.);
        app.fs_free_rotation = 0.25;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800., 600.));
        let pos = rect.center();
        let mut clicks = 0;
        for frame in 0..5 {
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if frame > 0 {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: frame % 2 == 1,
                    modifiers: if ctrl {
                        egui::Modifiers::CTRL
                    } else {
                        egui::Modifiers::NONE
                    },
                });
            }
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    events,
                    time: Some(frame as f64 * 0.05),
                    ..Default::default()
                },
                |ui| {
                    app.capture_fs_front_pointer_input(&ctx, rect);
                    let response = ui.interact(
                        rect,
                        egui::Id::unique("transformed-click"),
                        egui::Sense::click_and_drag(),
                    );
                    app.fs_front_pointer.accept_backing_click(&response, false);
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
                        app.fs_zoom,
                        app.fs_pan,
                    );
                    if response.clicked() {
                        clicks += 1;
                        if clicks == 2 {
                            assert!(matches!(action, FsCanvasClickAction::ResetTransform));
                        } else if ctrl {
                            assert!(
                                matches!(action, FsCanvasClickAction::DepthFocusAt(p) if p == pos)
                            );
                        } else {
                            assert!(matches!(action, FsCanvasClickAction::None));
                        }
                    }
                    assert_eq!(app.fs_free_rotation, 0.25, "click planning must not rotate");
                    assert!(app.fs_rotation_drag_start.is_none());
                },
            )
            .drop_without_applying_deltas();
        }
        assert_eq!(clicks, 2);
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_transform_ready_first_draw_commits_reading_start_and_bounds_like_2d() {
    let _input = crate::key_input::lock_test_input();
    for reading_start in [false, true] {
        let mut pans = vec![];
        for depth in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            let mut viewer = RootDepthViewer::new(&mut app);
            let resource_id = viewer.ready(&mut app);
            hide_panel(&mut app);
            if !depth {
                app.set_fs_exclusive_view(FsExclusiveView::None, "placement_2d_control");
            }
            app.settings.fullscreen_fit_mode = FullscreenFitMode::Width;
            app.settings.fullscreen_page_start_position =
                crate::settings::FullscreenPageStartPosition::ReadingStart;
            if reading_start {
                app.request_fs_initial_placement(0);
            } else {
                app.fs_zoom = 2.;
                app.fs_pan = egui::vec2(9999., -9999.);
            }
            PAGED_PREPARES.with(|count| count.set(0));
            viewer.frame(&mut app, vec![]);
            let transform = app.fullscreen_page_layout.single_page().unwrap().transform;
            let (_, drawn_pan) = normal_zoom_pan(transform).expect("ordinary placement");
            assert_eq!(app.fs_pan, drawn_pan);
            assert!(app.fs_initial_placement_request.is_none());
            if depth {
                assert_eq!(
                    PAGED_PREPARES.with(|count| count.get()),
                    0,
                    "Depth skips 2D payload preparation"
                );
                let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                    unreachable!()
                };
                assert_eq!(state.ready().unwrap().resource_id, resource_id);
                assert!(!viewer.callbacks().is_empty());
            }
            pans.push(app.fs_pan);
        }
        assert_eq!(pans[0], pans[1]);
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_transform_pdf_suppression_keeps_comparisons_until_the_first_2d_draw() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    app.items = vec![GridItem::PdfPage {
        pdf_path: "depth-demand.pdf".into(),
        page_num: 0,
        content_type: None,
    }];
    app.fullscreen_idx = Some(0);
    assert!(app.fs_pdf_rerender_demand.note_legacy(0, 1.5));
    app.fs_exclusive_view = FsExclusiveView::Depth(crate::depth_parallax::DepthViewState::new(
        app.edit_request_owner_context(),
    ));
    app.maybe_rerender_pdf(3.);
    app.settings.fullscreen_fit_mode = FullscreenFitMode::SpecifiedScale;
    app.reconcile_specified_pdf_demand(&ctx);
    assert_eq!(app.fs_pdf_rerender_demand.legacy_idx(), Some(0));
    app.set_fs_exclusive_view(FsExclusiveView::None, "pdf_demand_exit");
    assert_eq!(
        app.fs_pdf_rerender_demand.legacy_idx(),
        Some(0),
        "setter has no demand side effects"
    );
    assert!(app.fs_pdf_rerender_demand.resume_after_depth());
    assert!(
        app.fs_pdf_rerender_demand.note_legacy(0, 1.5),
        "same pre-entry demand must be issued again"
    );
    assert!(!app.fs_pdf_rerender_demand.resume_after_depth());
}

#[test]
fn depth_transform_pdf_first_single_draw_consumes_suppression_after_d_f12_and_z() {
    let _input = crate::key_input::lock_test_input();
    let render = {
        let mut renderer_app = crate::app::setup_app_for_test();
        let _renderer_viewer = RootDepthViewer::new(&mut renderer_app);
        renderer_app.wgpu_render_state.clone()
    };
    for specified in [false, true] {
        for z in [false, true] {
            for f12 in [false, true] {
                let ctx = egui::Context::default();
                let mut app = super::specified_scale_consumer_tests::pdf_app(&ctx, 1);
                app.wgpu_render_state = render.clone();
                app.settings.detached_viewer_enabled = false;
                app.settings.detached_viewer_open_images_in_window = false;
                app.settings.page_turn_animation_enabled = false;
                app.fs_zoom_active = z;
                app.fs_zoom_factor = 2.;
                if !specified {
                    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
                }
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200., 800.));
                let probe = crate::app::PdfRerenderRequestProbe::start();
                super::specified_scale_consumer_tests::single(&mut app, &ctx, rect, z);
                if !specified && !z {
                    app.fs_pdf_rerender_demand.note_legacy(0, 1.);
                }
                probe.take_requests();
                app.fs_pending.clear();
                let owner = app.edit_request_owner_context();
                app.set_fs_exclusive_view(
                    FsExclusiveView::Depth(crate::depth_parallax::DepthViewState::new(owner)),
                    "pdf_suppression_enter",
                );
                app.fs_zoom = 2.;
                app.maybe_rerender_pdf(2.);
                super::specified_scale_consumer_tests::single(&mut app, &ctx, rect, false);
                assert!(app.fs_exclusive_view.is_depth());
                assert!(
                    probe.take_requests().is_empty(),
                    "Depth cannot issue display zoom demands"
                );
                assert!(matches!(
                    app.fs_pdf_rerender_demand,
                    crate::app::PdfRerenderDemand::DepthSuppressed(_)
                ));
                // Return to the same pre-entry demand: stale records must not suppress it.
                app.fs_zoom = 1.;
                if f12 {
                    app.toggle_detached_viewer_mode();
                } else {
                    app.toggle_depth_parallax(&ctx, 0);
                }
                assert!(!app.fs_exclusive_view.is_depth());
                assert!(
                    probe.take_requests().is_empty(),
                    "exit does not issue a request"
                );
                let resumed_z = app.fs_zoom_mode_effective();
                if !f12 {
                    assert_eq!(resumed_z, z);
                }
                super::specified_scale_consumer_tests::single(&mut app, &ctx, rect, resumed_z);
                assert!(
                    !probe.take_requests().is_empty(),
                    "specified={specified}, z={z}, f12={f12}"
                );
                assert!(
                    !app.fs_pdf_rerender_demand.resume_after_depth(),
                    "first non-Depth draw consumed the mark"
                );
            }
        }
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_transform_wheel_prediction_matches_2d_for_default_navigation_and_local_scroll() {
    let _input = crate::key_input::lock_test_input();
    for mode in [
        crate::settings::FullscreenWheelAction::default(),
        crate::settings::FullscreenWheelAction::ScrollThenPageTurn,
    ] {
        for ctrl in [false, true] {
            let initial_zooms: &[f32] = if ctrl { &[1., 2.] } else { &[2.] };
            for &initial_zoom in initial_zooms {
                let mut outcomes = Vec::new();
                for depth in [false, true] {
                    let mut app = crate::app::setup_app_for_test();
                    let mut viewer = RootDepthViewer::new(&mut app);
                    viewer.ready(&mut app);
                    hide_panel(&mut app);
                    app.items[1] = GridItem::Image("wheel-shared-next.jpg".into());
                    crate::app::source_texture_test_support::seed(&mut app, &viewer.ctx, 1);
                    if !depth {
                        app.set_fs_exclusive_view(FsExclusiveView::None, "wheel_2d_control");
                    }
                    app.settings.fullscreen_wheel_action = mode;
                    app.settings.fullscreen_smooth_page_scroll = false;
                    app.window_visible = false;
                    app.fs_zoom = initial_zoom;
                    viewer.frame(&mut app, vec![]);
                    app.fs_focus_regained_at = None;
                    let media = app
                        .fullscreen_page_layout
                        .single_page()
                        .unwrap()
                        .transform
                        .viewport_rect;
                    let pos = media.center() + egui::vec2(25., 0.);
                    // Establish a physical canvas press before the wheel+release
                    // pass. The click stays unmodified even when the wheel is Ctrl.
                    viewer.frame(&mut app, vec![egui::Event::PointerMoved(pos)]);
                    viewer.frame(
                        &mut app,
                        vec![egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: egui::Modifiers::NONE,
                        }],
                    );
                    let modifiers = if ctrl {
                        egui::Modifiers::CTRL
                    } else {
                        egui::Modifiers::NONE
                    };
                    let events = vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: egui::vec2(0., -20.),
                            modifiers,
                            phase: egui::TouchPhase::Move,
                            source: egui::MouseWheelSource::Wheel,
                        },
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ];
                    let mut predicted = None;
                    viewer.frame_with_before(&mut app, events, |app, ctx| {
                        if predicted.is_some() {
                            return;
                        }
                        let options = ctx.options(|options| options.input_options);
                        let segments = ctx.input(|input| {
                            crate::wheel_input::remaining_wheel_segments(input, options)
                        });
                        let actions = segments
                            .into_iter()
                            .map(|segment| FsWheelAction {
                                input: segment.input,
                                segment,
                                operation: None,
                                class: app.fs_wheel_segment_in_page_scroll(segment.input, false),
                                scroll_origin: app.fullscreen_page_layout.drawn_unit.clone(),
                                page_owner: (
                                    app.items_generation,
                                    app.fullscreen_idx,
                                    app.fullscreen_page_slice,
                                ),
                                seek_hover: false,
                            })
                            .collect::<Vec<_>>();
                        assert_eq!(actions.len(), 1);
                        predicted = Some(app.predict_fs_wheel_frame(ctx, media, &actions));
                    });
                    let (zoom, pan, navigation) = predicted.unwrap();
                    let response = viewer
                        .ctx
                        .read_response(egui::Id::unique("fs_click"))
                        .unwrap();
                    assert!(
                        response.clicked(),
                        "the wheel pass must also contain the canvas release"
                    );
                    assert!(
                        matches!(
                            app.plan_fs_canvas_click(
                                &viewer.ctx,
                                &response,
                                egui::Rect::from_min_size(egui::Pos2::ZERO, viewer.size),
                                StillSeekGeometry::bar_only(),
                                false,
                                false,
                                false,
                                false,
                                None,
                                false,
                                zoom,
                                pan,
                            ),
                            FsCanvasClickAction::None
                        ),
                        "the wheel's transformed view owns the unmodified click"
                    );
                    let expected_navigation =
                        !ctrl && mode == crate::settings::FullscreenWheelAction::PageTurn;
                    assert_eq!(
                        navigation, expected_navigation,
                        "mode={mode:?}, ctrl={ctrl}, depth={depth}"
                    );
                    assert_eq!(app.fullscreen_idx, Some(usize::from(expected_navigation)));
                    if !navigation {
                        assert!((app.fs_zoom - zoom).abs() < 1e-5);
                        assert!((app.fs_pan - pan).abs().max_elem() < 1.);
                        if ctrl {
                            assert!(zoom < initial_zoom);
                        } else {
                            assert_ne!(pan, egui::Vec2::ZERO);
                        }
                    }
                    outcomes.push((app.fullscreen_idx, app.fs_zoom, app.fs_pan, predicted));
                }
                assert_eq!(
                    outcomes[0], outcomes[1],
                    "Depth and 2D share mode={mode:?}, ctrl={ctrl}, initial_zoom={initial_zoom}"
                );
            }
        }
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_transform_panel_rejects_wheel_and_drag_starts() {
    let _input = crate::key_input::lock_test_input();
    for ctrl in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        viewer.ready(&mut app);
        for _ in 0..3 {
            viewer.frame(&mut app, vec![]);
        }
        app.fs_focus_regained_at = None;
        let panel = app.visible_depth_panel_rect(&viewer.ctx).unwrap();
        let pos = panel.min + egui::vec2(15., 60.);
        assert!(panel.contains(pos));
        let before = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
        viewer.frame_with_before(
            &mut app,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0., -120.),
                    modifiers: egui::Modifiers::CTRL,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Wheel,
                },
            ],
            |app, ctx| {
                assert_eq!(app.visible_depth_panel_rect(ctx), Some(panel));
                assert!(
                    ctx.input(|input| input.pointer.hover_pos())
                        .is_some_and(|point| panel.contains(point))
                );
            },
        );
        assert_eq!(
            (app.fs_zoom, app.fs_pan, app.fs_free_rotation),
            before,
            "Ctrl+wheel on the recorded Depth panel must not reach image zoom"
        );
        let modifiers = if ctrl {
            egui::Modifiers::CTRL
        } else {
            egui::Modifiers::NONE
        };
        for (point, edge) in [
            (pos, Some(true)),
            (pos + egui::vec2(-100., 20.), None),
            (pos + egui::vec2(-120., 30.), Some(false)),
        ] {
            let mut events = vec![
                egui::Event::PointerMoved(point),
                egui::Event::ModifiersChanged(modifiers),
            ];
            if let Some(pressed) = edge {
                events.push(egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers,
                });
            }
            viewer.frame(&mut app, events);
        }
        assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), before);
        assert!(app.fs_pan_drag_start.is_none());
        assert!(app.fs_rotation_drag_start.is_none());
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_transform_middle_and_scrub_start_on_canvas_but_not_on_panel() {
    use crate::key_input::{SyntheticKeyCommand, SyntheticModifiers, SyntheticNavigationKey};
    let _input = crate::key_input::lock_test_input();
    for scrub in [false, true] {
        for panel in [false, true] {
            for completes_in_pass in [false, true] {
                let mut app = crate::app::setup_app_for_test();
                let mut viewer = RootDepthViewer::new(&mut app);
                let resource_id = viewer.ready(&mut app);
                app.window_visible = false;
                for _ in 0..3 {
                    viewer.frame(&mut app, vec![]);
                }
                app.fs_focus_regained_at = None;
                let panel_rect = app.visible_depth_panel_rect(&viewer.ctx).unwrap();
                let on_panel = panel_rect.min + egui::vec2(15., 60.);
                let on_canvas = egui::pos2(panel_rect.left() - 40., on_panel.y);
                let start = if panel { on_panel } else { on_canvas };
                let crossing = if panel { on_canvas } else { on_panel };
                let returned = on_canvas + egui::vec2(-80., -50.);
                if scrub {
                    app.keymap = crate::keymap::Keymap::from_ini_str(
                        "[FsImage]\nFsScrubZoomHold = Ctrl+Tab\n",
                    );
                    crate::key_input::arm_test_synthetic_input(0xE088, egui::ViewportId::ROOT);
                    let now = std::time::Instant::now();
                    crate::key_input::enqueue_test_synthetic_command(SyntheticKeyCommand::down(
                        now,
                        SyntheticNavigationKey::Tab,
                        SyntheticModifiers {
                            ctrl: true,
                            ..Default::default()
                        },
                    ));
                    crate::key_input::advance_test_synthetic_input(now);
                }
                let before = app.fs_zoom;
                let before_transform = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
                let button = if scrub {
                    egui::PointerButton::Primary
                } else {
                    egui::PointerButton::Middle
                };
                let modifiers = if scrub {
                    egui::Modifiers::CTRL
                } else {
                    egui::Modifiers::NONE
                };
                // The press and boundary crossing share a pass, so hover already
                // differs from press_origin when middle/scrub admission runs.
                let mut start_events = vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::PointerMoved(start),
                    egui::Event::PointerButton {
                        pos: start,
                        button,
                        pressed: true,
                        modifiers,
                    },
                    egui::Event::PointerMoved(crossing),
                ];
                if completes_in_pass {
                    start_events.push(egui::Event::PointerButton {
                        pos: crossing,
                        button,
                        pressed: false,
                        modifiers,
                    });
                }
                viewer.frame_with_before(&mut app, start_events, |_, ctx| {
                    ctx.input(|input| {
                        assert_eq!(
                            input.pointer.press_origin(),
                            (!completes_in_pass).then_some(start),
                            "scrub={scrub}, panel={panel}, completes_in_pass={completes_in_pass}"
                        );
                        assert_eq!(
                            input.pointer.hover_pos(),
                            Some(crossing),
                            "scrub={scrub}, panel={panel}"
                        );
                        assert_eq!(
                            input.pointer.button_down(button),
                            !completes_in_pass,
                            "scrub={scrub}, panel={panel}, completes_in_pass={completes_in_pass}"
                        );
                    });
                });
                assert_eq!(
                    app.fs_middle_zoom_drag.is_some(),
                    !panel && !completes_in_pass,
                    "scrub={scrub}, panel={panel}, completes_in_pass={completes_in_pass}, press={start:?}, hover={crossing:?}"
                );
                if completes_in_pass {
                    assert_eq!(
                        (app.fs_zoom, app.fs_pan, app.fs_free_rotation),
                        before_transform
                    );
                } else {
                    for (pos, edge) in [(returned, None), (returned, Some(false))] {
                        let mut events = vec![
                            egui::Event::ModifiersChanged(modifiers),
                            egui::Event::PointerMoved(pos),
                        ];
                        if let Some(pressed) = edge {
                            events.push(egui::Event::PointerButton {
                                pos,
                                button,
                                pressed,
                                modifiers,
                            });
                        }
                        viewer.frame(&mut app, events);
                    }
                    assert_eq!(
                        app.fs_zoom != before,
                        !panel,
                        "scrub={scrub}, panel={panel}"
                    );
                }
                assert!(app.fs_middle_zoom_drag.is_none());
                let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                    unreachable!()
                };
                assert_eq!(state.ready().unwrap().resource_id, resource_id);
                if scrub {
                    crate::key_input::clear_test_synthetic_input();
                }
            }
        }
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_transform_middle_short_click_same_pass_uses_pressed_position() {
    use crate::ring_shortcut::{RingActionId, RingShortcutContext};
    let _input = crate::key_input::lock_test_input();
    for starts_on_panel in [false, true] {
        for ends_over_panel in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            let mut viewer = RootDepthViewer::new(&mut app);
            let resource_id = viewer.ready(&mut app);
            app.window_visible = false;
            for _ in 0..3 {
                viewer.frame(&mut app, vec![]);
            }
            app.fs_focus_regained_at = None;
            app.settings
                .ring_shortcuts
                .mouse_button_profile_mut(RingShortcutContext::ImageFullscreen)
                .middle = RingActionId::ImageSlideshow;
            let panel = app.visible_depth_panel_rect(&viewer.ctx).unwrap();
            let on_panel = panel.min + egui::vec2(15., 60.);
            let on_canvas = egui::pos2(panel.left() - 40., on_panel.y);
            let press = if starts_on_panel { on_panel } else { on_canvas };
            let hover = if ends_over_panel { on_panel } else { on_canvas };
            let before = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
            viewer.frame_with_before(
                &mut app,
                vec![
                    egui::Event::PointerMoved(press),
                    egui::Event::PointerButton {
                        pos: press,
                        button: egui::PointerButton::Middle,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerButton {
                        pos: press,
                        button: egui::PointerButton::Middle,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                    // Later hover must neither admit a panel press nor reject a
                    // canvas press after the release has cleared press_origin.
                    egui::Event::PointerMoved(hover),
                ],
                |_, ctx| {
                    assert!(ctx.input(|input| input.pointer.press_origin()).is_none());
                },
            );
            assert_eq!(
                app.slideshow_playing, !starts_on_panel,
                "starts_on_panel={starts_on_panel}, ends_over_panel={ends_over_panel}"
            );
            assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), before);
            assert!(app.fs_middle_zoom_drag.is_none());
            assert_eq!(app.fullscreen_idx, Some(0));
            let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                unreachable!()
            };
            assert_eq!(state.ready().unwrap().resource_id, resource_id);
        }
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_transform_pinch_starts_on_canvas_but_not_on_panel() {
    let _input = crate::key_input::lock_test_input();
    for panel in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        let resource_id = viewer.ready(&mut app);
        app.settings.touch_still_chrome_learned = true;
        app.window_visible = false;
        for _ in 0..3 {
            viewer.frame(&mut app, vec![]);
        }
        app.fs_focus_regained_at = None;
        let panel_rect = app.visible_depth_panel_rect(&viewer.ctx).unwrap();
        let on_panel = panel_rect.min + egui::vec2(30., 60.);
        let on_canvas = egui::pos2(panel_rect.left() - 60., on_panel.y);
        let origin = if panel { on_panel } else { on_canvas };
        let crossing = if panel { on_canvas } else { on_panel };
        let returned = on_canvas + egui::vec2(-60., 70.);
        let touch = |id, phase, pos| egui::Event::Touch {
            device_id: egui::TouchDeviceId(874),
            id: egui::TouchId(id),
            phase,
            pos,
            force: None,
        };
        let pointer = |pressed, pos| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        viewer.frame(
            &mut app,
            vec![
                touch(1, egui::TouchPhase::Start, origin),
                egui::Event::PointerMoved(origin),
                pointer(true, origin),
            ],
        );
        viewer.frame(
            &mut app,
            vec![
                touch(1, egui::TouchPhase::Move, crossing),
                egui::Event::PointerMoved(crossing),
            ],
        );
        let second = crossing + egui::vec2(60., 0.);
        viewer.frame(&mut app, vec![touch(2, egui::TouchPhase::Start, second)]);
        let end_second = returned + egui::vec2(90., 6.);
        viewer.frame(
            &mut app,
            vec![
                touch(1, egui::TouchPhase::Move, returned),
                egui::Event::PointerMoved(returned),
                touch(2, egui::TouchPhase::Move, end_second),
            ],
        );
        viewer.frame(&mut app, vec![touch(2, egui::TouchPhase::End, end_second)]);
        viewer.frame(
            &mut app,
            vec![
                touch(1, egui::TouchPhase::End, returned),
                pointer(false, returned),
                egui::Event::PointerGone,
            ],
        );
        assert_eq!(app.fs_zoom != 1., !panel, "panel={panel}");
        assert_eq!(app.fullscreen_idx, Some(0));
        let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
            unreachable!()
        };
        assert_eq!(state.ready().unwrap().resource_id, resource_id);
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_transform_ctrl_click_does_not_apply_a_rotation_drag() {
    let _input = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = RootDepthViewer::new(&mut app);
    viewer.ready(&mut app);
    hide_panel(&mut app);
    app.fs_zoom = 2.;
    app.fs_free_rotation = 0.3;
    viewer.frame(&mut app, vec![]);
    app.fs_focus_regained_at = None;
    let pos = app
        .fullscreen_page_layout
        .single_page()
        .unwrap()
        .transform
        .viewport_rect
        .center();
    for pressed in [true, false] {
        viewer.frame(
            &mut app,
            vec![
                egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::CTRL,
                },
            ],
        );
        assert_eq!(app.fs_free_rotation, 0.3);
    }
    assert_eq!(app.settings.depth_parallax.focus, 0.5);
    let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
        unreachable!()
    };
    assert!(state.focus_highlight.is_some());
    assert!(app.fs_rotation_drag_start.is_none());
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_transform_drag_start_follows_press_origin_across_panel() {
    let _input = crate::key_input::lock_test_input();
    for ctrl in [false, true] {
        for starts_on_panel in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            let mut viewer = RootDepthViewer::new(&mut app);
            let resource_id = viewer.ready(&mut app);
            app.window_visible = false;
            app.fs_zoom = 2.;
            for _ in 0..3 {
                viewer.frame(&mut app, vec![]);
            }
            app.fs_focus_regained_at = None;
            let panel = app.visible_depth_panel_rect(&viewer.ctx).unwrap();
            let on_panel = panel.min + egui::vec2(15., 60.);
            let on_canvas = egui::pos2(panel.left() - 40., on_panel.y);
            let returned = on_canvas + egui::vec2(-60., 80.);
            assert!(panel.contains(on_panel));
            assert!(!panel.contains(on_canvas) && !panel.contains(returned));
            let origin = if starts_on_panel { on_panel } else { on_canvas };
            let crossing = if starts_on_panel { on_canvas } else { on_panel };
            let modifiers = if ctrl {
                egui::Modifiers::CTRL
            } else {
                egui::Modifiers::NONE
            };
            let before = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
            viewer.frame(&mut app, vec![egui::Event::PointerMoved(origin)]);
            viewer.frame(
                &mut app,
                vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::PointerButton {
                        pos: origin,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers,
                    },
                ],
            );
            viewer.frame(&mut app, vec![egui::Event::PointerMoved(crossing)]);
            assert_eq!(
                if ctrl {
                    app.fs_rotation_drag_start.is_some()
                } else {
                    app.fs_pan_drag_start.is_some()
                },
                !starts_on_panel,
                "the press owns admission: ctrl={ctrl}, starts_on_panel={starts_on_panel}"
            );
            viewer.frame(&mut app, vec![egui::Event::PointerMoved(returned)]);
            let after = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
            if starts_on_panel {
                assert_eq!(after, before);
            } else if ctrl {
                assert!((app.fs_free_rotation - before.2).abs() > 1e-4);
            } else {
                assert_ne!(app.fs_pan, before.1);
            }
            viewer.frame(
                &mut app,
                vec![egui::Event::PointerButton {
                    pos: returned,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers,
                }],
            );
            assert!(app.fs_pan_drag_start.is_none() && app.fs_rotation_drag_start.is_none());
            assert_eq!(app.fullscreen_idx, Some(0));
            let FsExclusiveView::Depth(state) = &app.fs_exclusive_view else {
                unreachable!()
            };
            assert_eq!(state.ready().unwrap().resource_id, resource_id);
        }
    }
    crate::key_input::clear_test_frame();
}
