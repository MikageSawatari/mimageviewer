//! Accepted resident materials retain the ordinary selector's completion contract.
use super::*;
use crate::gpu_lanczos::FullscreenPageMaterialKind;
use crate::settings::FullscreenPageStartPosition;
use crate::ui_fullscreen::page_turn_submission_tests::{frame, ready_app};

fn draw_current_with_initial_placement(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
) -> FsNavigatorTextureSources {
    let idx = app.fullscreen_idx.unwrap();
    let decision = app.fs_page_turn_decision_for_frame(ctx, idx);
    assert!(
        app.fs_nav_holdover_for_draw().is_none(),
        "the accepted resident target must reach the current-page drawing path"
    );
    let composition = app.spread_display_composition_for_anchor(idx);
    let screen = composition.pages_in_screen_order(app.spread_mode);
    assert_eq!(screen.len(), 1);
    let material = app.resolve_fs_processed_material(ctx, idx, false);
    let payload = app.fullscreen_paint_payload_for_page(idx, material, false);
    let source_size = app.fs_page_coordinate_source_size(idx);
    let zoom_pan = app.fs_zoom_pan();
    let fit = app.effective_fullscreen_fit_mode();
    let limits = app.fullscreen_fit_scale_limits(ctx.pixels_per_point());
    let live = app.page_turn_view_eligible(idx);
    let mut sources = FsNavigatorTextureSources::default();
    let transform = app
        .draw_fs_image_with_initial_placement(
            ui,
            ui.max_rect(),
            idx,
            source_size,
            FsPageLayoutSource::CurrentItem,
            None,
            None,
            composition.singleton_placement(),
            payload.as_ref(),
            None,
            false,
            false,
            false,
            crate::rotation_db::Rotation::None,
            zoom_pan,
            0.0,
            &FsBgStyle::Solid(egui::Color32::WHITE),
            "",
            false,
            fit,
            limits,
            None,
            Some(&mut sources),
            live,
            None,
            FlatCropPaint::WithPage,
            true,
        )
        .expect("the selected resident single page paints");
    app.fullscreen_page_layout
        .begin(FullscreenPageLayoutKind::Single);
    app.fullscreen_page_layout
        .push_occurrence(transform, screen[0]);
    assert!(sources.rebind_occurrences(&screen));
    app.emit_fs_page_turn_ready_for_display_unit(ctx, &composition, decision, &sources);
    sources
}

fn close_to(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1.01,
        "actual={actual}, expected={expected}"
    );
}

fn assert_resident_material(
    app: &App,
    quality: &'static str,
    texture: egui::TextureId,
    kind: FullscreenPageMaterialKind,
) {
    let (resource, selected_quality) = app.page_turn_resident_source(0).unwrap();
    assert_eq!(
        selected_quality, quality,
        "exercise the intended resident branch"
    );
    assert_eq!(resource.source_texture_id(), texture);
    assert_eq!(resource.page_material_kind(), kind, "{quality}");
    let payload = app.fullscreen_paint_payload_for_resource(0, resource);
    assert_eq!(payload.source_texture_id(), texture);
    assert_eq!(
        payload.page_material_kind(),
        kind,
        "{quality} payload conversion"
    );
}

#[test]
fn resident_lower_stages_remain_provisional_until_final_is_available() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    assert!(app.edit_result_cache.is_empty());
    assert!(app.final_composite_cache.is_empty());
    let FsCacheEntry::Static { tex, pixels, .. } = app.fs_cache.get(&0).unwrap() else {
        panic!("ordinary source fixture");
    };
    let source = tex.clone();
    let pixels = pixels.clone();
    assert_resident_material(
        &app,
        "normal",
        source.id(),
        FullscreenPageMaterialKind::Provisional,
    );

    let edit = ctx.load_texture(
        "resident-placement-edit",
        pixels.clone(),
        egui::TextureOptions::LINEAR,
    );
    let edit_key = app.current_edit_result_key(0);
    app.edit_result_cache.insert(
        edit_key,
        crate::app::EditResultEntry::raster(pixels.clone(), Some(edit.clone())),
    );
    assert_resident_material(
        &app,
        "edit",
        edit.id(),
        FullscreenPageMaterialKind::Provisional,
    );

    // The exact cumulative edit stage is also a fallback for an absent final.
    // Its own edit work being complete cannot complete the final pipeline.
    app.edit_result_cache.clear();
    let erase = ctx.load_texture(
        "resident-placement-erase",
        pixels.clone(),
        egui::TextureOptions::LINEAR,
    );
    let erase_key = app.current_erase_result_key(0);
    app.erase_result_cache.insert(
        erase_key,
        crate::app::EraseResultCacheEntry {
            texture_options: egui::TextureOptions::LINEAR,
            display_basis: crate::material_display_basis::MaterialDisplayBasis::from_sampler(
                pixels.size,
                crate::source_coverage::SourceCoverage::FullSource,
            ),
            pixels,
            texture: erase.clone(),
        },
    );
    assert_resident_material(
        &app,
        "erase",
        erase.id(),
        FullscreenPageMaterialKind::Provisional,
    );

    app.erase_result_cache.clear();
    app.fs_cache.clear();
    let thumbnail = ctx.load_texture(
        "resident-placement-thumbnail",
        egui::ColorImage::filled([8, 12], egui::Color32::WHITE),
        egui::TextureOptions::LINEAR,
    );
    app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
    app.thumbnails[0] = ThumbnailState::loaded_raster(
        thumbnail.clone(),
        crate::thumb_loader::ThumbLoadOrigin::UpgradeableCache,
        false,
        12,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((80, 120)),
        None,
    );
    assert_resident_material(
        &app,
        "thumbnail",
        thumbnail.id(),
        FullscreenPageMaterialKind::Provisional,
    );
    assert!(
        app.final_composite_cache.is_empty(),
        "resident selection starts no producer"
    );
}

#[test]
fn resident_stable_raw_preview_preserves_complete_contract() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    let pixels = Arc::new(egui::ColorImage::filled([80, 120], egui::Color32::WHITE));
    let texture = ctx.load_texture(
        "resident-placement-stable-raw-preview",
        pixels.clone(),
        egui::TextureOptions::LINEAR,
    );
    app.fs_cache.insert(
        0,
        FsCacheEntry::RawPreview {
            preview: Some(crate::fs_animation::RawPreviewTexture {
                tex: texture.clone(),
                pixels,
            }),
            developed_dims: [800, 1200],
            load_seq: 1,
        },
    );
    assert_resident_material(
        &app,
        "preview",
        texture.id(),
        FullscreenPageMaterialKind::Complete,
    );
    assert!(app.final_composite_cache.is_empty());
}

#[test]
fn resident_comic_on_completed_final_preserves_complete_contract() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    let pixels = Arc::new(egui::ColorImage::filled([80, 120], egui::Color32::WHITE));
    let texture = ctx.load_texture(
        "resident-placement-final-base",
        pixels.clone(),
        egui::TextureOptions::LINEAR,
    );
    let entry = crate::app::FinalCompositeEntry::raster(
        Some(crate::page_dims::CanonicalPageDimensions::new(80, 120)),
        [80, 120],
        pixels.clone(),
        texture,
        egui::TextureOptions::LINEAR,
        true,
    );
    let final_texture = entry.texture.id();
    let basis = entry.display_basis;
    let final_key = app.final_composite_key_for_pixels(
        app.current_edit_result_key(0),
        [80, 120],
        app.effective_params(0),
    );
    app.final_composite_cache.insert(final_key, entry);
    assert_resident_material(
        &app,
        "final",
        final_texture,
        FullscreenPageMaterialKind::Complete,
    );
    // A completed base is still provisional while its required annotation
    // overlay is absent. This resident check must not load a document or fonts.
    app.comic_demo_enabled = true;
    assert_resident_material(
        &app,
        "final",
        final_texture,
        FullscreenPageMaterialKind::Provisional,
    );
    app.comic_demo_enabled = false;
    let comic = ctx.load_texture(
        "resident-placement-comic",
        pixels.clone(),
        egui::TextureOptions::LINEAR,
    );
    let comic_generation = app.comic_generation;
    app.comic_cache.insert(
        0,
        crate::app::ComicCacheEntry {
            display_basis: basis,
            pixels: pixels.clone(),
            texture: comic.clone(),
            texture_options: egui::TextureOptions::LINEAR,
            base: pixels,
            comic_gen: comic_generation,
            dims: [80, 120],
            preview_scale: 1,
        },
    );
    assert_resident_material(
        &app,
        "comic",
        comic.id(),
        FullscreenPageMaterialKind::Complete,
    );
}

#[test]
fn initial_placement_offered_incomplete_final_retains_until_completed_material_is_painted() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
    app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::ReadingStart;
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.settings.global_preset.upscale_model = Some(
        crate::ai::ModelKind::UpscaleRealEsrGeneralV3
            .as_str()
            .into(),
    );
    app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();

    // Keep both ordinary producers resident and already admitted. No inference,
    // model loading, or asynchronous source loading is needed by this fixture.
    for (idx, size) in [(0, [80, 120]), (1, [400, 600])] {
        let pixels = Arc::new(egui::ColorImage::filled(size, egui::Color32::WHITE));
        let texture = ctx.load_texture(
            format!("resident-placement-source-{idx}"),
            pixels.clone(),
            egui::TextureOptions::LINEAR,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                pdf_rerender_failures: Vec::new(),
                pdf_metadata: None,
                tex: texture.clone(),
                pixels: pixels.clone(),
                source_dims: Some(size),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.record_page_dims_for_spread(idx, (size[0] as u32, size[1] as u32));
        let edit_key = app.current_edit_result_key(idx);
        app.edit_result_cache.insert(
            edit_key,
            crate::app::EditResultEntry::raster(pixels.clone(), Some(texture.clone())),
        );
        let ai_key = app
            .final_ai_key_for_pixels(edit_key, size, app.effective_params(idx))
            .expect("enabled upscale key");
        let final_key =
            app.final_composite_key_for_pixels(edit_key, size, app.effective_params(idx));
        app.final_composite_cache.insert(
            final_key,
            crate::app::FinalCompositeEntry::raster(
                Some(crate::page_dims::CanonicalPageDimensions::new(
                    size[0] as u32,
                    size[1] as u32,
                )),
                size,
                pixels,
                texture,
                egui::TextureOptions::LINEAR,
                false,
            ),
        );
        let retained_epoch = app.retained_final_ai_epoch;
        app.final_ai_pending.insert(
            ai_key,
            crate::app::FinalAiPending {
                feedback_request: crate::app::FinalAiFeedbackRequest::default(),
                job_id: 1726 + idx as u64,
                cancel: Arc::new(AtomicBool::new(false)),
                retained_key: None,
                retained_epoch,
                allow_retained_orphan: false,
                cancel_on_drop: true,
            },
        );
    }
    frame(&ctx, |ui, ctx| {
        draw_current_with_initial_placement(&mut app, ui, ctx);
    });
    assert!(matches!(
        app.page_turn_presentation.state,
        PresentationState::Flat(_)
    ));
    assert_eq!(app.fs_pan, egui::Vec2::ZERO);

    // Use the released navigation handler, including its real offer and open.
    frame(&ctx, |_, ctx| {
        let operation = ViewerOperationId::Pad {
            batch: next_operation_batch(),
            ordinal: 0,
        };
        app.page_turn_external_operation(
            ctx,
            operation,
            OperationPhase::Press,
            "resident-placement-navigation",
        );
        app.handle_fs_navigation_with_operation(
            ctx,
            false,
            false,
            None,
            None,
            None,
            FsPageNav::Target(1),
            None,
            0,
            Some(operation),
        );
    });
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.fs_initial_placement_request.is_some());
    let accepted = {
        let PresentationState::Offered { offer, .. } = &app.page_turn_presentation.state else {
            panic!("resident final must be accepted: {:?}", take_test_events());
        };
        assert_eq!(offer.ready_sources.0.len(), 1);
        let source = &offer.ready_sources.0[0];
        assert_eq!(source.occurrence.idx, 1);
        assert_eq!(source.resource.size(), [400, 600]);
        assert_eq!(
            source.resource.page_material_kind(),
            FullscreenPageMaterialKind::Provisional,
            "the actual Offered payload must preserve complete=false"
        );
        source.resource.source_texture_id()
    };

    // A worker can finish between acceptance and paint. Publish a different
    // completed texture now, so source IDs prove that the first paint chooses
    // Offered's immutable resource over the ordinary resolver's newer result.
    let edit_key = app.current_edit_result_key(1);
    let ai_key = app
        .final_ai_key_for_pixels(edit_key, [400, 600], app.effective_params(1))
        .unwrap();
    let final_key =
        app.final_composite_key_for_pixels(edit_key, [400, 600], app.effective_params(1));
    let canonical = Some(crate::page_dims::CanonicalPageDimensions::new(400, 600));
    let pixels = Arc::new(egui::ColorImage::filled([1600, 2400], egui::Color32::WHITE));
    // The headless Context's 2048 cap is smaller than this real AI result.
    // Allocate a manager lease without requiring a physical GPU upload.
    let manager = ctx.tex_manager();
    let id = manager.write().alloc(
        "resident-placement-complete".into(),
        egui::ImageData::Color(pixels.clone()),
        egui::TextureOptions::LINEAR,
    );
    let texture = egui::TextureHandle::new(manager, id);
    let completed = texture.id();
    assert_ne!(accepted, completed);
    app.final_ai_pending.remove(&ai_key);
    app.final_ai_cache.insert(
        ai_key,
        crate::app::FinalAiEntry {
            diagnostic_metadata: None,
            canonical_dims: canonical,
            source_size: [400, 600],
            pixels: pixels.clone(),
            used_upscale: true,
        },
    );
    app.final_composite_cache.insert(
        final_key,
        crate::app::FinalCompositeEntry::raster(
            canonical,
            [400, 600],
            pixels,
            texture,
            egui::TextureOptions::LINEAR,
            true,
        ),
    );
    frame(&ctx, |ui, ctx| {
        let ordinary = app.resolve_fs_processed_material(ctx, 1, false).unwrap();
        assert_eq!(ordinary.source_texture_id(), completed);
        assert_eq!(
            ordinary.page_material_kind(),
            FullscreenPageMaterialKind::Complete
        );
        let selected = app
            .fullscreen_paint_payload_for_page(1, Some(ordinary), false)
            .unwrap();
        assert_eq!(selected.source_texture_id(), accepted);
        assert_eq!(
            selected.page_material_kind(),
            FullscreenPageMaterialKind::Provisional
        );
        let sources = draw_current_with_initial_placement(&mut app, ui, ctx);
        assert_eq!(sources.pages.len(), 1);
        assert_eq!(sources.pages[0].resource.source_texture_id(), accepted);
        assert_eq!(
            sources.pages[0].resource.page_material_kind(),
            FullscreenPageMaterialKind::Provisional
        );
        let transform = app.fullscreen_page_layout.page_by_idx(1).unwrap().transform;
        close_to(transform.paint_rect.height(), 600.0);
        close_to(transform.paint_rect.top(), 0.0);
        assert!(
            app.fs_initial_placement_request.is_some(),
            "painting Offered's incomplete final must retain the request"
        );
        assert_ne!(app.fs_pan, egui::Vec2::ZERO);
        assert!(
            app.page_turn_held_resource(1).is_none(),
            "reading-start pan retires the ineligible offer through ordinary submission"
        );
    });
    frame(&ctx, |ui, ctx| {
        let sources = draw_current_with_initial_placement(&mut app, ui, ctx);
        assert_eq!(sources.pages.len(), 1);
        assert_eq!(sources.pages[0].resource.source_texture_id(), completed);
        assert_eq!(
            sources.pages[0].resource.page_material_kind(),
            FullscreenPageMaterialKind::Complete
        );
        let transform = app.fullscreen_page_layout.page_by_idx(1).unwrap().transform;
        close_to(transform.paint_rect.height(), 2400.0);
        close_to(transform.paint_rect.top(), 0.0);
        assert!(
            app.fs_initial_placement_request.is_none(),
            "the first actual Complete frame commits placement"
        );
    });
}
