//! 同名ファイル処理設定ダイアログ。

use eframe::egui;

impl crate::app::App {
    pub(crate) fn show_duplicate_settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_duplicate_settings {
            return;
        }

        let mut open = true;
        let mut changed = false;

        egui::Window::new("同名ファイル処理")
            .open(&mut open)
            .default_width(450.0)
            .resizable(false)
            .show(ctx, |ui| {
                // 1. ZIP + フォルダ
                if ui
                    .checkbox(
                        &mut self.settings.skip_zip_if_folder_exists,
                        "同名の ZIP ファイルとフォルダがある場合、ZIP をスキップ",
                    )
                    .changed()
                {
                    changed = true;
                }

                ui.add_space(4.0);

                // 2. 動画 + 画像
                if ui
                    .checkbox(
                        &mut self.settings.skip_image_if_video_exists,
                        "同名の動画と画像がある場合、画像をスキップ",
                    )
                    .changed()
                {
                    changed = true;
                }

                ui.add_space(4.0);

                // 3. 画像拡張子重複
                if ui
                    .checkbox(
                        &mut self.settings.skip_duplicate_images,
                        "同名の画像が複数拡張子で存在する場合、優先度で選択",
                    )
                    .changed()
                {
                    changed = true;
                }

                // 拡張子優先度リスト
                if self.settings.skip_duplicate_images {
                    ui.add_space(4.0);
                    ui.indent("ext_priority", |ui| {
                        ui.label(
                            egui::RichText::new("拡張子の優先度（上が最優先）:")
                                .size(12.0)
                                .color(egui::Color32::from_gray(160)),
                        );
                        ui.add_space(2.0);

                        let mut swap: Option<(usize, usize)> = None;
                        let len = self.settings.image_ext_priority.len();

                        egui::ScrollArea::vertical()
                            .max_height(200.0)
                            .show(ui, |ui| {
                                for i in 0..len {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(format!("{}.", i + 1))
                                                .size(11.0)
                                                .color(egui::Color32::from_gray(140)),
                                        );
                                        ui.label(&self.settings.image_ext_priority[i]);

                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if i + 1 < len {
                                                    if ui.small_button("▼").clicked() {
                                                        swap = Some((i, i + 1));
                                                    }
                                                }
                                                if i > 0 {
                                                    if ui.small_button("▲").clicked() {
                                                        swap = Some((i, i - 1));
                                                    }
                                                }
                                            },
                                        );
                                    });
                                }
                            });

                        if let Some((a, b)) = swap {
                            self.settings.image_ext_priority.swap(a, b);
                            changed = true;
                        }

                        ui.add_space(4.0);
                        if ui.button("デフォルトに戻す").clicked() {
                            self.settings.image_ext_priority =
                                crate::settings::default_image_ext_priority();
                            changed = true;
                        }
                    });
                }

                if changed {
                    self.settings.save();
                }
            });

        if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_duplicate_settings = false;
        }
    }
}
