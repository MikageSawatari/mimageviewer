use super::*;
use egui_kittest::kittest::{NodeT, Queryable};

fn harness(width: f32) -> egui_kittest::Harness<'static, (crate::mask_db::BucketSource, f32)> {
    crate::test_ui::builder()
        .with_context_setup(|ctx| {
            crate::ui_fonts::configure_fonts(ctx);
            crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
        })
        .with_size(egui::vec2(width, 300.0))
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
    let mut h = harness(250.0);
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

#[test]
fn bucket_source_buttons_share_region_dimensions_at_each_panel_width() {
    // 消しゴム・伏せ字は 200pt、ローカル調整は 300pt。実フォントで両基準を確認する。
    for width in [200.0, 300.0] {
        let mut h = harness(width);
        for source_label in ["画像の色", "描いたマスク"] {
            h.run();
            h.get_by_label(source_label).click();
            h.run();
            let image = h.get_by_label("画像の色").rect();
            let mask = h.get_by_label("描いたマスク").rect();
            let whole = h.get_by_label("全体").rect();
            let connected = h.get_by_label("隣接のみ").rect();
            for (left, right) in [(image, mask), (whole, connected)] {
                assert!((left.width() - right.width()).abs() <= 0.5);
                assert!((left.top() - right.top()).abs() <= 0.5);
                assert!((right.left() - left.right() - 4.0).abs() <= 0.5);
                assert!((left.union(right).width() - width).abs() <= 0.5);
                for rect in [left, right] {
                    // panel_toggle_button の 24pt は最小高。実フォントの行高を尊重する。
                    assert!(rect.height() >= 24.0);
                    assert!((rect.height() - whole.height()).abs() <= 0.5);
                    assert!(h.ctx.content_rect().contains_rect(rect));
                }
            }
            assert!((image.width() - whole.width()).abs() <= 0.5);
        }
    }
}
