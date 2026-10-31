use super::*;
use egui_kittest::kittest::Queryable;

#[test]
fn section1673_conceal_source_control_handler_undo_and_database_roundtrip() {
    let mut env = crate::app::setup_app_for_test();
    let page = env.tmp.path().join("bucket-conceal.png");
    let app = &mut env.app;
    app.items = vec![crate::grid_item::GridItem::Image(page)];
    app.fullscreen_idx = Some(0);
    app.conceal_mask_size = [9, 9];
    app.conceal_mask = Some(
        (0..81)
            .map(|i| {
                let (x, y) = (i % 9, i / 9);
                (2..=6).contains(&x)
                    && (2..=6).contains(&y)
                    && (x == 2 || x == 6 || y == 2 || y == 6)
            })
            .collect(),
    );
    app.conceal_paint_mode = true;
    app.conceal_bucket_region = crate::mask_db::BucketRegion::Connected;
    app.conceal_bucket_leak_stop = 0.0;
    app.conceal_bucket_outset = 0.0;
    let ctx = egui::Context::default();
    let pixels = egui::ColorImage::filled([9, 9], egui::Color32::WHITE);
    app.fs_cache.insert(
        0,
        crate::fs_animation::FsCacheEntry::Static {
            pdf_metadata: None,
            pdf_rerender_failures: Vec::new(),
            tex: ctx.load_texture("bucket-conceal", pixels.clone(), Default::default()),
            pixels: std::sync::Arc::new(pixels),
            source_dims: Some([9, 9]),
            load_seq: 0,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    assert_eq!(
        app.conceal_bucket_source,
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
                        &mut app.conceal_bucket_source,
                        &mut app.conceal_bucket_tolerance,
                    );
                });
            });
        harness.get_by_label("描いたマスク").click();
        harness.run();
    }
    assert_eq!(
        app.conceal_bucket_source,
        crate::mask_db::BucketSource::DrawnMask
    );
    let before = app.conceal_mask.clone().unwrap();
    app.apply_conceal_bucket(&ctx, 0, 4, 4);
    let filled = app.conceal_mask.clone().unwrap();
    for y in 0..9 {
        for x in 0..9 {
            assert_eq!(
                filled[y * 9 + x],
                (2..=6).contains(&x) && (2..=6).contains(&y)
            );
        }
    }
    assert_eq!(app.conceal_undo_stack.len(), 1);
    app.apply_conceal_bucket(&ctx, 0, 4, 4);
    assert_eq!(app.conceal_undo_stack.len(), 1);
    assert_eq!(app.conceal_mask.as_ref(), Some(&filled));
    assert!(app.undo_conceal());
    assert_eq!(app.conceal_mask.as_ref(), Some(&before));
    assert!(!app.undo_conceal());
    assert!(app.redo_conceal());
    let shapes = app.conceal_shapes.clone();
    app.save_conceal_with_sidecar(0, &filled, &shapes, 9, 9);
    let key = app.page_path_key(0).unwrap();
    let reopened =
        crate::conceal_db::ConcealDb::open_at(&crate::conceal_db::ConcealDb::db_path()).unwrap();
    assert_eq!(reopened.get_full(&key, 9, 9), Some((filled, shapes)));
}
