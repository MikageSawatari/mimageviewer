use super::*;
use egui_kittest::kittest::Queryable;

fn ring() -> Vec<bool> {
    (0..81)
        .map(|i| {
            let (x, y) = (i % 9, i / 9);
            (2..=6).contains(&x) && (2..=6).contains(&y) && (x == 2 || x == 6 || y == 2 || y == 6)
        })
        .collect()
}

#[test]
fn section1673_erase_source_control_handler_undo_and_database_roundtrip() {
    let mut env = crate::app::setup_app_for_test();
    let page = env.tmp.path().join("bucket-erase.png");
    let app = &mut env.app;
    app.items = vec![crate::grid_item::GridItem::Image(page)];
    app.fullscreen_idx = Some(0);
    app.erase_mask_size = [9, 9];
    app.erase_mask = Some(ring());
    app.erase_paint_mode = true;
    app.erase_bucket_region = crate::mask_db::BucketRegion::Connected;
    app.erase_bucket_leak_stop = 0.0;
    app.erase_bucket_outset = 0.0;
    app.erase_base_cache.insert(
        0,
        crate::app::EraseBaseSource::raster(Arc::new(egui::ColorImage::filled(
            [9, 9],
            egui::Color32::WHITE,
        ))),
    );
    assert_eq!(
        app.erase_bucket_source,
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
                        &mut app.erase_bucket_source,
                        &mut app.erase_bucket_tolerance,
                    );
                });
            });
        harness.get_by_label("描いたマスク").click();
        harness.run();
    }
    assert_eq!(
        app.erase_bucket_source,
        crate::mask_db::BucketSource::DrawnMask
    );
    let before = app.erase_mask.clone().unwrap();
    let ctx = egui::Context::default();
    app.apply_erase_bucket(&ctx, 0, 4, 4);
    let filled = app.erase_mask.clone().unwrap();
    for y in 0..9 {
        for x in 0..9 {
            assert_eq!(
                filled[y * 9 + x],
                (2..=6).contains(&x) && (2..=6).contains(&y)
            );
        }
    }
    assert_eq!(app.erase_undo_stack.len(), 1);
    // A second click inside the filled region must neither write nor add history.
    app.apply_erase_bucket(&ctx, 0, 4, 4);
    assert_eq!(app.erase_undo_stack.len(), 1);
    assert_eq!(app.erase_mask.as_ref(), Some(&filled));
    assert!(app.undo_erase());
    assert_eq!(app.erase_mask.as_ref(), Some(&before));
    assert!(!app.undo_erase());
    assert!(app.redo_erase());
    let shapes = app.erase_shapes.clone();
    app.save_mask_with_sidecar(0, &filled, &shapes, 9, 9);
    let key = app.page_path_key(0).unwrap();
    let reopened = crate::mask_db::MaskDb::open_at(&crate::mask_db::MaskDb::db_path()).unwrap();
    assert_eq!(reopened.get_full(&key, 9, 9), Some((filled, shapes)));
}
