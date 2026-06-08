//! バージョン情報ダイアログ。

use crate::app::App;
use eframe::egui;

impl App {
    pub(crate) fn show_about_dialog_window(&mut self, ctx: &egui::Context) {
        if !self.show_about_dialog {
            return;
        }
        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        // 追加パックのライセンスサマリは closure の借用衝突を避けるため事前にキャプチャする。
        let pack_about = self.ensure_editing_pack_about();
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

                        ui.label("FFmpeg");
                        ui.label("LGPLv3-or-later — FFmpeg project");
                        ui.end_row();

                        ui.label("UnRAR");
                        ui.label("UnRAR license — Alexander Roshal / RARLAB");
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

                        ui.label("絵文字 (Twemoji)");
                        ui.label("CC-BY 4.0 — Twitter, Inc. and other contributors");
                        ui.end_row();
                    });

                // 編集用追加パック (導入済みのときだけ表示、spec §10)。
                if let Some(pack) = &pack_about {
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("編集用追加パック (v{})", pack.version))
                            .strong(),
                    );
                    ui.add_space(4.0);
                    egui::Grid::new("editing_pack_licenses")
                        .num_columns(2)
                        .spacing([8.0, 2.0])
                        .show(ui, |ui| {
                            ui.label(format!("オノマトペ向けフォント ({}書体)", pack.font_count));
                            ui.label(format!("{} — Google Fonts 提供", pack.font_license));
                            ui.end_row();

                            ui.label(format!("被写体分離 ({})", pack.model_id));
                            ui.label(format!("{} — ZhengPeng7/BiRefNet", pack.model_license));
                            ui.end_row();
                        });
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new("各ライセンス全文は追加パック内に同梱されています。")
                            .size(10.0)
                            .color(egui::Color32::from_gray(150)),
                    );
                }

                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    if ui.button("閉じる").clicked() {
                        self.show_about_dialog = false;
                    }
                });
            });
        if !open || escape_pressed {
            self.show_about_dialog = false;
        }
    }
}
