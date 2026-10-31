use super::*;
use egui_kittest::kittest::Queryable;
use local_adjust_core::{
    LocalAdjustmentLayer, LocalEffect, LocalMask, MaskShape, RasterVectorMask,
};

fn target_ring() -> RasterVectorMask {
    let mut mask = RasterVectorMask::empty(9, 9);
    for (i, alpha) in mask.alpha_mut().iter_mut().enumerate() {
        let (x, y) = (i % 9, i / 9);
        *alpha = if (2..=6).contains(&x)
            && (2..=6).contains(&y)
            && (x == 2 || x == 6 || y == 2 || y == 6)
        {
            0.5
        } else {
            0.49
        };
    }
    // The top wall has a bitmap gap, closed by shapes in creation order.
    // Grouping all additions before subtractions would reopen it and leak.
    mask.alpha_mut()[2 * 9 + 4] = 0.0;
    mask.shapes = [
        local_adjust_core::ShapeOp::Add,
        local_adjust_core::ShapeOp::Subtract,
        local_adjust_core::ShapeOp::Add,
    ]
    .into_iter()
    .map(|op| MaskShape::Rect {
        op,
        center: [4.5, 2.5],
        half_w: 0.5,
        half_h: 0.5,
        rotation_rad: 0.0,
    })
    .collect();
    mask
}

fn filled_mask() -> RasterVectorMask {
    let mut mask = RasterVectorMask::empty(9, 9);
    mask.alpha_mut().fill(1.0);
    mask
}

fn layer_for_target(target: LocalAdjustMaskEditTarget, inverted: bool) -> LocalAdjustmentLayer {
    let mut layer = LocalAdjustmentLayer::new(
        "bucket target",
        LocalMask::RasterVector(filled_mask()),
        LocalEffect::None,
    );
    layer.mask_inverted = inverted;
    layer.manual_override.add = Some(filled_mask());
    layer.manual_override.subtract = Some(filled_mask());
    match target {
        LocalAdjustMaskEditTarget::Base => layer.mask = LocalMask::RasterVector(target_ring()),
        LocalAdjustMaskEditTarget::OverrideAdd => layer.manual_override.add = Some(target_ring()),
        LocalAdjustMaskEditTarget::OverrideSubtract => {
            layer.manual_override.subtract = Some(target_ring());
        }
        LocalAdjustMaskEditTarget::None => unreachable!(),
    }
    layer
}

fn check_target(target: LocalAdjustMaskEditTarget, inverted: bool) {
    let mut env = crate::app::setup_app_for_test();
    let page = env.tmp.path().join("bucket-local.png");
    let app = &mut env.app;
    app.items = vec![crate::grid_item::GridItem::Image(page)];
    app.fullscreen_idx = Some(0);
    app.local_adjust_mask_paint_add = true;
    app.local_adjust_bucket_region = crate::mask_db::BucketRegion::Connected;
    app.local_adjust_bucket_leak_stop = 0.0;
    app.local_adjust_bucket_outset = 0.0;
    let before = layer_for_target(target, inverted);
    app.local_adjust_page_layers
        .insert(0, Arc::new(vec![before.clone()]));
    let ctx = egui::Context::default();
    let pixels = egui::ColorImage::filled([9, 9], egui::Color32::WHITE);
    app.fs_cache.insert(
        0,
        crate::fs_animation::FsCacheEntry::Static {
            pdf_metadata: None,
            pdf_rerender_failures: Vec::new(),
            tex: ctx.load_texture("bucket-local", pixels.clone(), Default::default()),
            pixels: Arc::new(pixels),
            source_dims: Some([9, 9]),
            load_seq: 0,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    assert_eq!(
        app.local_adjust_bucket_source,
        crate::mask_db::BucketSource::ImageColor
    );
    {
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(480.0, 180.0))
            .build_ui(|root| {
                crate::test_ui::full_viewport_ui(root, |ui| {
                    crate::ui_helpers::draw_bucket_source_controls(
                        ui,
                        &mut app.local_adjust_bucket_source,
                        &mut app.local_adjust_edge_brush_tolerance,
                    );
                });
            });
        harness.get_by_label("描いたマスク").click();
        harness.run();
    }
    assert_eq!(
        app.local_adjust_bucket_source,
        crate::mask_db::BucketSource::DrawnMask
    );
    let undo_before = app.meta_undo.undo_len();
    app.apply_local_adjust_bucket(&ctx, 0, 0, target, [0.5, 0.5]);
    let after = app.local_adjust_page_layers[&0][0].clone();
    let edited = local_adjust_target_raster_vector_mask_ref(&after, target).unwrap();
    for y in 0..9 {
        for x in 0..9 {
            let expected = (2..=6).contains(&x) && (2..=6).contains(&y) && (x, y) != (4, 2);
            assert_eq!(edited.alpha[y * 9 + x], if expected { 1.0 } else { 0.0 });
        }
    }
    assert_eq!(edited.shapes, target_ring().shapes);
    let mut composite = edited.alpha.to_vec();
    local_adjust_core::rasterize_shapes_into(
        &mut composite,
        edited.width,
        edited.height,
        &edited.shapes,
    );
    for y in 0..9 {
        for x in 0..9 {
            assert_eq!(
                composite[y * 9 + x] >= 0.5,
                (2..=6).contains(&x) && (2..=6).contains(&y),
            );
        }
    }
    assert_eq!(after.mask_inverted, inverted);
    // All other masks are deliberately solid: combining them into the walls
    // would reject the seed instead of filling the chosen target.
    let mut expected_after = before.clone();
    match target {
        LocalAdjustMaskEditTarget::Base => expected_after.mask = after.mask.clone(),
        LocalAdjustMaskEditTarget::OverrideAdd => {
            expected_after.manual_override.add = after.manual_override.add.clone();
        }
        LocalAdjustMaskEditTarget::OverrideSubtract => {
            expected_after.manual_override.subtract = after.manual_override.subtract.clone();
        }
        LocalAdjustMaskEditTarget::None => unreachable!(),
    }
    assert_eq!(after, expected_after);
    assert_eq!(app.meta_undo.undo_len(), undo_before + 1);
    app.apply_local_adjust_bucket(&ctx, 0, 0, target, [0.5, 0.5]);
    assert_eq!(app.meta_undo.undo_len(), undo_before + 1);
    assert_eq!(app.local_adjust_page_layers[&0][0], after);
    app.finish_local_adjust_writes();
    let key = app.page_path_key(0).unwrap();
    let reopened = crate::local_adjust_db::LocalAdjustDb::open_at(
        &crate::local_adjust_db::LocalAdjustDb::db_path(),
    )
    .unwrap();
    assert_eq!(reopened.get_layers(&key), Some(vec![after]));
    app.apply_meta_undo();
    assert_eq!(app.local_adjust_page_layers[&0][0], before);
    assert_eq!(app.meta_undo.undo_len(), undo_before);
    app.finish_local_adjust_writes();
    // Persistent alpha uses the existing q8 codec; Undo restores exact RAM
    // values, whereas reopening restores their serialized representation.
    let json = serde_json::to_string(&vec![before.clone()]).unwrap();
    let expected_saved = crate::local_adjust_db::parse_layers_json(&json).unwrap();
    let reloaded = reopened.get_layers(&key).unwrap();
    assert_eq!(reloaded, expected_saved);
    let original_mask = local_adjust_target_raster_vector_mask_ref(&before, target).unwrap();
    let restored_mask = local_adjust_target_raster_vector_mask_ref(&reloaded[0], target).unwrap();
    assert_eq!(restored_mask.shapes, original_mask.shapes);
    assert_eq!(restored_mask.width, original_mask.width);
    assert_eq!(restored_mask.height, original_mask.height);
    for (restored, original) in restored_mask.alpha.iter().zip(original_mask.alpha.iter()) {
        assert_eq!(*restored >= 0.5, *original >= 0.5);
    }
    let mut only_target_quantized = before.clone();
    match target {
        LocalAdjustMaskEditTarget::Base => {
            only_target_quantized.mask = LocalMask::RasterVector(restored_mask.clone());
        }
        LocalAdjustMaskEditTarget::OverrideAdd => {
            only_target_quantized.manual_override.add = Some(restored_mask.clone());
        }
        LocalAdjustMaskEditTarget::OverrideSubtract => {
            only_target_quantized.manual_override.subtract = Some(restored_mask.clone());
        }
        LocalAdjustMaskEditTarget::None => unreachable!(),
    }
    assert_eq!(reloaded[0], only_target_quantized);
}

#[test]
fn section1673_local_base_source_control_handler_undo_and_database_roundtrip() {
    for inverted in [false, true] {
        check_target(LocalAdjustMaskEditTarget::Base, inverted);
    }
}

#[test]
fn section1673_local_override_add_uses_only_target_mask_even_when_inverted() {
    for inverted in [false, true] {
        check_target(LocalAdjustMaskEditTarget::OverrideAdd, inverted);
    }
}

#[test]
fn section1673_local_override_subtract_uses_only_target_mask_even_when_inverted() {
    for inverted in [false, true] {
        check_target(LocalAdjustMaskEditTarget::OverrideSubtract, inverted);
    }
}

#[test]
fn section1673_erasing_absent_override_reports_matching_seed_without_creating_mask() {
    for target in [
        LocalAdjustMaskEditTarget::OverrideAdd,
        LocalAdjustMaskEditTarget::OverrideSubtract,
    ] {
        let mut env = crate::app::setup_app_for_test();
        let page = env.tmp.path().join("empty-override.png");
        let app = &mut env.app;
        app.items = vec![crate::grid_item::GridItem::Image(page)];
        app.fullscreen_idx = Some(0);
        app.local_adjust_mask_paint_add = false;
        app.local_adjust_bucket_source = crate::mask_db::BucketSource::DrawnMask;
        let layer = LocalAdjustmentLayer::new("empty override", LocalMask::Full, LocalEffect::None);
        app.local_adjust_page_layers
            .insert(0, Arc::new(vec![layer.clone()]));
        let ctx = egui::Context::default();
        let pixels = egui::ColorImage::filled([9, 9], egui::Color32::WHITE);
        app.fs_cache.insert(
            0,
            crate::fs_animation::FsCacheEntry::Static {
                pdf_metadata: None,
                pdf_rerender_failures: Vec::new(),
                tex: ctx.load_texture("empty-override", pixels.clone(), Default::default()),
                pixels: Arc::new(pixels),
                source_dims: Some([9, 9]),
                load_seq: 0,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        let undo_before = app.meta_undo.undo_len();
        app.apply_local_adjust_bucket(&ctx, 0, 0, target, [0.5, 0.5]);
        assert_eq!(app.local_adjust_page_layers[&0][0], layer);
        assert_eq!(app.meta_undo.undo_len(), undo_before);
        assert!(app.feedback_toasts_for_test.iter().any(|(text, kind)| text
            == crate::ui_helpers::bucket_seed_already_value_message()
            && *kind == crate::feedback::FeedbackKind::Warning));
        app.finish_local_adjust_writes();
        let key = app.page_path_key(0).unwrap();
        let db = crate::local_adjust_db::LocalAdjustDb::open_at(
            &crate::local_adjust_db::LocalAdjustDb::db_path(),
        )
        .unwrap();
        assert_eq!(db.get_layers(&key), None, "no-op must not publish a save");
    }
}
