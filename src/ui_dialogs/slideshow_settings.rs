//! スライドショー設定ダイアログ。

use eframe::egui;

impl crate::app::App {
    pub(crate) fn show_slideshow_settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_slideshow_settings {
            return;
        }

        // 初回表示時に一時コピーを作成
        if self.slideshow_edit_interval.is_none() {
            self.slideshow_edit_interval = Some(self.settings.slideshow_interval_secs);
        }

        let mut open = true;
        let mut apply = false;
        let mut cancel = false;

        let interval = self.slideshow_edit_interval.as_mut().unwrap();

        egui::Window::new("スライドショー設定")
            .open(&mut open)
            .default_width(300.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("切り替え間隔:");
                    ui.add(
                        egui::Slider::new(interval, 0.5..=30.0)
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

                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("  OK  ").clicked() {
                        apply = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
            });

        if apply {
            if let Some(val) = self.slideshow_edit_interval.take() {
                self.settings.slideshow_interval_secs = val;
                self.settings.save();
            }
            self.show_slideshow_settings = false;
        } else if cancel || !open {
            self.slideshow_edit_interval = None;
            self.show_slideshow_settings = false;
        }
    }
}
