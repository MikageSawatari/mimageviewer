use eframe::egui;

use crate::app::App;
use crate::settings::{AiFeatureMode, UiTheme};

impl App {
    pub(crate) fn show_first_setup_dialog(&mut self, ctx: &egui::Context) {
        if self.settings.first_setup_completed {
            return;
        }

        let mut completed = false;
        egui::Modal::new(egui::Id::new("first_setup_modal")).show(ctx, |ui| {
            ui.set_min_width(440.0);
            ui.heading("初回設定");
            ui.add_space(8.0);
            ui.label("使い始める前に、表示とAI処理の基本設定を選んでください。");

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
            ui.label(egui::RichText::new("テーマ").strong());
            if self.settings.ui_theme == UiTheme::Standard {
                self.settings.ui_theme = UiTheme::Light;
            }
            ui.radio_value(
                &mut self.settings.ui_theme,
                UiTheme::System,
                "システムと同じ",
            );
            ui.radio_value(&mut self.settings.ui_theme, UiTheme::Light, "ライト");
            ui.radio_value(&mut self.settings.ui_theme, UiTheme::Dark, "ダーク");

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("表示時の AI 処理 (アップスケール / ノイズ除去)").strong(),
            );
            for &mode in AiFeatureMode::all() {
                ui.radio_value(
                    &mut self.settings.ai_feature_mode,
                    mode,
                    format!("{} - {}", mode.label(), mode.description()),
                );
            }
            ui.label(
                egui::RichText::new(
                    "画像を見るときの自動アップスケール / ノイズ除去だけを切り替えます。\n\
                     消しゴムや補正の被写体マスクなど編集ツールの AI は影響を受けません。\n\
                     高画質には、GPU によっては処理に時間がかかる AI モデルが含まれます。",
                )
                .weak(),
            );
            if ui
                .link("処理時間の目安を開く")
                .on_hover_text("ブラウザでマニュアルの AI 処理時間表を開きます。")
                .clicked()
            {
                let url =
                    crate::ui_helpers::manual_url("settings.html", Some("ai-processing-time"));
                crate::ui_helpers::open_url(&url);
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
            ui.label(egui::RichText::new("ZIP/PDF ファイル").strong());
            ui.radio_value(
                &mut self.settings.auto_fullscreen_zip_pdf,
                false,
                "開いたとき、ページ一覧を表示",
            );
            ui.radio_value(
                &mut self.settings.auto_fullscreen_zip_pdf,
                true,
                "開いたとき、ページをフルスクリーン表示",
            );

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("開始").clicked() {
                    completed = true;
                }
            });
        });

        if completed {
            self.settings.first_setup_completed = true;
            self.settings.save();
            self.apply_ai_feature_mode_change();
        }
    }
}
