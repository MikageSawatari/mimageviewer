//! 同名ファイル処理設定ダイアログ。

use eframe::egui;

impl crate::app::App {
    pub(crate) fn show_duplicate_settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_duplicate_settings {
            return;
        }

        let mut open = true;
        let mut apply = false;
        let mut cancel = false;

        // 初回表示時に一時コピーを作成
        if self.dup_edit.is_none() {
            self.dup_edit = Some(DupEdit {
                skip_zip: self.settings.skip_zip_if_folder_exists,
                skip_image: self.settings.skip_image_if_video_exists,
                skip_dup: self.settings.skip_duplicate_images,
                ext_priority: self.settings.image_ext_priority.clone(),
            });
        }

        let edit = self.dup_edit.as_mut().unwrap();

        egui::Window::new("同名ファイル処理")
            .open(&mut open)
            .default_width(450.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.checkbox(&mut edit.skip_zip, "同名の ZIP ファイルとフォルダがある場合、ZIP をスキップ");
                ui.add_space(4.0);
                ui.checkbox(&mut edit.skip_image, "同名の動画と画像がある場合、画像をスキップ");
                ui.add_space(4.0);
                ui.checkbox(&mut edit.skip_dup, "同名の画像が複数拡張子で存在する場合、優先度で選択");

                if edit.skip_dup {
                    ui.add_space(4.0);
                    ui.indent("ext_priority", |ui| {
                        ui.label(
                            egui::RichText::new("拡張子の優先度（上が最優先）:")
                                .size(12.0)
                                .color(egui::Color32::from_gray(160)),
                        );
                        ui.add_space(2.0);

                        let mut swap: Option<(usize, usize)> = None;
                        let len = edit.ext_priority.len();

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
                                        ui.label(&edit.ext_priority[i]);
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if i + 1 < len && ui.small_button("▼").clicked() {
                                                    swap = Some((i, i + 1));
                                                }
                                                if i > 0 && ui.small_button("▲").clicked() {
                                                    swap = Some((i, i - 1));
                                                }
                                            },
                                        );
                                    });
                                }
                            });

                        if let Some((a, b)) = swap {
                            edit.ext_priority.swap(a, b);
                        }

                        ui.add_space(4.0);
                        if ui.button("デフォルトに戻す").clicked() {
                            edit.ext_priority = crate::settings::default_image_ext_priority();
                        }
                    });
                }

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
            if let Some(edit) = self.dup_edit.take() {
                self.settings.skip_zip_if_folder_exists = edit.skip_zip;
                self.settings.skip_image_if_video_exists = edit.skip_image;
                self.settings.skip_duplicate_images = edit.skip_dup;
                self.settings.image_ext_priority = edit.ext_priority;
                self.settings.save();
                // フォルダを再読み込みして変更を反映
                if let Some(folder) = self.current_folder.clone() {
                    self.load_folder(folder);
                }
            }
            self.show_duplicate_settings = false;
        } else if cancel || !open {
            self.dup_edit = None;
            self.show_duplicate_settings = false;
        }
    }
}

/// 同名ファイル処理の一時編集状態
#[derive(Clone)]
pub(crate) struct DupEdit {
    pub skip_zip: bool,
    pub skip_image: bool,
    pub skip_dup: bool,
    pub ext_priority: Vec<String>,
}
