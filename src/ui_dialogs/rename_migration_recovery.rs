//! Explicit recovery for a rename-migration journal that no supported format can parse.

use eframe::egui;

use crate::app::App;

impl App {
    pub(crate) fn show_rename_migration_recovery_dialog(&mut self, ctx: &egui::Context) {
        if !self.rename_migration_recovery_dialog_visible() {
            return;
        }
        let quarantining = self.rename_migration_recovery_quarantine_running();
        let mut retry = false;
        let mut quarantine = false;
        let mut cancel = false;
        let response =
            egui::Modal::new(egui::Id::new("rename_migration_recovery_dialog")).show(ctx, |ui| {
                ui.set_min_width(520.0);
                ui.heading("名前変更の復旧記録");
                ui.add_space(8.0);
                ui.label(
                    "名前変更の復旧記録が壊れているため、名前変更に伴う設定やコレクション参照の引き継ぎを自動で再開できません。",
                );
                ui.add_space(8.0);
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "記録を外部で修正した場合は「再読み込み」を選んでください。退避すると、記録にだけ残っていた未完了の引き継ぎは自動再開できない可能性があります。",
                );
                ui.add_space(12.0);
                if quarantining {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("壊れた記録を確認して退避しています…");
                    });
                    ui.add_space(8.0);
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                } else {
                    ui.horizontal(|ui| {
                        if ui.button("再読み込み").clicked() {
                            retry = true;
                        }
                        if ui.button("壊れた記録を退避して再開").clicked() {
                            quarantine = true;
                        }
                        if ui.button("閉じる").clicked() {
                            cancel = true;
                        }
                    });
                }
            });
        if response.should_close() || self.dialog_escape_pressed(ctx) || cancel {
            self.dismiss_rename_migration_recovery();
        } else if retry {
            self.retry_rename_migration_recovery(ctx);
        } else if quarantine {
            self.quarantine_rename_migration_recovery(ctx);
        }
    }
}
