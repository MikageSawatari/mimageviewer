//! kittest 0.37 reports Node::rect in logical points, like egui responses.

pub fn assert_node_inside(ctx: &egui::Context, rect: egui::Rect, detail: &str) {
    let viewport = ctx.content_rect();
    assert!(
        viewport.contains_rect(rect),
        "{detail}: node {rect:?} outside logical viewport {viewport:?}"
    );
}

#[test]
fn high_dpi_node_bounds_reject_a_control_outside_the_logical_viewport() {
    use egui_kittest::{Harness, kittest::Queryable};
    let mut h = Harness::builder()
        .with_size(egui::vec2(640.0, 480.0))
        .build_ui(|ui| {
            ui.ctx().set_zoom_factor(2.0);
            // The original physical viewport would wrongly allow this button.
            ui.put(
                egui::Rect::from_min_size(egui::pos2(400.0, 100.0), egui::vec2(80.0, 24.0)),
                egui::Button::new("Outside"),
            );
        });
    h.run();
    let rect = h.get_by_label("Outside").rect();
    assert!(
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0)).contains_rect(rect)
    );
    assert!(!h.ctx.content_rect().contains_rect(rect));
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_node_inside(&h.ctx, rect, "high-DPI negative control");
        }))
        .is_err(),
        "a physical-sized boundary must not admit an off-viewport control"
    );
}
