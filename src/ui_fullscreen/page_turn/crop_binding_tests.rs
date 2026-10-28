//! Document crop binding stays independent of accepted raster quality.
use super::*;
use crate::export_crop::{CropAspectMode, CropRect, CropSettings};
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, frame, ready_app};

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

#[test]
fn crop_binding_is_memory_only_and_quality_independent_but_reference_size_is_content() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    let saved = crop();
    app.export_crop_page_settings.insert(0, saved);
    let binding = app.page_turn_content(0);
    for size in [[8, 12], [80, 120], [800, 1200]] {
        assert_eq!(app.prepared_single_page_crop(0, size, false), Some(saved));
        assert_eq!(app.resident_export_crop_for_idx(0), Some(saved));
        app.input_generation.insert(0, size[0] as u64);
        assert!(app.page_turn_content(0) == binding);
    }
    let mut changed = saved;
    changed.source_size = Some([160, 240]);
    app.export_crop_page_settings.insert(0, changed);
    assert!(app.page_turn_content(0) != binding);
    changed = saved;
    changed.rect.min_x += 1.0;
    app.export_crop_page_settings.insert(0, changed);
    assert!(app.page_turn_content(0) != binding);
    changed.source_size = None;
    app.export_crop_page_settings.insert(0, changed);
    assert_eq!(
        app.resident_export_crop_for_idx(0),
        Some(changed.with_legacy_source_size([80, 120]))
    );
    assert_eq!(app.export_crop_page_settings[&0].source_size, None);
}

#[test]
fn crop_legacy_normal_adoption_preserves_the_pure_acceptance_binding() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    let mut legacy = crop();
    legacy.source_size = None;
    app.export_crop_page_settings.insert(0, legacy);
    let before = app.page_turn_content(0);
    assert_eq!(app.export_crop_page_settings[&0].source_size, None);
    app.ensure_export_crop_source_size(0, [80, 120]).unwrap();
    assert_eq!(
        app.export_crop_page_settings[&0].source_size,
        Some([80, 120])
    );
    assert!(before == app.page_turn_content(0));

    // A too-small raster stays provisional and clamps in the frame's source
    // coordinates, rather than the fit/AI sampler dimensions.
    legacy.rect.max_x = 900.0;
    app.export_crop_page_settings.insert(0, legacy);
    let paint = app.prepared_single_page_crop(0, [100, 150], false).unwrap();
    assert_eq!(paint.source_size, Some([100, 150]));
    assert_eq!(paint.rect.max_x, 100.0);
    assert_eq!(app.export_crop_page_settings[&0], legacy);
}

#[cfg(windows)]
#[test]
fn crop_live_snapshot_preserves_original_preview_suppression() {
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    app.export_crop_page_settings.insert(0, crop());
    frame(&ctx, |ui, ctx| {
        draw_current(&mut app, ui, ctx);
        let viewport = if app.fullscreen_embedded_still_active() {
            ctx.viewport_id()
        } else {
            app.fullscreen_viewport_id()
        };
        let sample = FsOriginalPreviewFrameSample {
            frame_nr: ctx.cumulative_frame_nr(),
            items_generation: app.items_generation,
            idx: 0,
            active: true,
            written_pass_nr: ctx.cumulative_pass_nr(),
            written_call_order: 1,
            sequence_blocks_new_target_at_write: false,
        };
        ctx.data_mut(|data| {
            data.insert_temp(
                egui::Id::unique(("fs_original_preview_active", viewport)),
                sample,
            )
        });
        let capture = app
            .capture_paged_display_unit_for_snapshot(Some(ctx), 0)
            .unwrap();
        assert!(capture.unit.pages.iter().all(|page| page.crop.is_none()));
    });
}

#[test]
fn crop_prepared_display_conditions_preserve_tool_and_original_preview_policy() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Ltr);
    assert!(app.prepared_single_page_crop(0, [80, 120], false).is_none());
    app.export_crop_mode = true;
    let full = app.prepared_single_page_crop(0, [80, 120], false).unwrap();
    assert!(full.is_full(80, 120));
    assert!(app.prepared_single_page_crop(0, [80, 120], true).is_none());
    app.export_crop_mode = false;
    app.export_crop_page_settings.insert(0, crop());
    app.text_mode = true;
    assert!(app.prepared_single_page_crop(0, [80, 120], false).is_some());
    app.text_mode = false;
    for tool in 0..4 {
        app.erase_mode = tool == 0;
        app.local_adjust_mode = tool == 1;
        app.conceal_mode = tool == 2;
        app.analysis_mode = tool == 3;
        assert!(app.prepared_single_page_crop(0, [80, 120], false).is_none());
    }
    app.analysis_mode = false;
    app.adjustment_mode = crate::ui_helpers::MetadataPanelOpenState::ByPointer;
    assert!(app.prepared_single_page_crop(0, [80, 120], false).is_none());
    app.adjustment_mode = crate::ui_helpers::MetadataPanelOpenState::Closed;
    app.sns_split = Some(crate::sns_split::SnsSplitLayout::centered_max(
        crate::sns_split::SnsTarget::X,
        2,
        [80, 120],
    ));
    assert!(app.prepared_single_page_crop(0, [80, 120], false).is_none());
}

#[test]
fn crop_actual_pair_is_suppressed_and_singleton_spread_keeps_crop() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut app = ready_app(&ctx, mode);
        app.settings.page_turn_animation_enabled = true;
        app.export_crop_page_settings.insert(0, crop());
        frame(&ctx, |ui, ctx| {
            draw_current(&mut app, ui, ctx);
        });
        let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
            panic!()
        };
        assert!(
            unit.pages
                .iter()
                .all(|page| page.crop.is_some() == !mode.is_spread())
        );
    }
    // A spread setting can still present exactly one real page. Keep its crop
    // through actual prepare/submit rather than treating the setting as a pair.
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut app = ready_app(&ctx, mode);
        app.settings.page_turn_animation_enabled = true;
        app.items.truncate(1);
        app.visible_indices = vec![0];
        app.export_crop_page_settings.insert(0, crop());
        frame(&ctx, |ui, ctx| {
            let (composition, _) = draw_current(&mut app, ui, ctx);
            assert_eq!(composition.pages_in_screen_order(mode).len(), 1);
        });
        let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
            panic!("singleton must submit its normal prepared page")
        };
        assert_eq!(unit.pages.len(), 1);
        assert_eq!(unit.pages[0].crop, Some(crop()));
        assert_eq!(
            canonical_unit(unit)
                .unwrap()
                .pages
                .iter()
                .filter(|page| page.crop.is_some())
                .count(),
            1
        );
    }
}

#[test]
fn crop_prepared_mapping_preserves_source_coordinates_through_rotation_trim_and_zoom() {
    use crate::rotation_db::Rotation;
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    app.export_crop_page_settings.insert(0, crop());
    frame(&ctx, |ui, ctx| {
        draw_current(&mut app, ui, ctx);
    });
    let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
        panic!()
    };
    let original = unit.pages[0].clone();
    let expected = [
        egui::pos2(10.0 / 80.0, 20.0 / 120.0),
        egui::pos2(70.0 / 80.0, 20.0 / 120.0),
        egui::pos2(70.0 / 80.0, 100.0 / 120.0),
        egui::pos2(10.0 / 80.0, 100.0 / 120.0),
    ];
    for rotation in [
        Rotation::None,
        Rotation::Cw90,
        Rotation::Cw180,
        Rotation::Cw270,
    ] {
        for content_bbox in [
            None,
            Some(egui::Rect::from_min_max(
                egui::pos2(0.2, 0.1),
                egui::pos2(0.9, 0.85),
            )),
        ] {
            for zoom_pan in [None, Some((1.5, egui::vec2(32.0, -17.0)))] {
                let mut page = original.clone();
                page.transform = DisplayedImageTransform::resolve(DisplayedImageTransformInput {
                    page_idx: 0,
                    viewport_rect: original.transform.viewport_rect,
                    // Raster quality differs from crop's authored 80x120 basis.
                    source_size: egui::vec2(800.0, 1200.0),
                    texture_size: egui::vec2(800.0, 1200.0),
                    rotation,
                    free_rotation_rad: 0.0,
                    content_bbox,
                    fit_mode: FullscreenFitInput::legacy(FullscreenFitMode::Page),
                    fit_scale_limits: FullscreenFitScaleLimits::default(),
                    pixels_per_point: 1.0,
                    placement: ResolvedDisplayPlacement::Normal { zoom_pan },
                    pixel_fit: RectPixelFit::Proportional,
                })
                .unwrap();
                let paint = prepared_page_crop(&page).unwrap();
                for (actual, expected) in paint.corners.into_iter().zip(expected) {
                    let screen = page.transform.paint_rect.min
                        + actual.to_vec2() * page.transform.paint_rect.size();
                    let source = page.transform.screen_to_source_normalized(screen);
                    assert!(
                        (source - expected).length() < 0.00001,
                        "{rotation:?} trim={content_bbox:?} zoom={zoom_pan:?}: source={source:?} expected={expected:?}"
                    );
                }
                // Prepared crop follows the same coordinates into canonical paint.
                let mut canonical_source = (**unit).clone();
                canonical_source.pages = vec![page.clone()];
                assert_eq!(
                    canonical_unit(&canonical_source).unwrap().pages[0].crop,
                    Some(paint)
                );
                assert_eq!(
                    page.crop,
                    Some(crop()),
                    "mapping never rewrites its immutable binding"
                );
            }
        }
    }
}

#[test]
fn crop_held_clone_retains_quality_but_rejects_a_different_prepared_crop() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.export_crop_page_settings.insert(0, crop());
    app.export_crop_page_settings.insert(1, crop());
    crate::ui_fullscreen::page_turn_submission_tests::seed_leaf(&ctx, &mut app);
    let PresentationState::Turning { incoming, .. } = &app.page_turn_presentation.state else {
        panic!()
    };
    let held = incoming.pages[0].clone();
    assert_eq!(held.crop, Some(crop()));
    let replacement = ctx.load_texture(
        "crop-quality-publication",
        egui::ColorImage::filled([160, 240], egui::Color32::LIGHT_BLUE),
        crate::app::DISPLAY_IMAGE_TEXTURE_OPTIONS,
    );
    let mut fresh = held.clone();
    fresh.transform = DisplayedImageTransform::from_resolved_rect(
        DisplayedImageTransformInput {
            page_idx: held.occurrence.idx,
            viewport_rect: held.transform.viewport_rect,
            source_size: egui::vec2(160.0, 240.0),
            texture_size: egui::vec2(160.0, 240.0),
            rotation: held.transform.rotation,
            free_rotation_rad: held.transform.free_rotation_rad,
            content_bbox: held.transform.content_bbox,
            fit_mode: held.transform.fit_input,
            fit_scale_limits: held.transform.fit_scale_limits,
            pixels_per_point: held.transform.pixels_per_point,
            placement: held.transform.placement,
            pixel_fit: RectPixelFit::Proportional,
        },
        held.transform.paint_rect,
    )
    .unwrap();
    fresh.resource = FullscreenPaintResource::resampleable(
        held.occurrence.idx,
        replacement,
        crate::gpu_lanczos::FullscreenPaintSourceGeneration {
            items: app.items_generation,
            input: 99,
        },
    )
    .into();
    let fresh_id = fresh.resource.paint_texture_id();
    assert_ne!(fresh_id, held.resource.paint_texture_id());
    let bound = app.page_turn_bind_prepared(fresh.clone());
    assert_eq!(
        bound.resource.paint_texture_id(),
        held.resource.paint_texture_id()
    );
    assert_eq!(bound.transform.source_size, held.transform.source_size);
    assert_eq!(bound.crop, held.crop);

    let mut different = crop();
    different.rect.min_x += 1.0;
    fresh.crop = Some(different);
    let bound = app.page_turn_bind_prepared(fresh);
    assert_eq!(
        bound.resource.paint_texture_id(),
        fresh_id,
        "clone must not replace a newly prepared crop with held content"
    );
    assert_eq!(bound.crop, Some(different));
    app.export_crop_page_settings
        .insert(held.occurrence.idx, different);
    assert!(
        app.page_turn_held_transform(held.occurrence.idx).is_none(),
        "real document crop mutation invalidates held content"
    );
}

#[test]
fn crop_captured_singleton_retains_capture_time_crop_after_item_index_reuse() {
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    app.export_crop_page_settings.insert(0, crop());
    frame(&ctx, |ui, ctx| {
        draw_current(&mut app, ui, ctx);
    });
    let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
        panic!()
    };
    let mut expected_page = unit.pages[0].clone();
    let captured = app
        .capture_fs_display_unit(0)
        .expect("resident singleton capture");
    assert_eq!(captured.pages.len(), 1);
    assert_eq!(captured.pages[0].crop, Some(crop()));
    let mut replacement = crop();
    replacement.rect.min_x = 35.0;
    app.items[0] = GridItem::Image(PathBuf::from("c:/page-turn/replacement.png"));
    app.items_generation += 1;
    app.export_crop_page_settings.insert(0, replacement);
    let output = frame(&ctx, |ui, ctx| {
        app.draw_fs_display_unit_holdover_with_geometry(
            ui,
            ctx,
            ui.max_rect(),
            &captured,
            StillSeekGeometry::bar_only(),
        );
    });
    expected_page.transform = app.fullscreen_page_layout.single_page().unwrap().transform;
    let expected = crate::page_turn_animation::project_flat_crop(
        expected_page.transform.paint_rect,
        expected_page
            .painter
            .clip_rect()
            .intersect(expected_page.transform.viewport_rect),
        &expected_page.transform.paint_quad().positions,
        prepared_page_crop(&expected_page).unwrap(),
    );
    let dim_vertices = |meshes: Vec<egui::Mesh>| {
        meshes
            .into_iter()
            .flat_map(|mesh| mesh.vertices)
            .filter(|vertex| vertex.color == egui::Color32::from_black_alpha(145))
            .map(|vertex| vertex.pos)
            .collect::<Vec<_>>()
    };
    let actual = dim_vertices(
        output
            .shapes
            .into_iter()
            .filter_map(|shape| match shape.shape {
                egui::Shape::Mesh(mesh) => Some((*mesh).clone()),
                _ => None,
            })
            .collect(),
    );
    let expected = dim_vertices(expected.into_iter().map(|draw| draw.mesh).collect());
    assert!(!actual.is_empty(), "captured singleton must paint its crop");
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(&expected) {
        assert!(
            (*actual - *expected).length() < 0.0001,
            "captured crop must not resolve the replacement index"
        );
    }
    assert_eq!(app.resident_export_crop_for_idx(0), Some(replacement));
    app.export_crop_page_settings.remove(&0);
    let output = frame(&ctx, |ui, ctx| {
        app.draw_fs_display_unit_holdover_with_geometry(
            ui,
            ctx,
            ui.max_rect(),
            &captured,
            StillSeekGeometry::bar_only(),
        );
    });
    let after_removal = dim_vertices(
        output
            .shapes
            .into_iter()
            .filter_map(|shape| match shape.shape {
                egui::Shape::Mesh(mesh) => Some((*mesh).clone()),
                _ => None,
            })
            .collect(),
    );
    assert_eq!(
        after_removal, actual,
        "removing replacement crop must preserve the captured paint"
    );
    assert_eq!(captured.pages[0].crop, Some(crop()));
}

#[test]
fn crop_acceptance_uses_exact_edit_stage_before_source_without_starting_producers() {
    for stage in ["erase", "local_adjust", "conceal", "edit"] {
        let ctx = egui::Context::default();
        let mut app = ready_app(&ctx, SpreadMode::Single);
        let pixels = Arc::new(egui::ColorImage::filled([80, 120], egui::Color32::GOLD));
        let texture = ctx.load_texture(
            stage,
            pixels.clone(),
            crate::app::DISPLAY_IMAGE_TEXTURE_OPTIONS,
        );
        let expected = texture.id();
        match stage {
            "erase" => {
                let key = app.current_erase_result_key(1);
                app.erase_result_cache.insert(
                    key,
                    crate::app::EraseResultCacheEntry::raster(pixels, texture),
                );
            }
            "local_adjust" => {
                let key = app.current_local_adjust_key(1);
                app.local_adjust_cache.insert(
                    key,
                    crate::app::LocalAdjustCacheEntry::raster(pixels, texture),
                );
            }
            "conceal" => {
                let generation = app.conceal_generation;
                app.conceal_cache.insert(
                    1,
                    crate::app::ConcealCacheEntry::raster(pixels, texture, generation),
                );
            }
            "edit" => {
                let key = app.current_edit_result_key(1);
                app.edit_result_cache.insert(
                    key,
                    crate::app::EditResultEntry::raster(pixels, Some(texture)),
                );
            }
            _ => unreachable!(),
        }
        ctx.tex_manager().write().take_delta().clear();
        let (selected, quality) = app.page_turn_resident_source(1).unwrap();
        assert_eq!(quality, stage);
        assert_eq!(selected.source_texture_id(), expected);
        assert!(app.local_adjust_pending.is_empty());
        assert!(app.erase_inpaint_pending.is_empty());
        assert!(app.final_effect_pending.is_empty());
        assert!(ctx.tex_manager().write().take_delta().is_empty());
    }
}

#[test]
fn crop_acceptance_rejects_stale_edit_while_normal_display_keeps_its_holdover() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    let pixels = Arc::new(egui::ColorImage::filled([80, 120], egui::Color32::GOLD));
    let texture = ctx.load_texture(
        "stale-edit",
        pixels.clone(),
        crate::app::DISPLAY_IMAGE_TEXTURE_OPTIONS,
    );
    let stale = texture.id();
    let key = app.current_edit_result_key(1);
    app.edit_result_cache.insert(
        key,
        crate::app::EditResultEntry::raster(pixels, Some(texture)),
    );
    app.erase_mask_generation.insert(1, key.erase_mask_gen + 1);
    assert_eq!(
        app.resolve_fs_display_material_from_caches(1, false)
            .unwrap()
            .source_texture_id(),
        stale
    );
    let (accepted, quality) = app.page_turn_resident_source(1).unwrap();
    assert_eq!(quality, "normal");
    assert_ne!(accepted.source_texture_id(), stale);
}
