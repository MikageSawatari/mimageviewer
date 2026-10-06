//! 「新しいバージョンがあります」ダイアログ。
//!
//! - 起動時 / 定期 (24h) / 手動の更新チェック結果がここに集約される
//! - body (GitHub release の Markdown) は [crate::changelog_markdown] で整形描画する
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
        let mut close = false;
        let mut open_release_page = false;
        let mut dismiss_this_version = false;
        let info = self.update_info.clone();
        let error = self.update_check_error.clone();
        let response = draw_update_notice_dialog(ctx, &mut open, info.as_ref(), error.as_deref());
        close |= response.close;
        open_release_page |= response.open_release_page;
        dismiss_this_version |= response.dismiss_this_version;

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

#[derive(Default)]
pub(super) struct UpdateNoticeResponse {
    close: bool,
    open_release_page: bool,
    dismiss_this_version: bool,
}

pub(super) fn draw_update_notice_dialog(
    ctx: &egui::Context,
    open: &mut bool,
    info: Option<&crate::update_check::UpdateInfo>,
    error: Option<&str>,
) -> UpdateNoticeResponse {
    let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
    let mut response = UpdateNoticeResponse::default();
    egui::Window::new("バージョン情報")
            .open(open)
            .collapsible(false)
            .resizable(true)
            .default_pos(dialog_pos)
            .min_width(420.0)
            .default_height(360.0)
            .show(ctx, |ui| {
                super::startup_dialog_scroll_body(ui, "update_notice_body", 360.0, |ui| {
                ui.add_space(4.0);
                // 直近 manual チェックがエラーなら最上部にバナーで表示。
                // (既知の update_info は維持されるので、その下に通常表示が続く)
                if let Some(e) = error {
                    ui.label(
                        egui::RichText::new(format!("⚠ 更新確認に失敗しました: {e}"))
                            .color(ui.visuals().error_fg_color),
                    );
                    ui.label(
                        egui::RichText::new("ネットワーク接続を確認してください。")
                            .size(11.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);
                }
                if let Some(info) = info {
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
                            ui.label(egui::RichText::new(&info.latest_tag).monospace().color(
                                if info.is_newer {
                                    egui::Color32::from_rgb(100, 170, 100)
                                } else {
                                    egui::Color32::GRAY
                                },
                            ));
                            ui.end_row();
                        });
                    if !info.body.is_empty() {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new("更新内容").strong());
                        ui.add_space(2.0);
                        crate::changelog_markdown::render(ui, &info.body);

                    }
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
                }
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("リリースページを開く").clicked() {
                        response.open_release_page = true;
                    }
                    if info.is_some_and(|info| info.is_newer)
                        && ui.button("このバージョンの通知をオフ")
                            .on_hover_text("このバージョンに対する通知バッジを表示しません。\nさらに新しいバージョンが出れば再度通知します。")
                            .clicked()
                    {
                        response.dismiss_this_version = true;
                    }
                    if ui.button("閉じる").clicked() {
                        response.close = true;
                    }
                });
            });
    response
}
