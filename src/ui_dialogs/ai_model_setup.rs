//! AI モデルダウンロードダイアログ。
//!
//! AI 機能を有効化した際にモデルが未ダウンロードの場合に表示する。
//! モデル一覧・合計サイズ・ダウンロード進捗を表示し、
//! バックグラウンドダウンロードを管理する。

use crate::ai::ModelKind;
use crate::ai::model_manager::DownloadState;

impl crate::app::App {
    /// AI モデルセットアップダイアログを表示する。
    pub(crate) fn show_ai_model_setup_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_ai_model_setup {
            return;
        }

        // ダウンロード状態をポーリング
        if let Some(ref mgr) = self.ai_model_manager {
            mgr.poll_downloads();
        }

        let mut open = true;
        let mut start_download = false;

        egui::Window::new("AI モデルのセットアップ")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(ctx.content_rect().center() - egui::vec2(220.0, 180.0))
            .show(ctx, |ui| {
                ui.set_min_width(440.0);

                ui.label("AI 機能を使用するには ONNX モデルファイルが必要です。");
                ui.add_space(8.0);

                let manager = self.ai_model_manager.clone();
                let Some(manager) = manager else {
                    ui.label("AI ランタイムが初期化されていません。");
                    return;
                };

                let mut any_downloading = false;
                let mut all_ready = true;

                // モデル一覧を表示
                egui::Grid::new("ai_model_list")
                    .num_columns(3)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("モデル").strong().size(13.0));
                        ui.label(egui::RichText::new("サイズ").strong().size(13.0));
                        ui.label(egui::RichText::new("状態").strong().size(13.0));
                        ui.end_row();

                        let models = [
                            ModelKind::ClassifierMobileNet,
                            ModelKind::UpscaleRealEsrganX4Plus,
                            ModelKind::UpscaleRealEsrganAnime6B,
                            ModelKind::UpscaleWaifu2xCunet,
                            ModelKind::UpscaleRealEsrGeneralV3,
                            ModelKind::InpaintLama,
                        ];

                        for &kind in &models {
                            ui.label(kind.display_label());

                            let state = manager.download_state(kind);
                            match &state {
                                DownloadState::Ready(_) => {
                                    let size = crate::ai::model_manager::ModelManager::model_size(kind);
                                    ui.label(format_size(size));
                                    ui.label(
                                        egui::RichText::new("✓ 完了")
                                            .color(egui::Color32::from_rgb(80, 200, 80)),
                                    );
                                }
                                DownloadState::Downloading { progress, total, .. } => {
                                    any_downloading = true;
                                    all_ready = false;
                                    let done = progress.load(std::sync::atomic::Ordering::Relaxed);
                                    let t = total.load(std::sync::atomic::Ordering::Relaxed);
                                    ui.label(format!("{} / {}", format_size(done), format_size(t)));
                                    let pct = if t > 0 {
                                        done as f32 / t as f32
                                    } else {
                                        0.0
                                    };
                                    ui.add(
                                        egui::ProgressBar::new(pct)
                                            .desired_width(120.0)
                                            .show_percentage(),
                                    );
                                }
                                DownloadState::Failed(msg) => {
                                    all_ready = false;
                                    let size = crate::ai::model_manager::ModelManager::model_size(kind);
                                    ui.label(format_size(size));
                                    ui.label(
                                        egui::RichText::new(format!("✗ {msg}"))
                                            .color(egui::Color32::from_rgb(220, 80, 80))
                                            .small(),
                                    );
                                }
                                DownloadState::NotDownloaded => {
                                    all_ready = false;
                                    let size = crate::ai::model_manager::ModelManager::model_size(kind);
                                    ui.label(format_size(size));
                                    ui.label(
                                        egui::RichText::new("—")
                                            .color(egui::Color32::from_gray(130)),
                                    );
                                }
                            }
                            ui.end_row();
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                if all_ready {
                    ui.label(
                        egui::RichText::new("すべてのモデルが利用可能です。")
                            .color(egui::Color32::from_rgb(80, 200, 80)),
                    );
                }

                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    if !all_ready && !any_downloading {
                        if ui.button("全てダウンロード").clicked() {
                            start_download = true;
                        }
                    }

                    if any_downloading {
                        ui.label(
                            egui::RichText::new("ダウンロード中…")
                                .color(egui::Color32::from_gray(180))
                                .italics(),
                        );
                        // ダウンロード中はリペイントを継続
                        ctx.request_repaint();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("閉じる").clicked() {
                            self.show_ai_model_setup = false;
                        }
                    });
                });

                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "手動配置: %APPDATA%\\mimageviewer\\models\\ に ONNX ファイルを配置"
                    )
                    .small()
                    .color(egui::Color32::from_gray(140)),
                );
            });

        if start_download {
            if let Some(ref mgr) = self.ai_model_manager {
                mgr.start_download_all_missing();
            }
        }

        if !open {
            self.show_ai_model_setup = false;
        }
    }
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.0} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.0} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{} B", bytes)
    }
}
