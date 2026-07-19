//! 現在のバイナリより新しい版で保存された settings.db を検出した際の案内。

use eframe::egui;

use crate::app::App;

impl App {
    pub(crate) fn show_settings_incompatible_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_settings_incompatible_notice {
            return;
        }

        let mut open_restore = false;
        let mut quit = false;
        egui::Modal::new(egui::Id::new("settings_incompatible_modal")).show(ctx, |ui| {
            ui.set_min_width(500.0);
            ui.heading("設定を読み込めませんでした");
            ui.add_space(8.0);
            ui.label(
                "この設定は、現在の mImageViewer より新しいバージョンで保存されています。",
            );
            ui.label("設定ファイルとバックアップは変更せず、この起動中の設定保存を停止しました。");
            ui.add_space(8.0);
            ui.label(format!(
                "現在のアプリ: v{}",
                env!("CARGO_PKG_VERSION")
            ));
            ui.add_space(8.0);
            ui.label(
                "以前のバージョンを使用する場合は、「設定の復元」から互換性のある\n\
                 バックアップへ戻してください。バックアップを作成したバージョンも一覧で確認できます。",
            );
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
            self.show_settings_incompatible_notice = false;
            self.open_settings_restore_dialog();
        } else if quit {
            self.request_application_quit(ctx);
        }
    }
}
