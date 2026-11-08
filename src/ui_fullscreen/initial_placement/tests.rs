use super::*;
use crate::gpu_lanczos::FullscreenPageMaterialKind;
use crate::settings::FullscreenPageStartPosition;
use crate::ui_fullscreen::page_turn_submission_tests::{frame, ready_app};

fn viewport() -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(30.0, 50.0), egui::vec2(600.0, 400.0))
}

fn overflowing_unit() -> egui::Rect {
    egui::Rect::from_center_size(viewport().center(), egui::vec2(900.0, 800.0))
}

fn reading_app(ctx: &egui::Context, mode: SpreadMode) -> crate::app::AppTestEnvForTest {
    let mut app = ready_app(ctx, mode);
    app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::ReadingStart;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Width;
    app
}

fn close_to(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1.01,
        "actual={actual}, expected={expected}"
    );
}

fn paint_single(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    rect: egui::Rect,
    selected_current: bool,
    rotation: crate::rotation_db::Rotation,
    trim: Option<egui::Rect>,
    placement: SingletonSpreadPlacement,
) -> DisplayedImageTransform {
    let idx = app.fullscreen_idx.unwrap();
    let material = app.resolve_fs_processed_material(ctx, idx, false);
    let resource = app.fullscreen_paint_payload_for_page(idx, material, false);
    let source_size = app
        .known_page_dims_for_idx(idx)
        .map(|(w, h)| egui::vec2(w as f32, h as f32));
    let zoom_pan = app.fs_zoom_pan();
    let fit = app.effective_fullscreen_fit_mode();
    let limits = app.fullscreen_fit_scale_limits(ctx.pixels_per_point());
    let transform = app
        .draw_fs_image_with_initial_placement(
            ui,
            rect,
            idx,
            source_size,
            FsPageLayoutSource::CurrentItem,
            None,
            None,
            placement,
            resource.as_ref(),
            None,
            false,
            false,
            false,
            rotation,
            zoom_pan,
            0.0,
            &FsBgStyle::Solid(egui::Color32::BLACK),
            "",
            false,
            fit,
            limits,
            trim,
            None,
            app.settings.page_turn_animation_enabled,
            None,
            page_turn::FlatCropPaint::WithPage,
            selected_current,
        )
        .expect("ready single page paints");
    app.fullscreen_page_layout
        .begin(FullscreenPageLayoutKind::Single);
    app.fullscreen_page_layout.push(transform);
    transform
}

fn paint_spread(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    rect: egui::Rect,
    current: bool,
) {
    let anchor = app.fullscreen_idx.unwrap();
    let composition = app.spread_display_composition_for_anchor(anchor);
    let pages = composition.pages_in_screen_order(app.spread_mode);
    assert_eq!(pages.len(), 2);
    app.draw_fs_spread_with_placement(
        ui,
        ctx,
        rect,
        pages[0].idx,
        pages[1].idx,
        false,
        FsPageTurnDecision::normal(),
        None,
        StillSeekGeometry::bar_only(),
        false,
        Some(&composition),
        current,
        None,
    );
}

fn arrival_viewport() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0))
}

fn thumbnail_arrival_app(
    ctx: &egui::Context,
    fit: FullscreenFitMode,
) -> crate::app::AppTestEnvForTest {
    let mut app = reading_app(ctx, SpreadMode::Single);
    app.settings.fullscreen_fit_mode = fit;
    app.fs_cache.clear();
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    let texture = ctx.load_texture(
        "initial-placement-thumbnail",
        egui::ColorImage::filled([150, 200], egui::Color32::WHITE),
        egui::TextureOptions::LINEAR,
    );
    app.thumbnails[0] = ThumbnailState::loaded_raster(
        texture,
        crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
            evaluated_display_px: 200,
        },
        false,
        200,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((1500, 2000)),
        None,
    );
    app.record_page_dims_for_spread(0, (1500, 2000));
    app.request_fs_initial_placement(0);
    app
}

fn install_arriving_full(app: &mut App, ctx: &egui::Context, idx: usize) {
    let pixels = Arc::new(egui::ColorImage::filled([1500, 2000], egui::Color32::WHITE));
    let texture = ctx.load_texture(
        format!("initial-placement-full-{idx}"),
        (*pixels).clone(),
        egui::TextureOptions::LINEAR,
    );
    app.fs_cache.insert(
        idx,
        FsCacheEntry::Static {
            pdf_rerender_failures: Vec::new(),
            pdf_metadata: None,
            tex: texture,
            pixels,
            source_dims: Some([1500, 2000]),
            load_seq: 2,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    app.record_page_dims_for_spread(idx, (1500, 2000));
}

/// Use the same selected full/thumbnail producers as ordinary viewport paint.
fn paint_prepared_single(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
) -> DisplayedImageTransform {
    paint_prepared_single_in_viewport(app, ui, ctx, arrival_viewport())
}

fn paint_prepared_single_in_viewport(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    rect: egui::Rect,
) -> DisplayedImageTransform {
    let idx = app.fullscreen_idx.unwrap();
    let state = app.prepare_fullscreen_state(ctx, idx, FsPageTurnDecision::normal());
    let selected = state
        .tex
        .as_ref()
        .or(state.thumb_tex.as_ref())
        .expect("selected material");
    if state.tex.is_none() {
        assert_eq!(
            selected.page_material_kind(),
            FullscreenPageMaterialKind::Provisional
        );
    }
    let payload = app.fullscreen_paint_payload_for_page(idx, state.tex.clone(), false);
    let source_size = app.fs_page_coordinate_source_size(idx);
    let zoom_pan = app.fs_zoom_pan();
    let fit = app.effective_fullscreen_fit_mode();
    let limits = app.fullscreen_fit_scale_limits(ctx.pixels_per_point());
    let transform = app
        .draw_fs_image_with_initial_placement(
            ui,
            rect,
            idx,
            source_size,
            FsPageLayoutSource::CurrentItem,
            None,
            None,
            SingletonSpreadPlacement::Center,
            payload.as_ref(),
            state.thumb_tex.as_ref(),
            false,
            false,
            state.fs_load_failed,
            crate::rotation_db::Rotation::None,
            zoom_pan,
            0.0,
            &FsBgStyle::Solid(egui::Color32::BLACK),
            "",
            false,
            fit,
            limits,
            None,
            None,
            false,
            None,
            page_turn::FlatCropPaint::WithPage,
            true,
        )
        .expect("selected page paints");
    app.fullscreen_page_layout
        .begin(FullscreenPageLayoutKind::Single);
    app.fullscreen_page_layout.push(transform);
    transform
}

fn placement_frame_in_rect(
    ctx: &egui::Context,
    rect: egui::Rect,
    mut paint: impl FnMut(&mut egui::Ui, &egui::Context),
) {
    crate::test_ui::headless_paint_output(ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        },
        |ctx| {
            ctx.root_ui(|root| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(root, |ui| paint(ui, ctx));
            })
        },
    ))
    .drop_without_applying_deltas();
}

#[test]
fn initial_placement_single_provisional_resize_restores_origin_before_complete_arrival() {
    let small = arrival_viewport();
    let large = egui::Rect::from_min_size(small.min, egui::vec2(600.0, 1000.0));
    for origin_pan in [egui::Vec2::ZERO, egui::vec2(17.0, 23.0)] {
        let ctx = egui::Context::default();
        let mut app = thumbnail_arrival_app(&ctx, FullscreenFitMode::Width);
        app.fs_pan = origin_pan;
        app.request_fs_initial_placement(0);
        placement_frame_in_rect(&ctx, small, |ui, ctx| {
            let t = paint_prepared_single_in_viewport(&mut app, ui, ctx, small);
            close_to(t.paint_rect.height(), 800.0);
            close_to(t.paint_rect.top(), small.top());
        });
        let request = app.fs_initial_placement_request.unwrap();
        assert_eq!(request.origin_pan, origin_pan);
        close_to(request.applied_pan.y, 200.0);
        assert_eq!(request.applied_pan, app.fs_pan);

        placement_frame_in_rect(&ctx, large, |ui, ctx| {
            let t = paint_prepared_single_in_viewport(&mut app, ui, ctx, large);
            assert!(t.paint_rect.height() < large.height());
            close_to(t.paint_rect.center().y, large.center().y + origin_pan.y);
            assert!(t.paint_rect.bottom() <= large.bottom());
            assert_eq!(app.fs_pan, origin_pan);
            let request = app.fs_initial_placement_request.expect("still provisional");
            assert_eq!(request.origin_pan, origin_pan);
            assert_eq!(request.applied_pan, origin_pan);
            assert_eq!((request.origin_zoom, request.applied_zoom), (1.0, 1.0));
        });
        install_arriving_full(&mut app, &ctx, 0);
        placement_frame_in_rect(&ctx, large, |ui, ctx| {
            let t = paint_prepared_single_in_viewport(&mut app, ui, ctx, large);
            assert_eq!(app.fs_pan, origin_pan);
            close_to(t.paint_rect.center().y, large.center().y + origin_pan.y);
            assert!(t.paint_rect.bottom() <= large.bottom());
            assert!(app.fs_initial_placement_request.is_none());
        });
    }
}

#[test]
fn initial_placement_spread_provisional_resize_restores_origin_before_complete_arrival() {
    let small = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 200.0));
    let large = egui::Rect::from_min_size(small.min, egui::vec2(600.0, 1000.0));
    for origin_pan in [egui::Vec2::ZERO, egui::vec2(17.0, 23.0)] {
        let ctx = egui::Context::default();
        let mut app = thumbnail_arrival_app(&ctx, FullscreenFitMode::Width);
        app.spread_mode = SpreadMode::Ltr;
        let texture = ctx.load_texture(
            "origin-spread-thumbnail",
            egui::ColorImage::filled([150, 200], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        app.thumbnails[1] = ThumbnailState::loaded_raster(
            texture,
            crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 200,
            },
            false,
            200,
            crate::source_coverage::SourceCoverage::FullSource,
            Some((1500, 2000)),
            None,
        );
        app.record_page_dims_for_spread(1, (1500, 2000));
        app.fs_pan = origin_pan;
        app.request_fs_initial_placement(0);
        let unit_rect = |app: &App| {
            app.fullscreen_page_layout
                .page_by_idx(0)
                .unwrap()
                .transform
                .paint_rect
                .union(
                    app.fullscreen_page_layout
                        .page_by_idx(1)
                        .unwrap()
                        .transform
                        .paint_rect,
                )
        };
        placement_frame_in_rect(&ctx, small, |ui, ctx| {
            paint_spread(&mut app, ui, ctx, small, true);
            assert!(unit_rect(&app).height() > small.height());
            close_to(unit_rect(&app).top(), small.top());
        });
        let request = app.fs_initial_placement_request.unwrap();
        assert_eq!(request.origin_pan, origin_pan);
        assert!(request.applied_pan.y > origin_pan.y);
        assert_eq!(request.applied_pan, app.fs_pan);
        placement_frame_in_rect(&ctx, large, |ui, ctx| {
            paint_spread(&mut app, ui, ctx, large, true);
            let unit = unit_rect(&app);
            assert!(unit.height() < large.height());
            close_to(unit.center().y, large.center().y + origin_pan.y);
            assert!(unit.bottom() <= large.bottom());
            assert_eq!(app.fs_pan, origin_pan);
            let request = app
                .fs_initial_placement_request
                .expect("both pages still provisional");
            assert_eq!(request.origin_pan, origin_pan);
            assert_eq!(request.applied_pan, origin_pan);
        });
        install_arriving_full(&mut app, &ctx, 0);
        install_arriving_full(&mut app, &ctx, 1);
        placement_frame_in_rect(&ctx, large, |ui, ctx| {
            paint_spread(&mut app, ui, ctx, large, true);
            assert_eq!(app.fs_pan, origin_pan);
            close_to(unit_rect(&app).center().y, large.center().y + origin_pan.y);
            assert!(unit_rect(&app).bottom() <= large.bottom());
            assert!(app.fs_initial_placement_request.is_none());
        });
    }
}

#[test]
fn initial_placement_r4_original_thumbnail_then_full_aligns_first_complete_frame() {
    let ctx = egui::Context::default();
    let mut app = thumbnail_arrival_app(&ctx, FullscreenFitMode::Original);
    frame(&ctx, |ui, ctx| {
        let t = paint_prepared_single(&mut app, ui, ctx);
        close_to(t.paint_rect.width(), 150.0);
        close_to(t.paint_rect.height(), 200.0);
        assert_eq!(app.fs_pan, egui::Vec2::ZERO);
        assert!(app.fs_initial_placement_request.is_some());
    });
    install_arriving_full(&mut app, &ctx, 0);
    frame(&ctx, |ui, ctx| {
        let t = paint_prepared_single(&mut app, ui, ctx);
        close_to(t.paint_rect.width(), 1500.0);
        close_to(t.paint_rect.height(), 2000.0);
        close_to(t.paint_rect.top(), 0.0);
        assert!(app.fs_initial_placement_request.is_none());
    });
}

#[test]
fn initial_placement_r4_incomplete_final_retains_until_actual_completed_ai_material() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
    app.settings.global_preset.upscale_model = Some(
        crate::ai::ModelKind::UpscaleRealEsrGeneralV3
            .as_str()
            .into(),
    );
    let source = Arc::new(egui::ColorImage::filled([400, 600], egui::Color32::WHITE));
    let source_texture = ctx.load_texture(
        "initial-placement-ai-source",
        (*source).clone(),
        egui::TextureOptions::LINEAR,
    );
    app.fs_cache.insert(
        0,
        FsCacheEntry::Static {
            pdf_rerender_failures: Vec::new(),
            pdf_metadata: None,
            tex: source_texture.clone(),
            pixels: source.clone(),
            source_dims: Some([400, 600]),
            load_seq: 1,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    app.record_page_dims_for_spread(0, (400, 600));
    let edit_key = app.current_edit_result_key(0);
    app.edit_result_cache.insert(
        edit_key,
        crate::app::EditResultEntry::raster(source.clone(), Some(source_texture.clone())),
    );
    let ai_key = app
        .final_ai_key_for_pixels(edit_key, [400, 600], app.effective_params(0))
        .expect("enabled upscale key");
    let final_key =
        app.final_composite_key_for_pixels(edit_key, [400, 600], app.effective_params(0));
    let canonical = Some(crate::page_dims::CanonicalPageDimensions::new(400, 600));
    app.final_composite_cache.insert(
        final_key,
        crate::app::FinalCompositeEntry::raster(
            canonical,
            [400, 600],
            source,
            source_texture,
            egui::TextureOptions::LINEAR,
            false,
        ),
    );
    // Model an already admitted worker without spawning inference or loading a model.
    let retained_epoch = app.retained_final_ai_epoch;
    app.final_ai_pending.insert(
        ai_key,
        crate::app::FinalAiPending {
            feedback_request: crate::app::FinalAiFeedbackRequest::default(),
            job_id: 1726,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            retained_key: None,
            retained_epoch,
            allow_retained_orphan: false,
            cancel_on_drop: true,
        },
    );
    app.request_fs_initial_placement(0);
    frame(&ctx, |ui, ctx| {
        let selected = app.resolve_fs_processed_material(ctx, 0, false).unwrap();
        assert_eq!(
            selected.page_material_kind(),
            FullscreenPageMaterialKind::Provisional
        );
        let t = paint_prepared_single(&mut app, ui, ctx);
        close_to(t.paint_rect.height(), 600.0);
        close_to(t.paint_rect.top(), 0.0);
        assert!(app.fs_initial_placement_request.is_some());
    });
    let output = Arc::new(egui::ColorImage::filled([1600, 2400], egui::Color32::WHITE));
    // The headless Context defaults to a 2048 texture cap. Preserve the real
    // 1600x2400 completed-material dimensions using the same manager lease
    // technique as display_basis_tests, without a physical GPU allocation.
    let manager = ctx.tex_manager();
    let id = manager.write().alloc(
        "initial-placement-ai-complete".into(),
        egui::ImageData::Color(output.clone()),
        egui::TextureOptions::LINEAR,
    );
    let output_texture = egui::TextureHandle::new(manager, id);
    app.final_ai_pending.remove(&ai_key);
    app.final_ai_cache.insert(
        ai_key,
        crate::app::FinalAiEntry {
            diagnostic_metadata: None,
            canonical_dims: canonical,
            source_size: [400, 600],
            pixels: output.clone(),
            used_upscale: true,
        },
    );
    app.final_composite_cache.insert(
        final_key,
        crate::app::FinalCompositeEntry::raster(
            canonical,
            [400, 600],
            output,
            output_texture,
            egui::TextureOptions::LINEAR,
            true,
        ),
    );
    frame(&ctx, |ui, ctx| {
        let selected = app.resolve_fs_processed_material(ctx, 0, false).unwrap();
        assert_eq!(
            selected.page_material_kind(),
            FullscreenPageMaterialKind::Complete
        );
        let t = paint_prepared_single(&mut app, ui, ctx);
        close_to(t.paint_rect.height(), 2400.0);
        close_to(t.paint_rect.top(), 0.0);
        assert!(app.fs_initial_placement_request.is_none());
    });
}

#[test]
fn initial_placement_r4_width_thumbnail_then_full_keeps_initial_pan() {
    let ctx = egui::Context::default();
    let mut app = thumbnail_arrival_app(&ctx, FullscreenFitMode::Width);
    frame(&ctx, |ui, ctx| {
        close_to(
            paint_prepared_single(&mut app, ui, ctx).paint_rect.top(),
            0.0,
        );
    });
    let provisional_pan = app.fs_pan;
    assert_ne!(provisional_pan, egui::Vec2::ZERO);
    let provisional_request = app
        .fs_initial_placement_request
        .expect("provisional request remains");
    assert_eq!(provisional_request.applied_pan, provisional_pan);
    assert_eq!(provisional_request.origin_pan, egui::Vec2::ZERO);
    install_arriving_full(&mut app, &ctx, 0);
    frame(&ctx, |ui, ctx| {
        close_to(
            paint_prepared_single(&mut app, ui, ctx).paint_rect.top(),
            0.0,
        );
        assert_eq!(app.fs_pan, provisional_pan);
        assert!(app.fs_initial_placement_request.is_none());
    });
}

#[test]
fn initial_placement_r4_manual_pan_during_thumbnail_survives_full_arrival() {
    let ctx = egui::Context::default();
    let mut app = thumbnail_arrival_app(&ctx, FullscreenFitMode::Width);
    frame(&ctx, |ui, ctx| {
        paint_prepared_single(&mut app, ui, ctx);
    });
    let proposed = app.fs_pan + egui::vec2(0.0, -75.0);
    let zoom = app.fs_zoom;
    app.set_fs_pan_from_input(zoom, proposed);
    let manual = app.fs_pan;
    assert!(app.fs_initial_placement_request.is_none());
    install_arriving_full(&mut app, &ctx, 0);
    frame(&ctx, |ui, ctx| {
        let t = paint_prepared_single(&mut app, ui, ctx);
        assert_eq!(app.fs_pan, manual);
        close_to(t.paint_rect.top(), -75.0);
    });
}

#[test]
fn initial_placement_r4_failed_full_load_keeps_stable_provisional_pan() {
    let ctx = egui::Context::default();
    let mut app = thumbnail_arrival_app(&ctx, FullscreenFitMode::Width);
    frame(&ctx, |ui, ctx| {
        paint_prepared_single(&mut app, ui, ctx);
    });
    let pan = app.fs_pan;
    let request = app.fs_initial_placement_request;
    app.fs_cache.insert(0, FsCacheEntry::Failed);
    assert_eq!(
        app.fs_page_load_state(0),
        crate::app::FsPageLoadState::LoadFailed
    );
    for _ in 0..3 {
        frame(&ctx, |ui, ctx| {
            close_to(
                paint_prepared_single(&mut app, ui, ctx).paint_rect.top(),
                0.0,
            );
            assert_eq!(app.fs_pan, pan);
            assert_eq!(app.fs_initial_placement_request, request);
        });
    }
    app.bump_items_generation();
    assert!(app.fs_initial_placement_request.is_none());
}

#[test]
fn initial_placement_r4_spread_waits_until_both_actual_materials_are_complete() {
    let ctx = egui::Context::default();
    let mut app = thumbnail_arrival_app(&ctx, FullscreenFitMode::Width);
    app.spread_mode = SpreadMode::Ltr;
    let texture = ctx.load_texture(
        "initial-placement-second-thumbnail",
        egui::ColorImage::filled([150, 200], egui::Color32::WHITE),
        egui::TextureOptions::LINEAR,
    );
    app.thumbnails[1] = ThumbnailState::loaded_raster(
        texture,
        crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
            evaluated_display_px: 200,
        },
        false,
        200,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((1500, 2000)),
        None,
    );
    app.record_page_dims_for_spread(1, (1500, 2000));
    install_arriving_full(&mut app, &ctx, 0);
    frame(&ctx, |ui, ctx| {
        paint_spread(&mut app, ui, ctx, arrival_viewport(), true);
        assert!(app.fullscreen_page_layout.page_by_idx(0).is_some());
        assert!(app.fullscreen_page_layout.page_by_idx(1).is_some());
        assert!(
            app.fs_initial_placement_request.is_some(),
            "one provisional page keeps the whole-unit request"
        );
    });
    install_arriving_full(&mut app, &ctx, 1);
    frame(&ctx, |ui, ctx| {
        paint_spread(&mut app, ui, ctx, arrival_viewport(), true);
        assert!(app.fs_initial_placement_request.is_none());
    });
}

#[test]
fn initial_placement_r4_actual_passthrough_producer_keeps_request_until_full_arrives() {
    let ctx = egui::Context::default();
    let mut app = thumbnail_arrival_app(&ctx, FullscreenFitMode::Width);
    app.thumb_pixels.insert(
        0,
        Arc::new(egui::ColorImage::filled([150, 200], egui::Color32::WHITE)),
    );
    let rendition = app
        .ensure_passthrough_material(&ctx, 0)
        .expect("resident thumbnail produces a rendition");
    assert_eq!(
        rendition.page_material_kind(),
        FullscreenPageMaterialKind::Provisional
    );
    let payload: FullscreenPaintPayload = rendition.into();
    frame(&ctx, |ui, _ctx| {
        let t = app
            .draw_fs_image_with_initial_placement(
                ui,
                arrival_viewport(),
                0,
                Some(egui::vec2(1500.0, 2000.0)),
                FsPageLayoutSource::CurrentItem,
                None,
                None,
                SingletonSpreadPlacement::Center,
                Some(&payload),
                None,
                false,
                false,
                false,
                crate::rotation_db::Rotation::None,
                None,
                0.0,
                &FsBgStyle::Solid(egui::Color32::BLACK),
                "",
                false,
                FullscreenFitMode::Width,
                FullscreenFitScaleLimits::default(),
                None,
                None,
                false,
                None,
                page_turn::FlatCropPaint::WithPage,
                true,
            )
            .expect("actual rendition paints");
        close_to(t.paint_rect.top(), 0.0);
        assert!(app.fs_initial_placement_request.is_some());
    });
    let pan = app.fs_pan;
    install_arriving_full(&mut app, &ctx, 0);
    frame(&ctx, |ui, ctx| {
        close_to(
            paint_prepared_single(&mut app, ui, ctx).paint_rect.top(),
            0.0,
        );
        assert_eq!(app.fs_pan, pan);
        assert!(app.fs_initial_placement_request.is_none());
    });
}

#[test]
fn initial_placement_rejected_native_uses_fitting_thumbnail_without_committing_candidate_pan() {
    let ctx = egui::Context::default();
    let mut app = thumbnail_arrival_app(&ctx, FullscreenFitMode::Original);
    let source_texture = ctx.load_texture(
        "initial-placement-rejected-native-source",
        egui::ColorImage::filled([1500, 2000], egui::Color32::WHITE),
        egui::TextureOptions::LINEAR,
    );
    let source = FullscreenPaintResource::resampleable(
        0,
        source_texture,
        crate::gpu_lanczos::FullscreenPaintSourceGeneration {
            items: app.items_generation,
            input: app.input_seq,
        },
    );
    // This captured Native has neither the current Original view's prepare key
    // nor a held binding that could authorize repainting it without its source.
    let native = crate::gpu_lanczos::test_native_resource(source, [300, 400]);
    let native = FullscreenPaintPayload::PresentationMaterial(native.capture_presentation(&ctx));
    assert!(native.source_texture().is_none());
    let native_id = native.paint_texture_id();
    let thumbnail = app
        .fs_thumbnail_resource_for_display(0)
        .expect("actual allowed thumbnail");
    let thumbnail_id = thumbnail.paint_texture_id();
    let output = frame(&ctx, |ui, _ctx| {
        let transform = app
            .draw_fs_image_with_initial_placement(
                ui,
                arrival_viewport(),
                0,
                Some(egui::vec2(1500.0, 2000.0)),
                FsPageLayoutSource::CurrentItem,
                None,
                None,
                SingletonSpreadPlacement::Center,
                Some(&native),
                Some(&thumbnail),
                false,
                false,
                false,
                crate::rotation_db::Rotation::None,
                None,
                0.0,
                &FsBgStyle::Solid(egui::Color32::BLACK),
                "",
                false,
                FullscreenFitMode::Original,
                FullscreenFitScaleLimits::default(),
                None,
                None,
                false,
                None,
                page_turn::FlatCropPaint::WithPage,
                true,
            )
            .expect("rejected Native falls back to thumbnail");
        close_to(transform.paint_rect.width(), 150.0);
        close_to(transform.paint_rect.height(), 200.0);
        close_to(
            transform.paint_rect.center().x,
            arrival_viewport().center().x,
        );
        close_to(
            transform.paint_rect.center().y,
            arrival_viewport().center().y,
        );
        assert_eq!(app.fs_pan, egui::Vec2::ZERO);
        let request = app
            .fs_initial_placement_request
            .expect("fallback is still provisional");
        assert_eq!(request.applied_pan, egui::Vec2::ZERO);
    });
    let painted_ids: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Mesh(mesh) => Some(mesh.texture_id),
            _ => None,
        })
        .collect();
    assert!(painted_ids.contains(&thumbnail_id));
    assert!(!painted_ids.contains(&native_id));
    output.drop_without_applying_deltas();
}

#[test]
fn initial_placement_actual_paint_without_material_preserves_request_and_pan() {
    let ctx = egui::Context::default();
    for spread in [false, true] {
        let mut app = reading_app(
            &ctx,
            if spread {
                SpreadMode::Ltr
            } else {
                SpreadMode::Single
            },
        );
        app.fs_cache.clear();
        app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
        app.request_fs_initial_placement(0);
        let request = app.fs_initial_placement_request;
        frame(&ctx, |ui, ctx| {
            if spread {
                paint_spread(&mut app, ui, ctx, arrival_viewport(), true);
            } else {
                let t = app.draw_fs_image_with_initial_placement(
                    ui,
                    arrival_viewport(),
                    0,
                    None,
                    FsPageLayoutSource::CurrentItem,
                    None,
                    None,
                    SingletonSpreadPlacement::Center,
                    None,
                    None,
                    false,
                    false,
                    false,
                    crate::rotation_db::Rotation::None,
                    None,
                    0.0,
                    &FsBgStyle::Solid(egui::Color32::BLACK),
                    "",
                    false,
                    FullscreenFitMode::Width,
                    FullscreenFitScaleLimits::default(),
                    None,
                    None,
                    false,
                    None,
                    page_turn::FlatCropPaint::WithPage,
                    true,
                );
                assert!(t.is_none());
            }
            assert_eq!(app.fs_initial_placement_request, request);
            assert_eq!(app.fs_pan, egui::Vec2::ZERO);
        });
    }
}

fn pointer_handler_frame(app: &mut App, ctx: &egui::Context, time: f64, events: Vec<egui::Event>) {
    install_fs_navigator_input_tracking(ctx);
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(arrival_viewport()),
            time: Some(time),
            focused: true,
            events,
            ..Default::default()
        },
        |ui| {
            let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
            app.handle_fs_wheel_and_click(ui, ctx, arrival_viewport(), &state, false, 0, &mut None);
        },
    )
    .drop_without_applying_deltas();
}

#[test]
fn initial_placement_actual_ctrl_wheel_route_cancels_request_before_changing_zoom() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    pointer_handler_frame(
        &mut app,
        &ctx,
        0.1,
        vec![
            egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
            egui::Event::PointerMoved(arrival_viewport().center()),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, 120.0),
                modifiers: egui::Modifiers::CTRL,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
            },
        ],
    );
    assert!(app.fs_zoom > 1.0);
    assert!(app.fs_initial_placement_request.is_none());
}

#[test]
fn initial_placement_actual_double_click_reset_cancels_request_and_centers() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    let button = |pressed| egui::Event::PointerButton {
        pos: arrival_viewport().center(),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    // Register and hover the backing Response before delivering its first
    // physical stream. Egui and the canvas owner both count accepted releases.
    pointer_handler_frame(&mut app, &ctx, 0.0, Vec::new());
    pointer_handler_frame(
        &mut app,
        &ctx,
        0.02,
        vec![egui::Event::PointerMoved(arrival_viewport().center())],
    );
    pointer_handler_frame(&mut app, &ctx, 0.04, vec![button(true)]);
    pointer_handler_frame(&mut app, &ctx, 0.06, vec![button(false)]);
    app.fs_zoom = 2.0;
    app.fs_pan = egui::vec2(40.0, 50.0);
    app.request_fs_initial_placement(0);
    pointer_handler_frame(
        &mut app,
        &ctx,
        0.20,
        vec![egui::Event::PointerMoved(arrival_viewport().center())],
    );
    pointer_handler_frame(&mut app, &ctx, 0.22, vec![button(true)]);
    pointer_handler_frame(&mut app, &ctx, 0.24, vec![button(false)]);
    assert!(ctx.input(|input| {
        input
            .pointer
            .button_double_clicked(egui::PointerButton::Primary)
    }));
    assert!(app.fs_initial_placement_request.is_none());
    assert_eq!(app.fs_zoom, 1.0);
    assert_eq!(app.fs_pan, egui::Vec2::ZERO);
}

#[cfg(windows)]
struct TestKeyboardFrameCleanup;

#[cfg(windows)]
impl Drop for TestKeyboardFrameCleanup {
    fn drop(&mut self) {
        crate::key_input::clear_test_frame();
    }
}

#[test]
fn initial_placement_actual_zoom_key_hold_start_cancels_request() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    #[cfg(windows)]
    let _cleanup = TestKeyboardFrameCleanup;
    #[cfg(windows)]
    {
        // Windows KeyHold reads the synthetic source-routed native level.
        let mut settings = crate::keymap::KeymapSettings::default();
        settings.set_override_chords(
            KeyAction::FsZoomMode,
            vec![crate::keymap::Chord::key(
                crate::keymap::KeyName::NumpadEnter,
            )],
        );
        app.keymap = crate::keymap::Keymap::from_settings(&settings);
        crate::key_input::set_test_frame_for_viewport(ctx.viewport_id(), Vec::new());
        crate::key_input::set_test_return_key_state(ctx.viewport_id(), false, true);
    }
    #[cfg(windows)]
    let events = Vec::new();
    #[cfg(not(windows))]
    let events = vec![egui::Event::Key {
        key: egui::Key::Z,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }];
    ctx.begin_pass(egui::RawInput {
        focused: true,
        events,
        ..Default::default()
    });
    crate::keyboard_input::cache_keyboard_owner(
        &ctx,
        crate::keyboard_input::KeyboardOwner::ApplicationShortcut {
            scope: crate::keyboard_input::ShortcutScope::new(
                ctx.viewport_id(),
                crate::keyboard_input::ShortcutSurface::Fullscreen,
            ),
        },
    );
    let permit = crate::keyboard_input::keyboard_input_permits(&ctx).current_level;
    assert!(permit.is_some());
    app.update_fs_zoom_mode_keys(&ctx, 0, permit);
    ctx.end_pass().drop_without_applying_deltas();
    assert!(app.fs_zoom_aiming);
    assert!(app.fs_initial_placement_request.is_none());
}

#[test]
fn initial_placement_items_generation_boundary_retires_before_next_paint() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    let generation = app.items_generation;
    app.bump_items_generation();
    assert_ne!(app.items_generation, generation);
    assert!(app.fs_initial_placement_request.is_none());
}

#[test]
fn initial_placement_accepted_open_creates_request_after_resetting_transform() {
    let ctx = egui::Context::default();
    for reading_start in [false, true] {
        let mut app = reading_app(&ctx, SpreadMode::Single);
        if !reading_start {
            app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::Center;
        }
        app.fs_zoom = 2.0;
        app.fs_pan = egui::vec2(34.0, 56.0);
        app.open_fullscreen(1, crate::app::HistoryTrigger::UserChosen);
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_eq!((app.fs_zoom, app.fs_pan), (1.0, egui::Vec2::ZERO));
        assert_eq!(app.fs_initial_placement_request.is_some(), reading_start);
        if let Some(request) = app.fs_initial_placement_request {
            assert_eq!(request.anchor_idx, 1);
            assert_eq!(request.items_generation, app.items_generation);
            assert_eq!(
                (request.origin_zoom, request.origin_pan),
                (1.0, egui::Vec2::ZERO)
            );
            assert_eq!(
                (request.applied_zoom, request.applied_pan),
                (1.0, egui::Vec2::ZERO)
            );
        }
    }
}

#[test]
fn initial_placement_geometry_aligns_only_overflowing_axes_and_respects_direction() {
    let v = viewport();
    let base = egui::vec2(17.0, -23.0);
    for direction in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
        let tall = egui::Rect::from_center_size(v.center(), egui::vec2(300.0, 800.0));
        assert_eq!(
            reading_start_pan(tall, v, direction, base),
            egui::vec2(base.x, 200.0)
        );
        let wide = egui::Rect::from_center_size(v.center(), egui::vec2(900.0, 200.0));
        let x = if direction == ReadingDirection::Ltr {
            150.0
        } else {
            -150.0
        };
        assert_eq!(
            reading_start_pan(wide, v, direction, base),
            egui::vec2(x, base.y)
        );
        assert_eq!(
            reading_start_pan(overflowing_unit(), v, direction, base),
            egui::vec2(x, 200.0)
        );
        assert_eq!(reading_start_pan(v, v, direction, base), base);
        let small = egui::Rect::from_min_size(v.min, egui::vec2(150.0, 200.0));
        assert_eq!(reading_start_pan(small, v, direction, base), base);
    }
}

#[test]
fn initial_placement_center_creates_no_request_and_does_not_change_split_pan() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.fs_pan = egui::vec2(24.0, -31.0);
    app.request_fs_initial_placement(0);
    assert!(app.fs_initial_placement_request.is_none());
    assert!(!app.resolve_fs_initial_placement(
        0,
        true,
        viewport(),
        Some(overflowing_unit()),
        FullscreenPageMaterialKind::Complete
    ));
    assert_eq!(app.fs_pan, egui::vec2(24.0, -31.0));
}

#[test]
fn initial_placement_holdover_and_missing_geometry_retain_until_first_current_frame() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    let request = app.fs_initial_placement_request;
    assert!(!app.resolve_fs_initial_placement(
        0,
        false,
        viewport(),
        Some(overflowing_unit()),
        FullscreenPageMaterialKind::Complete
    ));
    assert_eq!(app.fs_initial_placement_request, request);
    assert!(!app.resolve_fs_initial_placement(
        0,
        true,
        viewport(),
        None,
        FullscreenPageMaterialKind::Complete
    ));
    assert_eq!(app.fs_initial_placement_request, request);
    assert!(app.resolve_fs_initial_placement(
        0,
        true,
        viewport(),
        Some(overflowing_unit()),
        FullscreenPageMaterialKind::Complete
    ));
    assert!(app.fs_initial_placement_request.is_none());
    let pan = app.fs_pan;
    assert!(!app.resolve_fs_initial_placement(
        0,
        true,
        viewport(),
        Some(viewport()),
        FullscreenPageMaterialKind::Complete
    ));
    assert_eq!(app.fs_pan, pan);
}

#[test]
fn initial_placement_stale_identity_and_changed_transform_discard_without_moving() {
    let ctx = egui::Context::default();
    for change in 0..4 {
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.request_fs_initial_placement(0);
        let mut anchor = 0;
        match change {
            0 => app.items_generation += 1,
            1 => anchor = 1,
            2 => app.fs_zoom = 2.0,
            _ => app.fs_pan = egui::vec2(9.0, 12.0),
        }
        let before = (app.fs_zoom, app.fs_pan);
        assert!(!app.resolve_fs_initial_placement(
            anchor,
            false,
            viewport(),
            None,
            FullscreenPageMaterialKind::Complete
        ));
        assert!(app.fs_initial_placement_request.is_none(), "case {change}");
        assert_eq!((app.fs_zoom, app.fs_pan), before);
    }
}

#[test]
fn initial_placement_replacement_request_uses_latest_anchor_and_transform() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    app.fs_zoom = 2.0;
    app.fs_pan = egui::vec2(14.0, 22.0);
    app.request_fs_initial_placement(1);
    assert!(app.resolve_fs_initial_placement(
        1,
        true,
        viewport(),
        Some(viewport()),
        FullscreenPageMaterialKind::Complete
    ));
    assert_eq!((app.fs_zoom, app.fs_pan), (2.0, egui::vec2(14.0, 22.0)));
}

#[test]
fn initial_placement_single_uses_book_direction_when_mode_has_no_direction() {
    let ctx = egui::Context::default();
    for (mode, book, expected_x) in [
        (SpreadMode::Single, ReadingDirection::Ltr, 150.0),
        (SpreadMode::Single, ReadingDirection::Rtl, -150.0),
        (SpreadMode::Ltr, ReadingDirection::Rtl, 150.0),
        (SpreadMode::Rtl, ReadingDirection::Ltr, -150.0),
    ] {
        let mut app = reading_app(&ctx, mode);
        app.reading_direction = book;
        app.request_fs_initial_placement(0);
        assert!(app.resolve_fs_initial_placement(
            0,
            true,
            viewport(),
            Some(overflowing_unit()),
            FullscreenPageMaterialKind::Complete
        ));
        assert_eq!(app.fs_pan, egui::vec2(expected_x, 200.0));
    }
}

#[test]
fn initial_placement_single_paint_starts_at_top_in_the_first_painted_frame() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    frame(&ctx, |ui, ctx| {
        let t = paint_single(
            &mut app,
            ui,
            ctx,
            viewport(),
            true,
            crate::rotation_db::Rotation::None,
            None,
            SingletonSpreadPlacement::Center,
        );
        assert!(t.paint_rect.height() > viewport().height());
        close_to(t.paint_rect.top(), viewport().top());
        close_to(t.paint_rect.center().x, viewport().center().x);
        assert!(app.fs_initial_placement_request.is_none());
        assert_eq!(app.fs_zoom, 1.0);
    });
    let pan = app.fs_pan;
    frame(&ctx, |ui, ctx| {
        let resized = egui::Rect::from_min_size(viewport().min, egui::vec2(500.0, 300.0));
        paint_single(
            &mut app,
            ui,
            ctx,
            resized,
            true,
            crate::rotation_db::Rotation::None,
            None,
            SingletonSpreadPlacement::Center,
        );
    });
    assert_eq!(app.fs_pan, pan, "resize must not reapply entry alignment");
}

#[test]
fn initial_placement_single_paint_respects_trim_rotation_and_reserved_viewport() {
    let ctx = egui::Context::default();
    for direction in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.reading_direction = direction;
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Height;
        app.request_fs_initial_placement(0);
        frame(&ctx, |ui, ctx| {
            let t = paint_single(
                &mut app,
                ui,
                ctx,
                viewport(),
                true,
                crate::rotation_db::Rotation::Cw90,
                Some(egui::Rect::from_min_max(
                    egui::pos2(0.1, 0.0),
                    egui::pos2(0.9, 1.0),
                )),
                SingletonSpreadPlacement::Center,
            );
            assert!(t.paint_rect.width() > viewport().width());
            match direction {
                ReadingDirection::Ltr => close_to(t.paint_rect.left(), viewport().left()),
                ReadingDirection::Rtl => close_to(t.paint_rect.right(), viewport().right()),
            }
            close_to(t.paint_rect.center().y, viewport().center().y);
            assert!(app.fs_initial_placement_request.is_none());
        });
    }
}

#[test]
fn initial_placement_single_paint_keeps_nonoverflowing_singleton_side() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::LtrCover);
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
    let mut before = None;
    frame(&ctx, |ui, ctx| {
        before = Some(paint_single(
            &mut app,
            ui,
            ctx,
            viewport(),
            true,
            crate::rotation_db::Rotation::None,
            None,
            SingletonSpreadPlacement::Right,
        ));
    });
    app.request_fs_initial_placement(0);
    frame(&ctx, |ui, ctx| {
        let after = paint_single(
            &mut app,
            ui,
            ctx,
            viewport(),
            true,
            crate::rotation_db::Rotation::None,
            None,
            SingletonSpreadPlacement::Right,
        );
        assert_eq!(after.paint_rect, before.unwrap().paint_rect);
        assert_eq!(app.fs_pan, egui::Vec2::ZERO);
    });
}

#[test]
fn initial_placement_white_companion_union_controls_overflow_and_reading_start_edge() {
    let ctx = egui::Context::default();
    for (direction, placement) in [
        (ReadingDirection::Ltr, SingletonSpreadPlacement::RightWhite),
        (ReadingDirection::Rtl, SingletonSpreadPlacement::LeftWhite),
    ] {
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.reading_direction = direction;
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
        app.settings.spread_page_gap_px = 12;
        app.fs_zoom = 4.0;
        app.request_fs_initial_placement(0);
        frame(&ctx, |ui, ctx| {
            let transform = paint_single(
                &mut app,
                ui,
                ctx,
                viewport(),
                true,
                crate::rotation_db::Rotation::None,
                None,
                placement,
            );
            let white = white_companion_rect(&transform).expect("painted white companion");
            let unit = transform.paint_rect.union(white);
            assert!(
                transform.paint_rect.width() < viewport().width(),
                "the real page alone fits"
            );
            assert!(
                unit.width() > viewport().width(),
                "only the full display unit overflows horizontally"
            );
            match direction {
                ReadingDirection::Ltr => close_to(unit.left(), viewport().left()),
                ReadingDirection::Rtl => close_to(unit.right(), viewport().right()),
            }
            close_to(unit.top(), viewport().top());
            assert_eq!(app.fs_zoom, 4.0);
            assert!(app.fs_initial_placement_request.is_none());
        });
    }
}

#[test]
fn initial_placement_selected_holdover_does_not_consume_single_or_spread_request() {
    let ctx = egui::Context::default();
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        let mut app = reading_app(&ctx, mode);
        app.request_fs_initial_placement(0);
        frame(&ctx, |ui, ctx| {
            if mode == SpreadMode::Single {
                paint_single(
                    &mut app,
                    ui,
                    ctx,
                    viewport(),
                    false,
                    crate::rotation_db::Rotation::None,
                    None,
                    SingletonSpreadPlacement::Center,
                );
            } else {
                paint_spread(&mut app, ui, ctx, viewport(), false);
            }
            assert!(app.fs_initial_placement_request.is_some());
            assert_eq!(app.fs_pan, egui::Vec2::ZERO);
        });
    }
}

#[test]
fn initial_placement_spread_paint_aligns_unit_union_in_the_first_frame() {
    let ctx = egui::Context::default();
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        let mut app = reading_app(&ctx, mode);
        app.request_fs_initial_placement(0);
        frame(&ctx, |ui, ctx| {
            paint_spread(&mut app, ui, ctx, viewport(), true);
            let left = app
                .fullscreen_page_layout
                .page_by_idx(0)
                .expect("page zero")
                .transform;
            let right = app
                .fullscreen_page_layout
                .page_by_idx(1)
                .expect("page one")
                .transform;
            let union = left.paint_rect.union(right.paint_rect);
            assert!(union.height() > viewport().height());
            close_to(union.top(), viewport().top());
            close_to(union.center().x, viewport().center().x);
            assert!(app.fs_initial_placement_request.is_none());
            assert_eq!(app.fs_zoom, 1.0);
        });
    }
}

#[test]
fn initial_placement_navigation_holdover_preserves_old_single_and_spread_position() {
    let ctx = egui::Context::default();
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        let mut app = reading_app(&ctx, mode);
        app.request_fs_initial_placement(0);
        let mut old_rect = None;
        frame(&ctx, |ui, ctx| {
            if mode == SpreadMode::Single {
                paint_single(
                    &mut app,
                    ui,
                    ctx,
                    viewport(),
                    true,
                    crate::rotation_db::Rotation::None,
                    None,
                    SingletonSpreadPlacement::Center,
                );
            } else {
                paint_spread(&mut app, ui, ctx, viewport(), true);
            }
            old_rect = Some(
                app.fullscreen_page_layout
                    .page_by_idx(0)
                    .unwrap()
                    .transform
                    .paint_rect,
            );
        });
        assert_ne!(app.fs_pan, egui::Vec2::ZERO);
        let unit = app
            .capture_fs_display_unit_with_rendition(Some(&ctx), 0)
            .expect("capture painted unit");
        app.fullscreen_idx = Some(2);
        app.fs_pan = egui::Vec2::ZERO;
        app.request_fs_initial_placement(2);
        frame(&ctx, |ui, ctx| {
            app.draw_fs_display_unit_holdover_for_selection(
                ui,
                ctx,
                viewport(),
                &unit,
                StillSeekGeometry::bar_only(),
                FsHoldoverTransform::NavigationPrevious,
            );
            let held = app
                .fullscreen_page_layout
                .page_by_idx(0)
                .unwrap()
                .transform
                .paint_rect;
            close_to(held.left(), old_rect.unwrap().left());
            close_to(held.top(), old_rect.unwrap().top());
            assert_eq!(app.fs_pan, egui::Vec2::ZERO);
            assert!(app.fs_initial_placement_request.is_some());
        });
    }
}

#[test]
fn initial_placement_source_reload_holdover_keeps_live_manual_pan() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    frame(&ctx, |ui, ctx| {
        paint_single(
            &mut app,
            ui,
            ctx,
            viewport(),
            true,
            crate::rotation_db::Rotation::None,
            None,
            SingletonSpreadPlacement::Center,
        );
    });
    let unit = app
        .capture_fs_display_unit_with_rendition(Some(&ctx), 0)
        .unwrap();
    let old_top = app
        .fullscreen_page_layout
        .page_by_idx(0)
        .unwrap()
        .transform
        .paint_rect
        .top();
    app.fs_pan.y += 35.0;
    frame(&ctx, |ui, ctx| {
        app.draw_fs_display_unit_holdover_for_selection(
            ui,
            ctx,
            viewport(),
            &unit,
            StillSeekGeometry::bar_only(),
            FsHoldoverTransform::Current,
        );
        close_to(
            app.fullscreen_page_layout
                .page_by_idx(0)
                .unwrap()
                .transform
                .paint_rect
                .top(),
            old_top + 35.0,
        );
    });
}

#[test]
fn initial_placement_z_navigation_holdover_uses_the_existing_live_transform_path() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    frame(&ctx, |ui, ctx| {
        paint_single(
            &mut app,
            ui,
            ctx,
            viewport(),
            true,
            crate::rotation_db::Rotation::None,
            None,
            SingletonSpreadPlacement::Center,
        );
    });
    let mut unit = app
        .capture_fs_display_unit_with_rendition(Some(&ctx), 0)
        .unwrap();
    unit.layout_projection = FsDisplayUnitLayoutProjection::Painted(ResolvedDisplayPlacement::Z {
        active: true,
        factor: 2.0,
        zoom_pan: Some((2.0, egui::vec2(80.0, 120.0))),
        singleton_side: SingletonSpreadPlacement::Center,
        singleton_gap: 0.0,
    });
    app.fs_zoom = 1.25;
    app.fs_pan = egui::vec2(15.0, 25.0);
    let mut current = None;
    frame(&ctx, |ui, ctx| {
        app.draw_fs_display_unit_holdover_for_selection(
            ui,
            ctx,
            viewport(),
            &unit,
            StillSeekGeometry::bar_only(),
            FsHoldoverTransform::Current,
        );
        current = Some(app.fullscreen_page_layout.page_by_idx(0).unwrap().transform);
    });
    frame(&ctx, |ui, ctx| {
        app.draw_fs_display_unit_holdover_for_selection(
            ui,
            ctx,
            viewport(),
            &unit,
            StillSeekGeometry::bar_only(),
            FsHoldoverTransform::NavigationPrevious,
        );
        let previous = app.fullscreen_page_layout.page_by_idx(0).unwrap().transform;
        assert_eq!(previous.paint_rect, current.unwrap().paint_rect);
        assert_eq!(previous.placement, current.unwrap().placement);
    });
}

#[test]
fn initial_placement_offered_animation_target_submits_aligned_flat_in_first_frame() {
    let ctx = egui::Context::default();
    page_turn::install(&ctx);
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    for idx in [1, 0] {
        app.fullscreen_idx = Some(idx);
        frame(&ctx, |ui, ctx| {
            paint_submitted_single(&mut app, ui, ctx);
        });
    }
    frame(&ctx, |ui, ctx| {
        app.page_turn_external_operation(
            ctx,
            ViewerOperationId::Slideshow(next_operation_batch()),
            OperationPhase::Press,
            "slideshow",
        );
        app.offer_page_turn(ctx, 1, true);
        assert!(
            app.page_turn_presentation_pending(),
            "a real offered animation must precede alignment"
        );
        app.fullscreen_idx = Some(1);
        app.request_fs_initial_placement(1);
        let _decision = app.fs_page_turn_decision_for_frame(ctx, 1);
        let (transform, sources) = paint_submitted_single(&mut app, ui, ctx);
        close_to(transform.paint_rect.top(), ui.max_rect().top());
        assert_ne!(app.fs_pan, egui::Vec2::ZERO);
        let FsDisplayUnitSubmission::Paged(receipt) = &sources.submission else {
            panic!("actual flat target submission");
        };
        assert!(
            !receipt.curl,
            "nonzero target pan cancels the offered curl before submission"
        );
        assert!(app.fs_initial_placement_request.is_none());
    });
}

fn paint_submitted_single(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
) -> (DisplayedImageTransform, FsNavigatorTextureSources) {
    let idx = app.fullscreen_idx.unwrap();
    let material = app.resolve_fs_processed_material(ctx, idx, false);
    let payload = app.fullscreen_paint_payload_for_page(idx, material, false);
    let mut sources = FsNavigatorTextureSources::default();
    let zoom_pan = app.fs_zoom_pan();
    let transform = app
        .draw_fs_image_with_initial_placement(
            ui,
            ui.max_rect(),
            idx,
            Some(egui::vec2(80.0, 120.0)),
            FsPageLayoutSource::CurrentItem,
            None,
            None,
            SingletonSpreadPlacement::Center,
            payload.as_ref(),
            None,
            false,
            false,
            false,
            crate::rotation_db::Rotation::None,
            zoom_pan,
            0.0,
            &FsBgStyle::Solid(egui::Color32::BLACK),
            "",
            false,
            app.effective_fullscreen_fit_mode(),
            FullscreenFitScaleLimits::default(),
            None,
            Some(&mut sources),
            true,
            None,
            page_turn::FlatCropPaint::WithPage,
            true,
        )
        .expect("selected single page submits");
    app.fullscreen_page_layout
        .begin(FullscreenPageLayoutKind::Single);
    app.fullscreen_page_layout.push(transform);
    let composition = app.spread_display_composition_for_anchor(idx);
    let occurrences = composition.pages_in_screen_order(app.spread_mode);
    assert!(sources.rebind_occurrences(&occurrences));
    (transform, sources)
}

#[test]
fn initial_placement_input_pan_pinch_and_key_zoom_cancel_even_when_values_return_to_base() {
    let ctx = egui::Context::default();
    for input in 0..4 {
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.request_fs_initial_placement(0);
        match input {
            0 => app.set_fs_pan_from_input(1.0, egui::vec2(10.0, 15.0)),
            1 => {
                assert!(app.apply_touch_zoom_factor_about_pivot(
                    1.25,
                    viewport().center(),
                    viewport().center()
                ));
            }
            2 => {
                assert!(app.apply_fs_key_zoom_step(&ctx, true));
            }
            _ => app.apply_touch_pan_delta(egui::vec2(10.0, 15.0), viewport()),
        }
        assert!(app.fs_initial_placement_request.is_none(), "input {input}");
        app.fs_zoom = 1.0;
        app.fs_pan = egui::Vec2::ZERO;
        assert!(!app.resolve_fs_initial_placement(
            0,
            true,
            viewport(),
            Some(overflowing_unit()),
            FullscreenPageMaterialKind::Complete
        ));
        assert_eq!(app.fs_pan, egui::Vec2::ZERO);
    }
}

#[test]
fn initial_placement_fit_continuous_and_edit_resets_clear_the_request() {
    let ctx = egui::Context::default();
    for reset in 0..3 {
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.request_fs_initial_placement(0);
        match reset {
            0 => app.reset_fullscreen_fit_transform(0),
            1 => app.reset_continuous_reading_transform(),
            _ => app.enter_page_edit_single_view(0),
        }
        assert!(app.fs_initial_placement_request.is_none(), "reset {reset}");
    }
}

#[test]
fn initial_placement_split_reconciliation_never_recreates_a_consumed_request() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::SplitLtr);
    // Exercise metadata arrival; a pre-existing portrait sampler would be the
    // higher-priority dimensions owner and hide this newly recorded landscape.
    app.fs_cache.remove(&0);
    app.record_page_dims_for_spread(0, (240, 120));
    app.fullscreen_page_slice = crate::page_split::PageSlice::Full;
    app.fs_pan = egui::vec2(27.0, 33.0);
    app.reconcile_fullscreen_page_slice(0);
    assert!(app.fullscreen_page_slice.is_half());
    assert!(app.fs_initial_placement_request.is_none());
    assert_eq!(app.fs_pan, egui::vec2(27.0, 33.0));
    app.rotation_cache
        .insert(0, crate::rotation_db::Rotation::Cw90);
    app.reconcile_fullscreen_page_slice(0);
    assert_eq!(
        app.fullscreen_page_slice,
        crate::page_split::PageSlice::Full
    );
    assert!(app.fs_initial_placement_request.is_none());
    assert_eq!(app.fs_pan, egui::vec2(27.0, 33.0));
    app.spread_mode = SpreadMode::Single;
    app.reconcile_fullscreen_page_slice(0);
    assert_eq!(
        app.fullscreen_page_slice,
        crate::page_split::PageSlice::Full
    );
    assert!(app.fs_initial_placement_request.is_none());
    assert_eq!(app.fs_pan, egui::vec2(27.0, 33.0));
}

#[test]
fn initial_placement_accepted_same_image_split_step_requests_only_reading_start() {
    let ctx = egui::Context::default();
    for reading_start in [false, true] {
        let mut app = reading_app(&ctx, SpreadMode::SplitLtr);
        if !reading_start {
            app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::Center;
        }
        app.record_page_dims_for_spread(0, (240, 120));
        app.fullscreen_page_slice = crate::page_split::PageSlice::Left;
        app.fs_zoom = 2.0;
        app.fs_pan = egui::vec2(13.0, 17.0);
        app.handle_fs_navigation(
            &ctx,
            false,
            false,
            None,
            None,
            None,
            FsPageNav::Split(crate::page_split::PresentationStep {
                source_idx: 0,
                slice: crate::page_split::PageSlice::Right,
            }),
            None,
            0,
        );
        assert_eq!(
            app.fullscreen_page_slice,
            crate::page_split::PageSlice::Right
        );
        assert_eq!(app.fs_zoom, 2.0);
        assert_eq!(app.fs_pan, egui::vec2(13.0, 17.0));
        assert_eq!(app.fs_initial_placement_request.is_some(), reading_start);
    }
}

#[test]
fn initial_placement_excluded_modes_retire_requests_without_changing_position() {
    let ctx = egui::Context::default();
    for mode in 0..12 {
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.request_fs_initial_placement(0);
        match mode {
            0 => app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::Center,
            1 => app.fs_zoom_active = true,
            2 => app.fs_zoom_aiming = true,
            3 => app.analysis_mode = true,
            4 => app.reading_flow = ReadingFlow::Vertical,
            5 => app.reading_flow = ReadingFlow::Horizontal,
            6 => app.compare_view_mode = crate::app::CompareViewMode::wipe(0.5),
            7 => app.view_trim_mode = true,
            8 => app.erase_mode = true,
            9 => app.items[0] = GridItem::Video(PathBuf::from("c:/test/entry-video.mp4")),
            10 => app.items[0] = GridItem::Audio(PathBuf::from("c:/test/entry-audio.mp3")),
            _ => {
                app.fs_exclusive_view = crate::fs_exclusive_view::FsExclusiveView::Depth(
                    crate::depth_parallax::DepthViewState::new(
                        crate::app::ViewerContextId::for_test(82),
                    ),
                )
            }
        }
        let before = (app.fs_zoom, app.fs_pan);
        app.retire_fs_initial_placement_request();
        assert!(app.fs_initial_placement_request.is_none(), "mode {mode}");
        assert_eq!((app.fs_zoom, app.fs_pan), before);
    }
}

#[test]
fn initial_placement_slideshow_same_image_slice_creates_request_without_reopening() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::SplitLtr);
    app.record_page_dims_for_spread(0, (240, 120));
    app.fullscreen_page_slice = crate::page_split::PageSlice::Left;
    app.fs_zoom = 2.0;
    app.fs_pan = egui::vec2(13.0, 17.0);
    let generation = app.items_generation;
    app.complete_book_slideshow_target(
        &ctx,
        0,
        Some(crate::page_split::PageSlice::Right),
        crate::app::book_composition_permission::BookTargetPurpose::Next,
    );
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(
        app.fullscreen_page_slice,
        crate::page_split::PageSlice::Right
    );
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.fs_zoom, 2.0);
    assert_eq!(app.fs_pan, egui::vec2(13.0, 17.0));
    let request = app
        .fs_initial_placement_request
        .expect("accepted same-image slideshow half");
    assert_eq!(request.anchor_idx, 0);
    assert_eq!(request.origin_zoom, 2.0);
    assert_eq!(request.origin_pan, egui::vec2(13.0, 17.0));
    assert_eq!(request.applied_zoom, 2.0);
    assert_eq!(request.applied_pan, egui::vec2(13.0, 17.0));
}

#[test]
fn initial_placement_close_clears_unresolved_request() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    app.close_fullscreen();
    assert!(app.fs_initial_placement_request.is_none());
    assert!(app.fullscreen_idx.is_none());
}

#[test]
fn initial_placement_panorama_retires_request() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    if let Some(FsCacheEntry::Static { source_dims, .. }) = app.fs_cache.get_mut(&0) {
        *source_dims = Some([2000, 1000]);
    }
    app.fs_exclusive_view = crate::fs_exclusive_view::FsExclusiveView::Panorama(
        crate::panorama::PanoramaState::new(0.0, 0.0, crate::panorama::PanoProjection::Perspective),
    );
    assert_eq!(app.fs_zoom_owner(), FsZoomOwner::Panorama);
    app.retire_fs_initial_placement_request();
    assert!(app.fs_initial_placement_request.is_none());
}

#[cfg(windows)]
#[test]
fn initial_placement_context_swap_keeps_sibling_requests_and_pan_separate() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    let main_request = app.fs_initial_placement_request;
    let child = app.build_window_context_for_test(8126, |owner| {
        assert!(owner.fs_initial_placement_request.is_none());
        owner.items = vec![GridItem::Image(PathBuf::from("c:/test/child-entry.png"))];
        owner.fullscreen_idx = Some(0);
        owner.items_generation = 93;
        owner.fs_pan = egui::vec2(12.0, 18.0);
        owner.request_fs_initial_placement(0);
    });
    assert_eq!(app.fs_initial_placement_request, main_request);
    app.with_window_context_for_test(child, |owner| {
        assert_eq!(
            owner.fs_initial_placement_request.unwrap().items_generation,
            93
        );
        assert!(owner.resolve_fs_initial_placement(
            0,
            true,
            viewport(),
            Some(overflowing_unit()),
            FullscreenPageMaterialKind::Complete
        ));
        assert_ne!(owner.fs_pan, egui::Vec2::ZERO);
    });
    assert_eq!(app.fs_initial_placement_request, main_request);
    assert_eq!(app.fs_pan, egui::Vec2::ZERO);
    app.with_window_context_for_test(child, |owner| {
        assert!(owner.fs_initial_placement_request.is_none())
    });
}

#[cfg(windows)]
#[test]
fn initial_placement_parked_geometry_resets_clear_only_the_affected_context_request() {
    let ctx = egui::Context::default();
    for reset in 0..3 {
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.request_fs_initial_placement(0);
        let main_request = app.fs_initial_placement_request;
        let child = app.build_window_context_for_test(8127 + reset, |owner| {
            owner.items = vec![GridItem::Image(PathBuf::from("c:/test/parked-entry.png"))];
            owner.fullscreen_idx = Some(0);
            owner.spread_mode = SpreadMode::LtrCover;
            owner.fs_pan = egui::vec2(12.0, 18.0);
            owner.request_fs_initial_placement(0);
        });
        match reset {
            0 => app.invalidate_final_cover_spread_display_in_parked_contexts(),
            1 => app.invalidate_singleton_spread_placement_in_parked_contexts(true, true),
            _ => app.invalidate_page_alone_in_parked_contexts(true, true),
        }
        assert_eq!(app.fs_initial_placement_request, main_request);
        app.with_window_context_for_test(child, |owner| {
            assert!(
                owner.fs_initial_placement_request.is_none(),
                "reset {reset}"
            );
            assert_eq!(owner.fs_pan, egui::Vec2::ZERO);
        });
    }
}

#[test]
fn initial_placement_page_turn_target_paints_aligned_before_disabling_nonzero_pan_animation() {
    let ctx = egui::Context::default();
    page_turn::install(&ctx);
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    app.request_fs_initial_placement(0);
    frame(&ctx, |ui, ctx| {
        let t = paint_single(
            &mut app,
            ui,
            ctx,
            viewport(),
            true,
            crate::rotation_db::Rotation::None,
            None,
            SingletonSpreadPlacement::Center,
        );
        close_to(t.paint_rect.top(), viewport().top());
        assert_ne!(app.fs_pan, egui::Vec2::ZERO);
        assert!(!app.page_turn_view_eligible(0));
    });
    assert!(app.fs_initial_placement_request.is_none());
}
