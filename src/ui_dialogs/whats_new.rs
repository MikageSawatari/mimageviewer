//! 更新後 初回起動の「重要な変更点」ダイアログ (v2.0.0、version_highlights ④)。
//!
//! - 表示判定 (どのバージョンの変更点を出すか) は `App` 構築時に
//!   [`crate::version_highlights::highlights_to_show`] (純関数) で決め、`whats_new_entries` に入る。
//! - ここはその描画だけを担当する (display-only。移行の二択 UI は持たない)。
//! - `last_seen_version` の更新は `Settings::load` 側で済んでいるので、閉じる際に永続化は不要
//!   (次回起動では previous == current となり再表示されない)。

use crate::app::App;
use eframe::egui;

impl App {
    pub(crate) fn show_whats_new_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_whats_new || self.whats_new_entries.is_empty() {
            return;
        }
        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
        let mut close = false;
        let mut open_changelog = false;
        // &'static 参照の Vec なので clone は安価 (借用衝突回避のためローカルへ)。
        let entries = self.whats_new_entries.clone();

        egui::Window::new("重要な変更点")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_pos(dialog_pos)
            .min_width(440.0)
            .default_height(400.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label("mImageViewer が新しくなりました。主な変更点です。");
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .id_salt("whats_new_scroll")
                    .max_height(320.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        crate::version_highlights::render(ui, &entries);
                    });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("すべての変更を見る").clicked() {
                        open_changelog = true;
                    }
                    if ui.button("閉じる").clicked() {
                        close = true;
                    }
                });
            });

        if open_changelog {
            let url = crate::ui_helpers::manual_url("changelog.html", None);
            crate::ui_helpers::open_url(&url);
        }
        if close || !open || escape_pressed {
            self.show_whats_new = false;
        }
    }
}
