use super::*;
use egui_kittest::kittest::{NodeT, Queryable};

fn harness() -> egui_kittest::Harness<'static, (crate::mask_db::BucketSource, f32)> {
    crate::test_ui::builder()
        .with_context_setup(|ctx| {
            crate::ui_fonts::configure_fonts(ctx);
            crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
        })
        .with_size(egui::vec2(250.0, 300.0))
        .build_ui_state(
            |ui, (source, tolerance)| {
                crate::test_ui::full_viewport_ui(ui, |ui| {
                    draw_tool_settings_heading(ui, Some("バケツ"), ui.visuals().text_color());
                    draw_bucket_source_controls(ui, source, tolerance);
                    draw_bucket_region_controls(
                        ui,
                        &mut crate::mask_db::BucketRegion::Connected,
                        &mut 0.0,
                        &mut 1.0,
                        &mut 10.0,
                    );
                });
            },
            (crate::mask_db::BucketSource::DrawnMask, 32.0),
        )
}

#[test]
fn mask_bucket_1673_options_snapshot_and_color_control_state() {
    let mut h = harness();
    h.run();
    assert!(
        h.get_by_role_and_label(egui::accesskit::Role::Slider, "色差の許容値")
            .accesskit_node()
            .is_disabled()
    );
    h.snapshot("bucket_drawn_mask_options_dark");
    h.get_by_label("画像の色").click();
    h.run();
    assert_eq!(*h.state(), (crate::mask_db::BucketSource::ImageColor, 32.0));
    assert!(
        !h.get_by_role_and_label(egui::accesskit::Role::Slider, "色差の許容値")
            .accesskit_node()
            .is_disabled()
    );
    h.get_by_label("描いたマスク").click();
    h.run();
    assert_eq!(*h.state(), (crate::mask_db::BucketSource::DrawnMask, 32.0));
}
