//! Memory-only startup status rendering; no lifecycle or diagnostic writer ownership.

use std::time::Duration;

pub fn draw_status(ui: &mut egui::Ui, stage: &str, elapsed: Duration, total: Duration) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.3);
        ui.spinner();
        ui.add_space(12.0);
        ui.label(egui::RichText::new("起動中…").size(20.0).strong());
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(format!(
                "{stage}\nこの処理 {:.1}秒 / 起動から {:.1}秒",
                elapsed.as_secs_f64(),
                total.as_secs_f64()
            ))
            .size(14.0)
            .color(ui.visuals().weak_text_color()),
        );
    });
}

pub fn draw_preparing(ui: &mut egui::Ui) {
    ui.label("検索の準備中");
}
