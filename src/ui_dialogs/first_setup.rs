use eframe::egui;

use crate::app::App;
use crate::settings::{AiFeatureMode, UiTheme};

impl App {
    pub(crate) fn show_first_setup_dialog(&mut self, ctx: &egui::Context) {
        // 新しい版の settings.db を読めず defaults で保護起動した場合は「初回」ではない。
        // 復元案内を優先し、既定値を初回設定として保存しようとする誤解を防ぐ。
        if self.settings_incompatible_at_boot || self.settings.first_setup_completed {
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
                     AI 処理は GPU 負荷が高いため、表示が重い環境では「なし」を推奨します。\n\
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
            ui.label(egui::RichText::new("ビューワモード").strong());
            ui.radio_value(
                &mut self.settings.detached_viewer_open_images_in_window,
                false,
                "フル機能ウィンドウ（編集機能あり）",
            );
            // サブ選択肢 (本の開き方 / 画像フォルダ) は ui.indent で 1 段字下げして、
            // 上位のモード選択 (フル機能 / 複数ウィンドウ) との階層を視覚的に分ける。
            // 字下げが無いと 4 つの radio が同列に見えて何を選ぶ設定か分からなくなる
            // (環境設定ページ pages.rs の viewer_mode_* と同じパターン)。
            ui.indent("first_setup_viewer_mode_full", |ui| {
                ui.add_enabled_ui(!self.settings.detached_viewer_open_images_in_window, |ui| {
                    ui.radio_value(
                        &mut self.settings.auto_fullscreen_zip_pdf,
                        false,
                        "本はページ一覧を表示して開く",
                    );
                    ui.radio_value(
                        &mut self.settings.auto_fullscreen_zip_pdf,
                        true,
                        "本はページを表示して開く",
                    );
                    ui.add_enabled_ui(self.settings.auto_fullscreen_zip_pdf, |ui| {
                        ui.checkbox(
                            &mut self.settings.auto_fullscreen_image_folders,
                            "画像のみのフォルダは、PDF/ZIP のように本として扱う",
                        );
                    });
                });
            });
            ui.add_space(8.0);
            ui.radio_value(
                &mut self.settings.detached_viewer_open_images_in_window,
                true,
                "複数ウィンドウ（編集機能なし）",
            );
            ui.indent("first_setup_viewer_mode_multi", |ui| {
                ui.add_enabled_ui(self.settings.detached_viewer_open_images_in_window, |ui| {
                    ui.checkbox(
                        &mut self.settings.auto_fullscreen_image_folders,
                        "画像のみのフォルダは、PDF/ZIP のように本として扱う",
                    );
                });
            });

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
