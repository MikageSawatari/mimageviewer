//! スライドショー設定ダイアログ。

use eframe::egui;

impl crate::app::App {
    pub(crate) fn show_slideshow_settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_slideshow_settings {
            return;
        }

        let mut open = true;
        egui::Window::new("スライドショー設定")
            .open(&mut open)
            .default_width(300.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("切り替え間隔:");
                    ui.add(
                        egui::Slider::new(
                            &mut self.settings.slideshow_interval_secs,
                            0.5..=30.0,
                        )
                        .suffix(" 秒")
                        .fixed_decimals(1),
                    );
                });

                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("フルスクリーンで Space キーまたは ▶ ボタンで開始")
                        .size(11.0)
                        .color(egui::Color32::from_gray(140)),
                );

                ui.add_space(8.0);
                if ui.button("保存").clicked() {
                    self.settings.save();
                    self.show_slideshow_settings = false;
                }
            });

        if !open {
            self.show_slideshow_settings = false;
        }
    }
}
