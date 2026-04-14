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
                    ui.add_space(8.0);
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
