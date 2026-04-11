//! EXIF 表示設定ダイアログ。
//!
//! 非表示にする EXIF タグ名を編集する。

use eframe::egui;

impl crate::app::App {
    pub(crate) fn show_exif_settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_exif_settings {
            return;
        }

        let mut open = true;
        egui::Window::new("EXIF 表示設定")
            .open(&mut open)
            .default_width(400.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label("非表示にする EXIF タグ名:");
                ui.add_space(4.0);

                let mut to_remove: Option<usize> = None;
                let avail_w = ui.available_width();

                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        ui.set_min_width(avail_w);
                        for (i, tag) in self.settings.exif_hidden_tags.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.set_min_width(avail_w - 8.0);
                                ui.label(tag);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("×").clicked() {
                                            to_remove = Some(i);
                                        }
                                    },
                                );
                            });
                        }
                    });

                if let Some(idx) = to_remove {
                    self.settings.exif_hidden_tags.remove(idx);
                    self.settings.save();
                    self.exif_cache.clear();
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.label("追加:");
                    let response = ui.text_edit_singleline(&mut self.exif_add_tag_input);
                    if (ui.button("追加").clicked()
                        || response.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        && !self.exif_add_tag_input.trim().is_empty()
                    {
                        let tag = self.exif_add_tag_input.trim().to_string();
                        if !self.settings.exif_hidden_tags.contains(&tag) {
                            self.settings.exif_hidden_tags.push(tag);
                            self.settings.save();
                            self.exif_cache.clear();
                        }
                        self.exif_add_tag_input.clear();
                    }
                });

                ui.add_space(8.0);
                if ui.button("デフォルトに戻す").clicked() {
                    self.settings.exif_hidden_tags =
                        crate::settings::default_exif_hidden_tags();
                    self.settings.save();
                    self.exif_cache.clear();
                }
            });

        if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_exif_settings = false;
        } else {
            self.show_exif_settings = open;
        }
    }
}
