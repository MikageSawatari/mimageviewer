//! Exact crop meshes across the real flat/curl/canonical renderer handoff.
use super::*;
use crate::export_crop::{CropAspectMode, CropRect, CropSettings};

fn crop() -> CropSettings {
    CropSettings::authored(
        CropRect {
            min_x: 10.0,
            min_y: 20.0,
            max_x: 70.0,
            max_y: 100.0,
        },
        CropAspectMode::Free,
        [80, 120],
    )
}

fn render(ctx: &egui::Context, app: &mut App, navigation: bool) -> egui::FullOutput {
    crate::key_input::set_test_frame_for_viewport(ctx.viewport_id(), vec![]);
    app.frame_counter += 1;
    let mut navigate = navigation;
    let mut admitted = None;
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(960.0, 720.0),
        )),
        focused: true,
        ..Default::default()
    };
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .focused = Some(true);
    let output = crate::test_ui::headless_paint_output(ctx.run_ui(input, |ui| {
        if std::mem::take(&mut navigate) {
            app.begin_page_turn_viewer_frame(ctx, true, None);
            let operation = ViewerOperationId::Slideshow(next_operation_batch());
            app.page_turn_external_operation(
                ctx,
                operation,
                OperationPhase::Press,
                "crop-order-test",
            );
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
                Some(operation),
            );
            assert_eq!(app.fullscreen_idx, Some(1));
            if let PresentationState::Offered { outgoing, .. } = &app.page_turn_presentation.state {
                admitted = Some(outgoing.clone());
            }
        }
        app.render_fullscreen_viewport(ui);
    }));
    if let (Some(outgoing), PresentationState::Flat(incoming)) =
        (admitted, &app.page_turn_presentation.state)
    {
        let reason = prepare_leaf(&outgoing, incoming, true).err();
        panic!(
            "accepted fixture did not curl: {reason:?}; outgoing={:?}; incoming={:?}; events={:?}",
            outgoing.pages[0].transform,
            incoming.pages[0].transform,
            take_test_events()
        );
    }
    output
}

fn crop_meshes(output: &egui::FullOutput) -> Vec<(egui::Rect, egui::Mesh)> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Mesh(mesh) if is_crop(mesh) => Some((shape.clip_rect, (**mesh).clone())),
            _ => None,
        })
        .collect()
}

fn is_crop(mesh: &egui::Mesh) -> bool {
    mesh.vertices
        .iter()
        .any(|v| v.color == egui::Color32::from_rgb(255, 230, 100))
}

fn assert_flat(app: &App, output: &egui::FullOutput) {
    let transform = app.fullscreen_page_layout.single_page().unwrap().transform;
    let projected = crate::page_turn_animation::project_flat_crop(
        transform.paint_rect,
        transform.viewport_rect,
        &transform.paint_quad().positions,
        page_crop_paint(&transform, crop()),
    );
    let actual = crop_meshes(output);
    assert_eq!(
        actual.len(),
        1,
        "exactly one flat crop, no missing transition frame"
    );
    assert_eq!(projected.len(), 1);
    assert_eq!(
        actual[0].1, projected[0].mesh,
        "actual bound flat crop geometry and multiplicity"
    );
    let crop_order = output
        .shapes
        .iter()
        .position(|s| {
            matches!(&s.shape,
        egui::Shape::Mesh(mesh) if is_crop(mesh))
        })
        .unwrap();
    let grid: Vec<_> = output
        .shapes
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            matches!(
        &s.shape, egui::Shape::LineSegment { stroke, .. }
            if stroke.color == egui::Color32::from_black_alpha(120))
            .then_some(i)
        })
        .collect();
    assert!(grid.iter().all(|i| *i < crop_order));
}

fn assert_curl(app: &App, output: &egui::FullOutput, progress: f32) {
    let PresentationState::Turning { projection, .. } = &app.page_turn_presentation.state else {
        panic!(
            "must exercise an actual curl; events={:?}",
            take_test_events()
        );
    };
    let expected = crate::page_turn_animation::project_prepared(
        &projection.outgoing,
        &projection.incoming,
        projection.direction,
        projection.recipe,
        progress,
        false,
    )
    .unwrap()
    .meshes
    .into_iter()
    .filter(|draw| is_crop(&draw.mesh))
    .map(|draw| (draw.clip_rect, draw.mesh))
    .collect::<Vec<_>>();
    assert!(
        !expected.is_empty(),
        "visible crop faces at progress={progress}"
    );
    assert_eq!(
        crop_meshes(output),
        expected,
        "curl must contain only its projected crop: no missing face or extra flat crop"
    );
}

#[test]
fn crop_real_renderer_flat_curl_canonical_and_landing_keep_exactly_one_owner() {
    let _input = crate::key_input::lock_test_input();
    for terminal in ["finish", "off", "zoom"] {
        let origin = Instant::now();
        let clock = super::clock_tests::TestClock::at(origin);
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        install(&ctx);
        install_fs_navigator_input_tracking(&ctx);
        let mut app =
            crate::ui_fullscreen::page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
        app.viewer_presentation = crate::app::ViewerPresentation::MainWindow;
        app.native_video_in_window_active = true;
        app.settings.video_in_window_mode = true;
        app.settings.fullscreen_top_bar_locked = false;
        app.settings
            .set_still_bottom_lock(crate::settings::BottomBarLock::None);
        app.fs_info_panel.locked = false;
        app.fs_prev_focused = true;
        app.settings.page_turn_animation_enabled = true;
        app.settings.page_turn_animation_duration_ms = 200;
        app.settings.fullscreen_navigator_visible = false;
        app.settings.still_seek_strip_visible = false;
        app.settings.touch_still_chrome_learned = true;
        app.fs_pixel_grid_enabled = true;
        for idx in 0..4 {
            app.export_crop_page_settings.insert(idx, crop());
        }
        for idx in [1, 0, 0] {
            app.fullscreen_idx = Some(idx);
            let output = render(&ctx, &mut app, false);
            assert_flat(&app, &output);
        }
        assert!(app.page_turn_current_unit_presented());
        let first = render(&ctx, &mut app, true);
        assert_curl(&app, &first, 0.0);
        if terminal == "finish" {
            for elapsed in [1, 99, 101, 199] {
                clock.set(origin + Duration::from_millis(elapsed));
                let output = render(&ctx, &mut app, false);
                assert_curl(
                    &app,
                    &output,
                    Duration::from_millis(elapsed).as_secs_f32()
                        / Duration::from_millis(200).as_secs_f32(),
                );
            }
            clock.set(origin + Duration::from_millis(200));
        } else if terminal == "off" {
            app.settings.page_turn_animation_enabled = false;
        } else {
            app.fs_zoom = 3.0;
        }
        let landing = render(&ctx, &mut app, false);
        assert_flat(&app, &landing);
        let ordinary = render(&ctx, &mut app, false);
        assert_flat(&app, &ordinary);
        assert!(!matches!(
            app.page_turn_presentation.state,
            PresentationState::Turning { .. }
        ));
    }
}
