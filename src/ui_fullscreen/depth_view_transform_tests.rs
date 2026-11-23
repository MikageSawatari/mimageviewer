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
fn depth_transform_wheel_prediction_matches_application_for_zoom_and_local_scroll() {
    let _input = crate::key_input::lock_test_input();
    for ctrl in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = RootDepthViewer::new(&mut app);
        viewer.ready(&mut app);
        hide_panel(&mut app);
        app.fs_zoom = 2.;
        viewer.frame(&mut app, vec![]);
        app.fs_focus_regained_at = None;
        let media = app
            .fullscreen_page_layout
            .single_page()
            .unwrap()
            .transform
            .viewport_rect;
        let pos = media.center();
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
        ];
        let mut predicted = None;
        viewer.frame_with_before(&mut app, events, |app, ctx| {
            if predicted.is_some() {
                return;
            }
            let options = ctx.options(|options| options.input_options);
            let segments =
                ctx.input(|input| crate::wheel_input::remaining_wheel_segments(input, options));
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
            predicted = Some(app.predict_fs_wheel_frame(ctx, media, &actions));
        });
        let (zoom, pan, navigation) = predicted.unwrap();
        assert!(!navigation);
        assert!((app.fs_zoom - zoom).abs() < 1e-5);
        assert!((app.fs_pan - pan).abs().max_elem() < 1.);
        assert_eq!(app.fullscreen_idx, Some(0));
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
        let before = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
        viewer.frame(
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
