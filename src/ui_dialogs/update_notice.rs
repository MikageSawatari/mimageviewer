//! 「新しいバージョンがあります」ダイアログ。
//!
//! - 起動時 / 定期 (24h) / 手動の更新チェック結果がここに集約される
//! - body (Markdown) はそのままプレーンテキスト表示する (egui には組み込み Markdown
//!   レンダラがないため、ScrollArea で折りたたんで原文を見せる方針)
//! - 「リリースページを開く」「閉じる」「このバージョンの通知をオフ」の 3 ボタン

use crate::app::App;
use eframe::egui;

impl App {
    pub(crate) fn show_update_dialog_window(&mut self, ctx: &egui::Context) {
        if !self.show_update_dialog {
            return;
        }
        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
        let mut close = false;
        let mut open_release_page = false;
        let mut dismiss_this_version = false;
        let info = self.update_info.clone();
        let error = self.update_check_error.clone();
        egui::Window::new("バージョン情報")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_pos(dialog_pos)
            .min_width(420.0)
            .default_height(360.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                // 直近 manual チェックがエラーなら最上部にバナーで表示。
                // (既知の update_info は維持されるので、その下に通常表示が続く)
                if let Some(ref e) = error {
                    ui.label(
                        egui::RichText::new(format!("⚠ 更新確認に失敗しました: {e}"))
                            .color(egui::Color32::from_rgb(220, 120, 120)),
                    );
                    ui.label(
                        egui::RichText::new("ネットワーク接続を確認してください。")
                            .size(11.0)
                            .color(egui::Color32::from_gray(180)),
                    );
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);
                }
                if let Some(ref info) = info {
                    ui.heading(if info.is_newer {
                        "新しいバージョンがあります"
                    } else {
                        "最新バージョンです"
                    });
                    ui.add_space(4.0);
                    egui::Grid::new("update_versions")
                        .num_columns(2)
                        .spacing([8.0, 2.0])
                        .show(ui, |ui| {
                            ui.label("現在のバージョン:");
                            ui.label(
                                egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                                    .monospace(),
                            );
                            ui.end_row();
                            ui.label("最新バージョン:");
                            ui.label(
                                egui::RichText::new(&info.latest_tag)
                                    .monospace()
                                    .color(if info.is_newer {
                                        egui::Color32::from_rgb(100, 170, 100)
                                    } else {
                                        egui::Color32::GRAY
                                    }),
                            );
                            ui.end_row();
                        });
                    if !info.body.is_empty() {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new("更新内容").strong());
                        ui.add_space(2.0);
                        egui::ScrollArea::vertical()
                            .id_salt("update_body_scroll")
                            .max_height(200.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(&info.body)
                                        .size(12.0)
                                        .color(egui::Color32::from_gray(190)),
                                );
                            });
                    }
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("リリースページを開く").clicked() {
                            open_release_page = true;
                        }
                        if info.is_newer
                            && ui
                                .button("このバージョンの通知をオフ")
                                .on_hover_text(
                                    "このバージョンに対する通知バッジを表示しません。\n\
                                     さらに新しいバージョンが出れば再度通知します。",
                                )
                                .clicked()
                        {
                            dismiss_this_version = true;
                        }
                        if ui.button("閉じる").clicked() {
                            close = true;
                        }
                    });
                } else {
                    // 既知の update_info も無く、初回 manual チェックも失敗したケース。
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "現在のバージョン: v{}",
                            env!("CARGO_PKG_VERSION")
                        ))
                        .size(12.0),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("リリースページを開く").clicked() {
                            open_release_page = true;
                        }
                        if ui.button("閉じる").clicked() {
                            close = true;
                        }
                    });
                }
            });
        if open_release_page {
            let url: &str = match info.as_ref() {
                Some(i) => i.release_url.as_str(),
                None => crate::update_check::releases_page_url(),
            };
            crate::ui_helpers::open_url(url);
        }
        if dismiss_this_version {
            if let Some(ref info) = self.update_info {
                self.settings.update_check_dismissed_version = Some(info.latest_tag.clone());
                self.settings.save();
            }
            close = true;
        }
        if close || !open || escape_pressed {
            self.show_update_dialog = false;
            // 一度見せたエラーは消す (次回の manual で再評価)。
            self.update_check_error = None;
        }
    }
}
