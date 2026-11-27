//! The accepted-material producer and delayed merge use one frozen continuous geometry.
use super::*;

fn r9_record(app: &mut App, viewport: egui::Rect) {
    let size = egui::vec2(1600., 900.);
    let material = crate::plane_view_wgpu::PlaneMaterialDescription {
        source_size: [1600, 900],
        texture_size: size,
        coverage: egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
        sampler_roi: egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
        display_basis: crate::material_display_basis::MaterialDisplayBasis::from_sampler(
            [1600, 900],
            crate::source_coverage::SourceCoverage::FullSource,
        ),
        material_kind: FullscreenPageMaterialKind::Complete,
    };
    let input = DisplayedImageTransformInput {
        page_idx: 0,
        viewport_rect: viewport,
        source_size: size,
        texture_size: size,
        rotation: crate::rotation_db::Rotation::None,
        free_rotation_rad: 0.,
        content_bbox: None,
        fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
            FullscreenFitMode::Page,
        ),
        fit_scale_limits: FullscreenFitScaleLimits::default(),
        pixels_per_point: 1.,
        placement: ResolvedDisplayPlacement::Normal {
            zoom_pan: Some((4., egui::vec2(2202., 0.))),
        },
        pixel_fit: RectPixelFit::Texels,
    };
    let drawn = resolve_fs_image_transform(input, Some(size)).unwrap();
    app.fullscreen_page_layout
        .begin(FullscreenPageLayoutKind::Single);
    app.fullscreen_page_layout.push(drawn);
    app.record_plane_cover_material(0, input, Some(size), material);
    app.fs_zoom = 4.;
    app.fs_pan = egui::vec2(2202., 0.);
    app.set_fs_exclusive_view(
        FsExclusiveView::PlaneView(PlaneViewState::new(PlaneViewPose {
            anchor: app.plane_view_anchor(0),
            strength: 0.25,
            lens_center: egui::vec2(0., 0.5),
        })),
        "r9_accepted_material_fixture",
    );
}

#[test]
fn plane_revision_r9_prediction_immediate_and_delayed_use_frozen_geometry_without_layout_clamp() {
    let _keys = crate::key_input::lock_test_input();
    for delayed in [false, true] {
        for notches in [1., -1.] {
            let mut app = crate::app::setup_app_for_test();
            let viewer = viewer(&mut app);
            r9_record(
                &mut app,
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900., 1600.)),
            );
            let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600., 900.));
            let full_rect = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600., 900. + StillSeekGeometry::bar_only().reserved_height),
            );
            let mut events = vec![
                egui::Event::PointerMoved(canvas.center()),
                wheel(
                    0.,
                    notches * 40.,
                    egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                ),
            ];
            if delayed {
                events.push(wheel(0., -40., egui::Modifiers::NONE));
            }
            viewer
                .ctx
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(full_rect),
                        time: Some((app.frame_counter + 1) as f64 / 60.),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        let cover = app.plane_cover_input_for_canvas(ui.ctx(), canvas).unwrap();
                        assert_eq!(cover.transform.viewport_rect, canvas);
                        let (zoom, pan, pose) = (app.fs_zoom, app.fs_pan, app.plane_view_pose());
                        let expected = plane_view_wheel_transition(
                            app.plane_view_wheel_input(
                                canvas,
                                Some(canvas.center()),
                                notches,
                                zoom,
                                pan,
                                pose,
                                Some(cover),
                            )
                            .unwrap(),
                        );
                        if notches > 0. {
                            assert!((expected.zoom - 4.4).abs() < 1.0e-5);
                            assert!((expected.pan.x - 2502.2).abs() < 0.01);
                        }
                        let options = ui.ctx().options(|options| options.input_options);
                        let actions = ui
                            .ctx()
                            .input(|input| {
                                crate::wheel_input::remaining_wheel_segments(input, options)
                            })
                            .into_iter()
                            .map(|segment| FsWheelAction {
                                input: segment.input,
                                segment,
                                operation: None,
                                class: if segment.input.modifiers.ctrl {
                                    FsWheelInPageClass::PlaneView
                                } else {
                                    FsWheelInPageClass::Legacy
                                },
                                scroll_origin: None,
                                page_owner: (
                                    app.items_generation,
                                    app.fullscreen_idx,
                                    app.fullscreen_page_slice,
                                ),
                                seek_hover: false,
                            })
                            .collect::<Vec<_>>();
                        let predicted = app.predict_fs_wheel_frame_with_cover(
                            ui.ctx(),
                            canvas,
                            &actions,
                            Some(cover),
                        );
                        assert_eq!(
                            (predicted.0, predicted.1, predicted.3),
                            (expected.zoom, expected.pan, expected.pose.is_some())
                        );
                        let (mut result, mut operation) =
                            crate::ui_fullscreen::wheel_frame_tests::collect_actual_merge(
                                &mut app, ui, false,
                            );
                        assert_eq!(result.deferred.is_some(), delayed);
                        if let Some(mut proposal) = result.deferred.take() {
                            assert_eq!(
                                proposal.plane_cover.unwrap().transform.viewport_rect,
                                canvas
                            );
                            // Paint can replace both the layout and its accepted material before merge.
                            r9_record(
                                &mut app,
                                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300., 200.)),
                            );
                            app.fullscreen_page_layout
                                .plane_cover
                                .as_mut()
                                .unwrap()
                                .material
                                .coverage
                                .max
                                .x = 0.1;
                            app.claim_fs_wheel_frame_after_ui(&mut proposal);
                            result =
                                app.apply_fs_wheel_frame_proposal(proposal, false, &mut operation);
                            assert!(!result.close);
                        }
                        assert_eq!((app.fs_zoom, app.fs_pan), (expected.zoom, expected.pan));
                        assert_eq!(app.plane_view_pose(), expected.pose);
                    },
                )
                .drop_without_applying_deltas();
        }
    }
}

#[test]
fn plane_revision_unpainted_page_discards_wheel_and_assigned_key_until_material_record_exists() {
    let _keys = crate::key_input::lock_test_input();
    for keyboard in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsFisheyeIn = F13\n");
        app.fullscreen_page_layout.plane_cover = None;
        let zoom_pan = (app.fs_zoom, app.fs_pan);
        let events = if keyboard {
            key(egui::Key::F13)
        } else {
            vec![
                egui::Event::PointerMoved(viewer.size.to_pos2() * 0.5),
                wheel(0., 40., egui::Modifiers::CTRL | egui::Modifiers::SHIFT),
            ]
        };
        viewer.frame(&mut app, events);
        assert!(!app.fs_exclusive_view.is_plane_view());
        assert_eq!((app.fs_zoom, app.fs_pan), zoom_pan);
        assert!(app.fs_boundary_hint.is_none());
        assert!(
            app.fullscreen_page_layout.plane_cover.is_some(),
            "ordinary paint publishes the first accepted record"
        );
        if keyboard {
            viewer.frame(&mut app, key(egui::Key::F13));
        } else {
            enter(&mut app, &mut viewer);
        }
        assert_eq!(app.plane_view_pose().unwrap().strength, 0.25);
    }
}

#[test]
fn plane_revision_cover_producer_rebinds_current_canvas_dpi_rotation_and_fit() {
    let _keys = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let viewer = viewer(&mut app);
    let accepted = app.fullscreen_page_layout.plane_cover.unwrap();
    let canvas = egui::Rect::from_min_size(egui::pos2(11., 17.), egui::vec2(614., 1093.));
    app.fs_free_rotation = 17_f32.to_radians();
    app.rotation_cache
        .insert(0, crate::rotation_db::Rotation::Cw90);
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Width;
    viewer.ctx.set_pixels_per_point(1.25);
    viewer
        .ctx
        .run_pass(
            egui::RawInput {
                screen_rect: Some(canvas),
                time: Some((app.frame_counter + 1) as f64 / 60.),
                ..Default::default()
            },
            |ctx| {
                let input = app.plane_cover_input_for_canvas(ctx, canvas).unwrap();
                assert_eq!(input.material, accepted.material);
                assert_eq!(input.transform.texture_size, accepted.input.texture_size);
                assert_eq!(input.transform.viewport_rect, canvas);
                assert_eq!(input.transform.pixels_per_point, ctx.pixels_per_point());
                assert_eq!(input.transform.rotation, crate::rotation_db::Rotation::Cw90);
                assert_eq!(input.transform.free_rotation_rad, app.fs_free_rotation);
                assert_eq!(input.transform.fit_mode.mode(), FullscreenFitMode::Width);
                assert_eq!(input.layout_source_size, accepted.layout_source_size);
            },
        )
        .drop_without_applying_deltas();
    app.fullscreen_page_slice = crate::page_split::PageSlice::Left;
    assert!(
        app.plane_cover_input_for_canvas(&viewer.ctx, canvas)
            .is_none(),
        "a new display unit cannot borrow the previous slice's material"
    );
}
