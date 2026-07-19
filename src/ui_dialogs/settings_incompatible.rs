//! settings.db を読み込めず、既定値 + 保存抑止で保護起動した際の案内。

use eframe::egui;

use crate::app::App;

struct SettingsBootProblemCopy {
    heading: &'static str,
    lead: &'static str,
    guidance: &'static str,
}

fn settings_boot_problem_copy(
    source: crate::settings_db::BootSource,
) -> Option<SettingsBootProblemCopy> {
    match source {
        crate::settings_db::BootSource::IncompatibleSettings => Some(SettingsBootProblemCopy {
            heading: "設定を読み込めませんでした",
            lead: "この設定は、現在の mImageViewer より新しいバージョンで保存されています。",
            guidance: "設定を保存したバージョン以降の mImageViewer を起動してください。\n\
                       以前のバージョンへ戻す場合は、「設定の復元」から互換性のある\n\
                       バックアップを選べます。バックアップを作成したバージョンも一覧で確認できます。",
        }),
        crate::settings_db::BootSource::FailedFallbackDefault => Some(SettingsBootProblemCopy {
            heading: "設定の読み込みに失敗しました",
            lead: "設定ファイルとバックアップを読み込めなかったため、既定の設定で保護起動しました。",
            guidance: "一時的なファイルロックの場合は、アプリを終了してもう一度起動してください。\n\
                           改善しない場合は、「設定の復元」から利用できるバックアップを確認できます。",
        }),
        _ => None,
    }
}

impl App {
    pub(crate) fn show_settings_boot_problem_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_settings_boot_problem_notice {
            return;
        }
        let Some(source) = self.settings_boot_problem_source else {
            self.show_settings_boot_problem_notice = false;
            return;
        };

        let Some(copy) = settings_boot_problem_copy(source) else {
            self.show_settings_boot_problem_notice = false;
            return;
        };

        let mut open_restore = false;
        let mut quit = false;
        egui::Modal::new(egui::Id::new("settings_boot_problem_modal")).show(ctx, |ui| {
            ui.set_min_width(500.0);
            ui.heading(copy.heading);
            ui.add_space(8.0);
            ui.label(copy.lead);
            ui.label("設定ファイルとバックアップは変更せず、この起動中の設定保存を停止しました。");
            ui.add_space(8.0);
            ui.label(format!("現在のアプリ: v{}", env!("CARGO_PKG_VERSION")));
            ui.add_space(8.0);
            ui.label(copy.guidance);
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("設定の復元を開く").clicked() {
                    open_restore = true;
                }
                if ui.button("アプリを終了").clicked() {
                    quit = true;
                }
            });
        });

        if open_restore {
            self.show_settings_boot_problem_notice = false;
            self.open_settings_restore_dialog();
        } else if quit {
            self.request_application_quit(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_problem_copy_distinguishes_incompatible_and_unreadable_settings() {
        let incompatible =
            settings_boot_problem_copy(crate::settings_db::BootSource::IncompatibleSettings)
                .unwrap();
        assert_eq!(incompatible.heading, "設定を読み込めませんでした");
        assert!(incompatible.guidance.contains("バージョン以降"));

        let unreadable =
            settings_boot_problem_copy(crate::settings_db::BootSource::FailedFallbackDefault)
                .unwrap();
        assert_eq!(unreadable.heading, "設定の読み込みに失敗しました");
        assert!(unreadable.guidance.contains("もう一度起動"));

        assert!(settings_boot_problem_copy(crate::settings_db::BootSource::CleanInstall).is_none());
    }
}
