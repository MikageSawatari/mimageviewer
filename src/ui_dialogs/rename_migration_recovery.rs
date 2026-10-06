//! Explicit recovery for a rename-migration journal that no supported format can parse.

use eframe::egui;

use crate::app::App;

impl App {
    pub(crate) fn show_rename_migration_recovery_dialog(&mut self, ctx: &egui::Context) {
        if !self.rename_migration_recovery_dialog_visible() {
            return;
        }
        let quarantining = self.rename_migration_recovery_quarantine_running();
        let response = draw_rename_recovery_dialog(ctx, quarantining);
        let (retry, quarantine, cancel) = response.inner;
        if response.should_close() || self.dialog_escape_pressed(ctx) || cancel {
            self.dismiss_rename_migration_recovery();
        } else if retry {
            self.retry_rename_migration_recovery(ctx);
        } else if quarantine {
            self.quarantine_rename_migration_recovery(ctx);
        }
    }
}

pub(super) fn draw_rename_recovery_dialog(
    ctx: &egui::Context,
    quarantining: bool,
) -> egui::ModalResponse<(bool, bool, bool)> {
    let mut retry = false;
    let mut quarantine = false;
    let mut cancel = false;
    let response =
            egui::Modal::new(egui::Id::new("rename_migration_recovery_dialog")).show(ctx, |ui| {
                ui.set_width(520.0_f32.min((ctx.content_rect().width() - 48.0).max(1.0)));
                ui.heading("名前変更の復旧記録");
                ui.add_space(8.0);
                let footer = if quarantining {
                    let status = egui::WidgetText::from("壊れた記録を確認して退避しています…")
                        .into_galley(ui, Some(egui::TextWrapMode::Wrap), (ui.available_width() - ui.spacing().interact_size.y - ui.spacing().item_spacing.x).max(1.0), egui::TextStyle::Body);
                    super::startup_dialog_footer_height(ui, &["キャンセル"], 20.0 + status.size().y.max(ui.spacing().interact_size.y))
                } else {
                    super::startup_dialog_footer_height(ui, &["再読み込み", "壊れた記録を退避して再開", "閉じる"], 12.0)
                };
                super::startup_dialog_scroll_body(ui, "rename_recovery_body", footer, |ui| {
                ui.label(
                    "名前変更の復旧記録が壊れているため、名前変更に伴う設定やコレクション参照の引き継ぎを自動で再開できません。",
                );
                ui.add_space(8.0);
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "記録を外部で修正した場合は「再読み込み」を選んでください。退避すると、記録にだけ残っていた未完了の引き継ぎは自動再開できない可能性があります。",
                );
            });
                ui.add_space(12.0);
                if quarantining {
                    ui.horizontal_wrapped(|ui| {
                        ui.spinner();
                        ui.label("壊れた記録を確認して退避しています…");
                    });
                    ui.add_space(8.0);
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                } else {
                    ui.horizontal_wrapped(|ui| {
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
                (retry, quarantine, cancel)
            });
    response
}
