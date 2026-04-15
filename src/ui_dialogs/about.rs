//! バージョン情報ダイアログ。

use eframe::egui;
use crate::app::App;

impl App {
    pub(crate) fn show_about_dialog_window(&mut self, ctx: &egui::Context) {
        if !self.show_about_dialog {
            return;
        }
        let mut open = true;
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
        egui::Window::new("バージョン情報")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.heading("mImageViewer");
                    ui.label(format!("v{}", env!("CARGO_PKG_VERSION")));
                    ui.add_space(8.0);
                    ui.label("© 2025 Mikage Sawatari");
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(4.0);

                // サードパーティライセンス
                ui.label(egui::RichText::new("サードパーティ ライセンス").strong());
                ui.add_space(4.0);
                egui::Grid::new("third_party_licenses")
                    .num_columns(2)
                    .spacing([8.0, 2.0])
                    .show(ui, |ui| {
                        ui.label("ONNX Runtime");
                        ui.label("MIT — Microsoft");
                        ui.end_row();

                        ui.label("Real-ESRGAN");
                        ui.label("BSD-3-Clause — Xintao");
                        ui.end_row();

                        ui.label("Real-CUGAN");
                        ui.label("MIT — bilibili");
                        ui.end_row();

                        ui.label("4x-NMKD-Siax-200k");
                        ui.label("WTFPL — Nmkd");
                        ui.end_row();

                        ui.label("MI-GAN");
                        ui.label("MIT");
                        ui.end_row();

                        ui.label("1xDeJPG_realplksr_otf");
                        ui.label("CC-BY-4.0 — Phhofm");
                        ui.end_row();

                        ui.label("anime_classification");
                        ui.label("MIT — deepghs");
                        ui.end_row();
                    });

                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    if ui.button("閉じる").clicked() {
                        self.show_about_dialog = false;
                    }
                });
            });
        if !open {
            self.show_about_dialog = false;
        }
    }
}
