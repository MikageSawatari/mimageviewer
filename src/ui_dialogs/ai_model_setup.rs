//! AI モデルダウンロードダイアログ（起動時表示）。
//!
//! AI 機能が有効だが必要モデルが不足している場合に起動時に表示する。
//! 機能ごとのチェックボックスで ON/OFF 切り替え可能。
//! OFF にすると設定を保存し、その機能のモデルダウンロードをスキップする。

use crate::ai::ModelKind;
use crate::ai::model_manager::{DownloadState, ModelManager};

/// 機能グループの識別子。
#[derive(Clone, Copy, PartialEq)]
enum FeatureId { Upscale, Denoise, Inpaint }

/// 機能グループの定義。
struct FeatureGroup {
    id: FeatureId,
    label: &'static str,
    enabled: bool,
    models: &'static [ModelKind],
}

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

        let mut start_download = false;
        let mut settings_changed = false;

        // 機能グループ構築
        let mut groups = vec![
            FeatureGroup {
                id: FeatureId::Upscale,
                label: "AI アップスケール",
                enabled: self.settings.ai_upscale_feature,
                models: &[
                    ModelKind::ClassifierMobileNet,
                    ModelKind::UpscaleRealEsrganX4Plus,
                    ModelKind::UpscaleRealEsrganAnime6B,
                    ModelKind::UpscaleRealEsrGeneralV3,
                    ModelKind::UpscaleRealCugan4x,
                ],
            },
            FeatureGroup {
                id: FeatureId::Denoise,
                label: "AI ノイズ除去",
                enabled: self.settings.ai_denoise_feature,
                models: &[
                    ModelKind::DenoiseRealplksr,
                ],
            },
            FeatureGroup {
                id: FeatureId::Inpaint,
                label: "AI 見開き補完",
                enabled: self.settings.ai_inpaint_feature,
                models: &[ModelKind::InpaintMiGan],
            },
        ];

        egui::Window::new("AI モデルのセットアップ")
            .resizable(false)
            .collapsible(false)
            .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
            .show(ctx, |ui| {
                ui.set_min_width(480.0);

                ui.label("AI 機能を使用するにはモデルファイルのダウンロードが必要です。");
                ui.label("使用しない機能のチェックを外すとダウンロードをスキップできます。");
                ui.add_space(8.0);

                let manager = self.ai_model_manager.clone();
                let Some(manager) = manager else {
                    ui.label("AI ランタイムが初期化されていません。");
                    return;
                };

                let mut any_downloading = false;
                let mut any_missing = false;

                for group in &mut groups {
                    ui.add_space(4.0);

                    // 機能チェックボックス
                    let prev_enabled = group.enabled;
                    ui.checkbox(&mut group.enabled, egui::RichText::new(group.label).strong());
                    if group.enabled != prev_enabled {
                        settings_changed = true;
                    }

                    if group.enabled {
                        // モデル一覧（インデント）
                        ui.indent(group.label, |ui| {
                            egui::Grid::new(format!("model_grid_{}", group.label))
                                .num_columns(3)
                                .spacing([12.0, 3.0])
                                .show(ui, |ui| {
                                    for &kind in group.models {
                                        ui.label(kind.display_label());
                                        let state = manager.download_state(kind);
                                        draw_model_state(ui, kind, &state);
                                        match &state {
                                            DownloadState::Downloading { .. } => {
                                                any_downloading = true;
                                            }
                                            DownloadState::Ready(_) => {}
                                            _ => {
                                                any_missing = true;
                                            }
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                    }
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                // ステータス
                if !any_missing && !any_downloading {
                    let enabled_count = groups.iter().filter(|g| g.enabled).count();
                    if enabled_count > 0 {
                        ui.label(
                            egui::RichText::new("すべての必要なモデルが利用可能です。")
                                .color(egui::Color32::from_rgb(80, 200, 80)),
                        );
                    }
                }

                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    if any_missing && !any_downloading {
                        if ui.button("ダウンロード開始").clicked() {
                            start_download = true;
                        }
                    }

                    if any_downloading {
                        ui.label(
                            egui::RichText::new("ダウンロード中…")
                                .color(egui::Color32::from_gray(180))
                                .italics(),
                        );
                        ctx.request_repaint();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close_label = if any_missing || any_downloading { "後で" } else { "閉じる" };
                        if ui.button(close_label).clicked() {
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

        // チェックボックスの変更を settings に反映
        if settings_changed {
            for group in &groups {
                match group.id {
                    FeatureId::Upscale => self.settings.ai_upscale_feature = group.enabled,
                    FeatureId::Denoise => self.settings.ai_denoise_feature = group.enabled,
                    FeatureId::Inpaint => self.settings.ai_inpaint_feature = group.enabled,
                }
            }
            self.settings.save();
        }

        // ダウンロード開始
        if start_download {
            if let Some(ref mgr) = self.ai_model_manager {
                for group in &groups {
                    if group.enabled {
                        mgr.start_download_models(group.models);
                    }
                }
            }
        }

        // 全完了で自動クローズ
        if let Some(ref mgr) = self.ai_model_manager {
            let all_enabled_ready = groups.iter().all(|group| {
                if !group.enabled { return true; }
                group.models.iter().all(|&kind| {
                    matches!(mgr.download_state(kind), DownloadState::Ready(_))
                })
            });
            let any_enabled = groups.iter().any(|g| g.enabled);
            if all_enabled_ready && any_enabled {
                // 全完了 — 少し待ってから自動クローズ（ユーザーに完了を見せる）
                // ただし最初から全て揃っている場合は即閉じ
                self.show_ai_model_setup = false;
            }
        }
    }
}

/// モデル状態を 2 カラム（サイズ + ステータス）で描画する。
fn draw_model_state(ui: &mut egui::Ui, kind: ModelKind, state: &DownloadState) {
    match state {
        DownloadState::Ready(_) => {
            let size = ModelManager::model_size(kind);
            ui.label(format_size(size));
            ui.label(
                egui::RichText::new("✓ 完了")
                    .color(egui::Color32::from_rgb(80, 200, 80)),
            );
        }
        DownloadState::Downloading { progress, total, .. } => {
            let done = progress.load(std::sync::atomic::Ordering::Relaxed);
            let t = total.load(std::sync::atomic::Ordering::Relaxed);
            ui.label(format!("{} / {}", format_size(done), format_size(t)));
            let pct = if t > 0 { done as f32 / t as f32 } else { 0.0 };
            ui.add(
                egui::ProgressBar::new(pct)
                    .desired_width(120.0)
                    .show_percentage(),
            );
        }
        DownloadState::Failed(msg) => {
            let size = ModelManager::model_size(kind);
            ui.label(format_size(size));
            ui.label(
                egui::RichText::new(format!("✗ {msg}"))
                    .color(egui::Color32::from_rgb(220, 80, 80))
                    .small(),
            );
        }
        DownloadState::NotDownloaded => {
            let size = ModelManager::model_size(kind);
            ui.label(format_size(size));
            ui.label(
                egui::RichText::new("—")
                    .color(egui::Color32::from_gray(130)),
            );
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
