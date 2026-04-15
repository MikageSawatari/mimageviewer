//! 回転情報リセット確認ダイアログ。

use eframe::egui;

impl crate::app::App {
    pub(crate) fn show_rotation_reset_confirm_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_rotation_reset_confirm {
            return;
        }

        let count = self
            .rotation_db
            .as_ref()
            .map(|db| db.count())
            .unwrap_or(0);

        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        egui::Window::new("回転情報のリセット")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!(
                    "登録されている回転情報 ({count} 件) をすべて削除しますか？"
                ));
                ui.label("この操作は元に戻せません。");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("リセット").clicked() {
                        if let Some(ref db) = self.rotation_db {
                            let deleted = db.clear_all().unwrap_or(0);
                            self.rotation_cache.clear();
                            crate::logger::log(format!(
                                "[rotation] Reset: {deleted} entries cleared"
                            ));
                        }
                        self.show_rotation_reset_confirm = false;
                    }
                    if ui.button("キャンセル").clicked() || escape_pressed {
                        self.show_rotation_reset_confirm = false;
                    }
                });
            });

        if !open {
            self.show_rotation_reset_confirm = false;
        }
    }
}
