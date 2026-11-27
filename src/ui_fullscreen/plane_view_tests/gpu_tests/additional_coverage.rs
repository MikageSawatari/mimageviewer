//! Real input/selector/PDF-demand coverage; only worker dispatch and GPU registration are controlled.
use super::*;

fn register_textures(app: &App, ctx: &egui::Context) {
    let render = app.wgpu_render_state.as_ref().unwrap();
    let mut delta = ctx.tex_manager().write().take_delta();
    for (id, deltas) in std::mem::take(&mut delta.set) {
        for delta in deltas {
            render
                .renderer
                .write()
                .update_texture(&render.device, &render.queue, id, &delta);
        }
    }
}

fn drawn(app: &App) -> Arc<crate::plane_view_wgpu::PlaneFrameSnapshot> {
    drawn_at(app, "input/material pass")
}

fn drawn_at(app: &App, stage: &str) -> Arc<crate::plane_view_wgpu::PlaneFrameSnapshot> {
    let FsExclusiveView::PlaneView(state) = &app.fs_exclusive_view else {
        panic!("PlaneView must remain active");
    };
    state
        .last_drawn
        .clone()
        .unwrap_or_else(|| panic!("a plane callback must be painted at {stage}"))
}

fn register_final_texture(app: &mut App, ctx: &egui::Context) -> egui::TextureId {
    let material = app
        .ensure_final_composite_material(ctx, 0)
        .expect("fixture final material");
    let pixels = app
        .ensure_final_composite_pixels(ctx, 0)
        .expect("fixture final pixels");
    let render = app.wgpu_render_state.as_ref().unwrap();
    let id = material.source_texture().id();
    // RootDepthViewer deliberately drops FullOutput texture deltas. Reproduce
    // renderer registration using the retained final pixels, even after that pass.
    render.renderer.write().update_texture(
        &render.device,
        &render.queue,
        id,
        &egui::epaint::ImageDelta::full(
            (*pixels).clone(),
            crate::app::DISPLAY_IMAGE_TEXTURE_OPTIONS,
        ),
    );
    id
}

struct ClearPhysicalInput;
impl Drop for ClearPhysicalInput {
    fn drop(&mut self) {
        crate::key_input::clear_test_synthetic_input();
    }
}

fn physical_modifiers(modifiers: crate::key_input::SyntheticModifiers) {
    crate::key_input::clear_test_synthetic_input();
    crate::key_input::arm_test_synthetic_input(0xE918, egui::ViewportId::ROOT);
    let now = std::time::Instant::now();
    crate::key_input::enqueue_test_synthetic_command(crate::key_input::SyntheticKeyCommand::down(
        now,
        crate::key_input::SyntheticNavigationKey::Tab,
        modifiers,
    ));
    crate::key_input::advance_test_synthetic_input(now);
}

#[test]
fn plane_modifier_hold_selects_original_with_custom_binding_and_keeps_plane_paint() {
    let _keys = crate::key_input::lock_test_input();
    let _clear = ClearPhysicalInput;
    for (binding, hold) in [
        (
            "Ctrl",
            crate::key_input::SyntheticModifiers {
                ctrl: true,
                ..Default::default()
            },
        ),
        (
            "Alt",
            crate::key_input::SyntheticModifiers {
                alt: true,
                ..Default::default()
            },
        ),
    ] {
        crate::key_input::clear_test_synthetic_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        app.keymap = crate::keymap::Keymap::from_ini_str(&format!(
            "[FsImage]\nFsOriginalPreviewHold = {binding}\nFsLoupeHold = {binding}\n"
        ));
        let original = app.resolve_original_preview_tex(0).unwrap();
        let pixels = Arc::new(egui::ColorImage::filled(
            original.size(),
            egui::Color32::RED,
        ));
        let final_texture = viewer.ctx.load_texture(
            "plane visibly different adjusted material",
            (*pixels).clone(),
            crate::app::DISPLAY_IMAGE_TEXTURE_OPTIONS,
        );
        let key = app.final_composite_key_for_pixels(
            app.current_edit_result_key(0),
            original.size(),
            app.effective_params(0),
        );
        app.bind_display_texture_identity(&final_texture, key.edit_key.item_id);
        app.final_composite_cache.insert(
            key,
            crate::app::FinalCompositeEntry::raster(
                None,
                original.size(),
                pixels,
                final_texture.clone(),
                crate::app::DISPLAY_IMAGE_TEXTURE_OPTIONS,
                true,
            ),
        );
        assert_eq!(
            app.resolve_fs_display_tex(0, false).unwrap().id(),
            final_texture.id()
        );
        register_textures(&app, &viewer.ctx);
        enter(&mut app, &mut viewer);
        let pose = app.plane_view_pose().unwrap();
        assert_eq!(drawn(&app).material.copy.texture_id, final_texture.id());

        // These are physical levels, not RawInput.modifiers or a boolean passed to the selector.
        physical_modifiers(hold);
        viewer.frame(
            &mut app,
            vec![egui::Event::PointerMoved(egui::pos2(500., 300.))],
        );
        assert_eq!(app.plane_view_pose().unwrap(), pose);
        assert_eq!(
            drawn(&app).material.copy.texture_id,
            original.id(),
            "{binding}"
        );
        assert!(app.original_preview_active(&viewer.ctx, 0));
        assert!(app.fs_exclusive_view_active(0).is_some());
        let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, viewer.size);
        // Share RootDepthViewer's explicit f64 clock. An implicit pass advances
        // by predicted_dt (f32), which can overtake the next counter / 60 tick.
        app.frame_counter += 1;
        let time = app.frame_counter as f64 / 60.0;
        assert!(time > viewer.ctx.input(|input| input.time));
        let output = viewer.ctx.run_pass(
            egui::RawInput {
                screen_rect: Some(canvas),
                time: Some(time),
                events: vec![egui::Event::PointerMoved(egui::pos2(500., 300.))],
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
                app.keyboard_owner_for_pass(ctx);
                let permit = crate::keyboard_input::focused_key_state_permit(ctx).unwrap();
                assert!(
                    app.keymap
                        .modifier_held_action(ctx, permit, KeyAction::FsLoupeHold)
                );
                ctx.root_ui(|ui| app.draw_fs_loupe_if_active(ui, ctx, canvas, 0));
            },
        );
        assert!(
            output
                .shapes
                .iter()
                .all(|shape| matches!(shape.shape, egui::Shape::Noop)),
            "an admitted loupe hold must paint no loupe while PlaneView owns the canvas"
        );
        output.drop_without_applying_deltas();

        // ModifierHold reads its physical level, including while other modifiers
        // are held: a Ctrl reassignment overlaps the fisheye chord just as RightCtrl does.
        physical_modifiers(crate::key_input::SyntheticModifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        });
        viewer.frame(&mut app, vec![]);
        assert_eq!(
            app.original_preview_active(&viewer.ctx, 0),
            binding == "Ctrl"
        );
        assert_eq!(
            drawn(&app).material.copy.texture_id,
            if binding == "Ctrl" {
                original.id()
            } else {
                final_texture.id()
            }
        );
        assert_eq!(app.plane_view_pose().unwrap(), pose);

        crate::key_input::clear_test_synthetic_input();
        viewer.frame(&mut app, vec![]);
        assert_eq!(drawn(&app).material.copy.texture_id, final_texture.id());
    }
}

fn touch(id: u64, phase: egui::TouchPhase, pos: egui::Pos2) -> egui::Event {
    egui::Event::Touch {
        device_id: egui::TouchDeviceId(918),
        id: egui::TouchId(id),
        phase,
        pos,
        force: None,
    }
}

#[test]
fn plane_actual_two_contact_pinch_changes_image_zoom_without_changing_lens_or_page() {
    let _keys = crate::key_input::lock_test_input();
    crate::touch_correlation::with_gestures_disabled(false, || {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        app.settings.touch_still_chrome_learned = true;
        enter(&mut app, &mut viewer);
        app.fs_focus_regained_at = None;
        assert!(app.fs_prev_focused);
        let pose = app.plane_view_pose().unwrap();
        let zoom = app.fs_zoom;
        let center = egui::pos2(500., 300.);
        for (phase, radius) in [
            (egui::TouchPhase::Start, 60.),
            (egui::TouchPhase::Move, 90.),
            (egui::TouchPhase::Move, 120.),
        ] {
            let first = center - egui::vec2(radius, 0.);
            let mut events = vec![touch(1, phase, first), egui::Event::PointerMoved(first)];
            if phase == egui::TouchPhase::Start {
                events.push(egui::Event::PointerButton {
                    pos: first,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            // egui-winit only promotes its first contact to pointer events.
            events.push(touch(2, phase, center + egui::vec2(radius, 0.)));
            viewer.frame(&mut app, events);
        }
        assert!(
            app.fs_zoom > zoom * 1.5,
            "raw Touch events must reach the image zoom owner"
        );
        assert_eq!(app.plane_view_pose().unwrap(), pose);
        assert_eq!(app.fs_zoom_owner(), FsZoomOwner::Image);
        assert_eq!(app.fullscreen_idx, Some(0));
        let pinched_zoom = app.fs_zoom;
        viewer.frame(
            &mut app,
            vec![
                touch(1, egui::TouchPhase::End, center - egui::vec2(120., 0.)),
                egui::Event::PointerButton {
                    pos: center - egui::vec2(120., 0.),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerGone,
                touch(2, egui::TouchPhase::End, center + egui::vec2(120., 0.)),
                egui::Event::PointerGone,
            ],
        );
        assert_eq!(app.fs_zoom, pinched_zoom);
        assert_eq!(app.plane_view_pose().unwrap(), pose);
        assert_eq!(app.fullscreen_idx, Some(0));
    });
}

#[test]
fn plane_registration_wait_requests_the_owning_viewport_next_pass() {
    let _keys = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = viewer(&mut app);
    enter(&mut app, &mut viewer);
    let previous = drawn(&app).material.clone();
    let pixels = egui::ColorImage::filled([4, 6], egui::Color32::GREEN);
    let texture = viewer.ctx.load_texture(
        "plane registration wait",
        pixels.clone(),
        Default::default(),
    );
    let payload =
        FullscreenPaintPayload::from(app.fixture_raster_paint_resource(0, texture.clone()));
    // Drain egui's initial/input repaint requests before isolating material adoption.
    for _ in 0..3 {
        viewer
            .ctx
            .run_pass(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
    }
    let baseline = viewer.ctx.run_pass(egui::RawInput::default(), |_| {});
    assert_ne!(
        baseline.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
        std::time::Duration::ZERO
    );
    baseline.drop_without_applying_deltas();
    let output = viewer.ctx.run_pass(egui::RawInput::default(), |ctx| {
        let selected = app
            .select_plane_material(ctx, &payload, egui::vec2(4., 6.), egui::vec2(4., 6.))
            .unwrap();
        assert!(Arc::ptr_eq(&selected, &previous));
    });
    assert_eq!(
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
        std::time::Duration::ZERO
    );
    assert!(
        matches!(&app.fs_exclusive_view, FsExclusiveView::PlaneView(state)
        if matches!(state.color, PlaneColorPhase::Pending { .. }))
    );
    output.drop_without_applying_deltas();
    let render = app.wgpu_render_state.as_ref().unwrap();
    render.renderer.write().update_texture(
        &render.device,
        &render.queue,
        texture.id(),
        &egui::epaint::ImageDelta::full(pixels, Default::default()),
    );
    viewer
        .ctx
        .run_pass(egui::RawInput::default(), |ctx| {
            let selected = app
                .select_plane_material(ctx, &payload, egui::vec2(4., 6.), egui::vec2(4., 6.))
                .unwrap();
            assert_eq!(selected.copy.texture_id, texture.id());
            assert!(!Arc::ptr_eq(&selected, &previous));
        })
        .drop_without_applying_deltas();
    assert!(
        matches!(&app.fs_exclusive_view, FsExclusiveView::PlaneView(state)
        if matches!(state.color, PlaneColorPhase::Ready(_)))
    );
}

fn install_pdf_raster(
    app: &mut App,
    ctx: &egui::Context,
    probe: &crate::app::PdfRerenderRequestProbe,
    size: [usize; 2],
) {
    let generation = app.items_generation;
    assert!(probe.complete(
        app,
        generation,
        0,
        crate::fs_animation::FsLoadResult::StaticPdf {
            ci: egui::ColorImage::filled(size, egui::Color32::WHITE),
            source_dims: size,
            page_size_points: crate::pdf_loader::PdfPageSizePoints {
                width: 613.125,
                height: 791.875,
            },
            content_type: PdfPageContentType::Vector,
        }
    ));
    app.poll_prefetch(ctx, crate::app::PollPrefetchOrigin::TopLevel);
    assert_eq!(
        app.source_material_for_idx(0)
            .unwrap()
            .source_texture()
            .size(),
        size
    );
}

#[test]
fn plane_revision3_real_pdf_inward_outward_and_exit_keep_the_planned_geometry_across_reraster() {
    let _keys = crate::key_input::lock_test_input();
    for notches in [1., -0.5, -8.] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        let probe = crate::app::PdfRerenderRequestProbe::start();
        app.items[0] = GridItem::PdfPage {
            pdf_path: app.tmp.path().join("plane-revision3-geometry.pdf"),
            page_num: 0,
            content_type: Some(PdfPageContentType::Vector),
        };
        viewer.size = egui::vec2(1200., 900.);
        install_pdf_raster(&mut app, &viewer.ctx, &probe, [317, 409]);
        register_final_texture(&mut app, &viewer.ctx);
        viewer.frame(&mut app, vec![]);
        enter(&mut app, &mut viewer);
        let old = drawn(&app);
        let canvas = old.params.canvas();
        let before = (app.fs_zoom, app.fs_pan);
        let expected = plane_view_wheel_transition(wheel_input_for_current_view(
            &mut app,
            &viewer.ctx,
            egui::Rect::from_min_size(egui::Pos2::ZERO, viewer.size),
            Some(canvas.center()),
            notches,
        ));
        if notches < 0. {
            assert_eq!((expected.zoom, expected.pan), before);
        } else {
            assert!(expected.zoom <= before.0 * 1.5 + 1.0e-6);
        }
        // Complete a genuine PDF reraster before the same input pass; the current
        // texture is not registered yet, so active PlaneView still paints the old one.
        install_pdf_raster(&mut app, &viewer.ctx, &probe, [634, 819]);
        viewer.frame(
            &mut app,
            vec![wheel(
                0.,
                notches * 40.,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            )],
        );
        assert_eq!(app.plane_view_pose(), expected.pose);
        assert!((app.fs_zoom - expected.zoom).abs() < 1.0e-5);
        assert!((app.fs_pan - expected.pan).length() < 0.01);
        let pending = app
            .fullscreen_page_layout
            .displayed_pages()
            .next()
            .unwrap()
            .transform;
        let crate::displayed_image_transform::ResolvedDisplayPlacement::Normal {
            zoom_pan: Some((paint_zoom, paint_pan)),
        } = pending.placement
        else {
            panic!("the real single PDF paint must publish its normal zoom/pan");
        };
        assert!((paint_zoom - expected.zoom).abs() < 1.0e-5);
        assert!(
            (paint_pan - expected.pan).length() < 0.01,
            "paint-time pan correction must not move the planned placement"
        );
        if expected.pose.is_some() {
            assert!(Arc::ptr_eq(&drawn(&app).material, &old.material));
        }
        let planned = app
            .plane_cover_input_for_canvas(&viewer.ctx, canvas)
            .unwrap();
        let resolved = planned
            .resolve(
                expected.zoom,
                expected.pan,
                crate::displayed_image_transform::RectPixelFit::Texels,
            )
            .unwrap();
        let assert_same_board =
            |actual: &crate::displayed_image_transform::DisplayedImageTransform,
             expected: &crate::displayed_image_transform::DisplayedImageTransform| {
                for source in [
                    egui::pos2(0., 0.),
                    egui::pos2(1., 0.),
                    egui::pos2(1., 1.),
                    egui::pos2(0., 1.),
                ] {
                    assert!(
                        (actual.source_normalized_to_screen(source)
                            - expected.source_normalized_to_screen(source))
                        .length()
                            <= 1. / 64.,
                        "PDF paint differs from its canonical material geometry at source {source:?}"
                    );
                }
            };
        assert_same_board(&pending, &resolved);
        register_final_texture(&mut app, &viewer.ctx);
        viewer.frame(&mut app, vec![]);
        assert_eq!((app.fs_zoom, app.fs_pan), (expected.zoom, expected.pan));
        let adopted = app
            .fullscreen_page_layout
            .displayed_pages()
            .next()
            .unwrap()
            .transform;
        let adopted_input = app
            .plane_cover_input_for_canvas(&viewer.ctx, canvas)
            .unwrap();
        let adopted_expected = adopted_input
            .resolve(
                expected.zoom,
                expected.pan,
                crate::displayed_image_transform::RectPixelFit::Texels,
            )
            .unwrap();
        // PDF raster rounding may change the sampler aspect and its contained paint
        // rectangle. The canonical page placement must stay fixed across that change.
        assert!(placement_frames_match(
            pending.placement_frame,
            adopted.placement_frame,
            adopted.pixels_per_point,
        ));
        assert!(placement_frames_match(
            resolved.layout_full_image_rect,
            adopted.layout_full_image_rect,
            adopted.pixels_per_point,
        ));
        assert_same_board(&adopted, &adopted_expected);
        if expected.pose.is_some() {
            assert_eq!(
                drawn(&app).material.description.texture_size,
                egui::vec2(634., 819.)
            );
        }
        // Exit hands the very same placement to the ordinary renderer.
        if expected.pose.is_none() {
            assert_eq!((app.fs_zoom, app.fs_pan), before);
        }
    }
}

#[test]
fn plane_real_pdf_routes_keep_both_demands_current_while_previous_material_is_painted() {
    let _keys = crate::key_input::lock_test_input();
    for tilt_only in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        let probe = crate::app::PdfRerenderRequestProbe::start();
        app.items[0] = GridItem::PdfPage {
            pdf_path: app.tmp.path().join("plane-demands.pdf"),
            page_num: 0,
            content_type: Some(PdfPageContentType::Vector),
        };
        viewer.size = egui::vec2(1200., 900.);
        install_pdf_raster(&mut app, &viewer.ctx, &probe, [317, 409]);
        register_final_texture(&mut app, &viewer.ctx);
        viewer.frame(&mut app, vec![]);
        if tilt_only {
            super::tilt_input_tests::enter_tilt(&mut viewer, &mut app);
            assert_eq!(
                app.fs_zoom, 1.,
                "tilt-only entry preserves the 2D PDF demand scale"
            );
            assert_eq!(app.plane_view_pose().unwrap().strength, 0.);
        } else {
            enter(&mut app, &mut viewer);
        }
        let old = drawn_at(&app, "first PDF raster registered and entered");
        assert_eq!(
            old.material.description.texture_size,
            egui::vec2(317., 409.)
        );
        probe.take_requests();

        // Adopt a real reraster completion, leaving its newly selected final texture
        // unregistered until after the UI pass.
        let previous_generation = app.current_edit_result_key(0).source_gen;
        install_pdf_raster(&mut app, &viewer.ctx, &probe, [634, 819]);
        assert_ne!(
            app.current_edit_result_key(0).source_gen,
            previous_generation
        );
        assert!(
            probe.take_requests().is_empty(),
            "adopting the completed raster has not issued either display request"
        );
        viewer.frame(&mut app, vec![wheel(0., 40., egui::Modifiers::CTRL)]);
        let snapshot = drawn_at(&app, "reraster accepted but current texture unregistered");
        assert!(Arc::ptr_eq(&snapshot.material, &old.material));
        assert!(
            matches!(&app.fs_exclusive_view, FsExclusiveView::PlaneView(state)
        if matches!(state.color, PlaneColorPhase::Pending { .. }))
        );
        let viewport = app.fs_pdf_display_target.unwrap();
        let base = crate::pdf_loader::display_render_long_edge(
            viewport,
            634.,
            819.,
            false,
            PdfPageContentType::Vector,
        );
        let legacy_target = crate::pdf_loader::zoom_render_long_edge(base, app.fs_zoom, None);
        // Before input, ensure_pdf_display_resolution reads the accepted current
        // raster (634x819), whose 819px edge is below 90% of the fit request (991).
        // Then the wheel calls maybe_rerender_pdf at the new 2D zoom. Tilt is not a demand input.
        // Neither entry uses the previous PlaneView material's drawn transform.
        assert!(819. < base as f32 * 0.9);
        assert_ne!(
            base, legacy_target,
            "the fixture distinguishes fit and wheel requests"
        );
        assert_eq!(
            probe.take_requests(),
            vec![(0, base), (0, legacy_target)],
            "pre-input current-raster fit resolution precedes the real wheel zoom demand"
        );

        app.settings.fullscreen_fit_mode = FullscreenFitMode::SpecifiedScale;
        app.settings.fullscreen_specified_scale = crate::settings::FullscreenSpecifiedScale {
            width: crate::settings::SpecifiedScaleAxisSetting::disabled(
                crate::settings::SpecifiedScaleAxis::PercentTenths(800),
            ),
            height: crate::settings::SpecifiedScaleAxisSetting::enabled(
                crate::settings::SpecifiedScaleAxis::LogicalPixels(1200),
            ),
        };
        viewer.frame(&mut app, vec![]);
        let painted = app
            .fullscreen_page_layout
            .displayed_pages()
            .next()
            .unwrap()
            .transform;
        assert_eq!(painted.texture_size, egui::vec2(317., 409.));
        assert!(Arc::ptr_eq(
            &drawn_at(
                &app,
                "specified-scale PDF demand while current texture is pending"
            )
            .material,
            &old.material
        ));
        let current = app.fs_fit_payload_for_idx(0).unwrap();
        let source = app.source_dims_for_idx(0).unwrap();
        let current_geometry = resolve_fs_image_transform(
            DisplayedImageTransformInput {
                page_idx: 0,
                viewport_rect: painted.viewport_rect,
                source_size: egui::vec2(source.0, source.1),
                texture_size: current.full_source_size_vec2(),
                rotation: crate::rotation_db::Rotation::None,
                free_rotation_rad: 0.,
                content_bbox: None,
                fit_mode: app
                    .fs_fit_input_for_payload(0, FullscreenFitMode::SpecifiedScale, &current)
                    .unwrap(),
                fit_scale_limits: app.fullscreen_fit_scale_limits(viewer.ctx.pixels_per_point()),
                pixels_per_point: viewer.ctx.pixels_per_point(),
                placement: ResolvedDisplayPlacement::Normal {
                    zoom_pan: app.fs_zoom_pan(),
                },
                pixel_fit: RectPixelFit::Texels,
            },
            None,
        )
        .unwrap();
        let required = current_geometry.layout_full_image_rect.size().max_elem()
            * viewer.ctx.pixels_per_point();
        let absolute_target = crate::pdf_loader::absolute_display_render_long_edge(
            required,
            false,
            PdfPageContentType::Vector,
        );
        assert_eq!(
            probe.take_requests(),
            vec![(0, absolute_target)],
            "the actual post-draw layout demand stays enabled while previous pixels paint"
        );
        assert_ne!(
            legacy_target, absolute_target,
            "fixture must distinguish the two request policies"
        );
        assert_eq!(
            painted.source_size,
            egui::vec2(
                old.material.description.source_size[0] as f32,
                old.material.description.source_size[1] as f32
            )
        );
        // Normal renderer registration ends the same pending interval without a
        // replacement request or a change to the already issued absolute demand.
        let current_id = register_final_texture(&mut app, &viewer.ctx);
        viewer.frame(&mut app, vec![]);
        assert_eq!(
            drawn_at(&app, "current PDF renderer registration completed")
                .material
                .copy
                .texture_id,
            current_id
        );
        assert_eq!(
            drawn(&app).material.description.texture_size,
            egui::vec2(634., 819.)
        );
        assert_eq!(
            old.material.description.texture_size,
            egui::vec2(317., 409.)
        );
        assert!(probe.take_requests().is_empty());
    }
}
