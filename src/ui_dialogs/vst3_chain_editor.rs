//! VST3 プラグインチェーン編集ダイアログ (= 独立ダイアログ)。
//!
//! プレイバックパネル (= `vst3_manager.rs`) の「編集…」ボタンから開く。
//! 機能はチェーン編集に専念:
//! - スキャン (= `%COMMONPROGRAMFILES%\VST3\` 等を再帰)
//! - 候補から **チェーン末尾に追加** (上限 10 個、既追加は除外、検索フィルタ)
//! - 並べ替え (↑↓) と削除 (×)
//!
//! 環境設定→動画タブからは独立 (= ユーザー要望「環境設定だとわかりにくい」)。
//!
//! ## 反映
//!
//! ダイアログ操作で `settings.vst3_plugins` が直接変更される (= 即時保存)。
//! bridge のチェーン状態とのリコンサイル (= プラグインのロード/アンロード)
//! は **ダイアログを閉じる時** にまとめて行う:
//!
//! - 旧チェーン (= bridge.slots) と新チェーン (= settings.vst3_plugins) を
//!   path 順序で比較
//! - 違っていれば bridge を nuke + 再ロード (= 順序・追加・削除を反映)
//!
//! 即時リコンサイルしない理由: ユーザーが連続で並べ替え・削除を行う最中に
//! 都度 bridge を再ロードすると音声が頻繁に途切れるため。

#![cfg(windows)]

use crate::app::App;
use crate::settings::Vst3PluginEntry;
use crate::video::dsp;

const MAX_CHAIN_LEN: usize = 10;

impl App {
    pub(crate) fn show_vst3_chain_editor_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_vst3_chain_editor {
            return;
        }
        let mut open = self.show_vst3_chain_editor;
        let initial_pos = ctx.content_rect().min + egui::vec2(80.0, 80.0);

        // 編集の操作フラグ (Window closure 外で実行 = self の借用解放後)
        let mut clicked_remove: Option<usize> = None;
        let mut clicked_move_up: Option<usize> = None;
        let mut clicked_move_down: Option<usize> = None;
        let mut clicked_scan = false;
        let mut clicked_add: Option<String> = None;
        let mut clicked_close = false;

        let chain_snapshot = self.settings.vst3_plugins.clone();
        let chain_len = chain_snapshot.len();

        egui::Window::new("VST3 プラグインチェーン編集")
            .id(egui::Id::new("vst3-chain-editor-window"))
            .open(&mut open)
            .default_pos(initial_pos)
            .default_width(560.0)
            .default_height(500.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "現在のチェーン ({chain_len}/{MAX_CHAIN_LEN} 個)"
                    ))
                    .strong(),
                );
                ui.label(
                    egui::RichText::new(
                        "上から順に音声を通します。動画再生中はホバーバーの VST ボタンから\n\
                         ON/OFF (バイパス) を切り替えできます。",
                    )
                    .small()
                    .weak(),
                );
                ui.add_space(4.0);

                // ── チェーン一覧 ──
                if chain_snapshot.is_empty() {
                    ui.label(
                        egui::RichText::new("(空)")
                            .weak(),
                    );
                } else {
                    for (idx, entry) in chain_snapshot.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(format!("{}.", idx + 1)).weak());
                            let name = std::path::Path::new(&entry.path)
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("(不明)");
                            ui.label(egui::RichText::new(name).strong())
                                .on_hover_text(entry.path.as_str());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .small_button("×")
                                        .on_hover_text("チェーンから削除")
                                        .clicked()
                                    {
                                        clicked_remove = Some(idx);
                                    }
                                    let down_enabled = idx + 1 < chain_snapshot.len();
                                    if ui
                                        .add_enabled(
                                            down_enabled,
                                            egui::Button::new("↓").small(),
                                        )
                                        .on_hover_text("下へ")
                                        .clicked()
                                    {
                                        clicked_move_down = Some(idx);
                                    }
                                    let up_enabled = idx > 0;
                                    if ui
                                        .add_enabled(
                                            up_enabled,
                                            egui::Button::new("↑").small(),
                                        )
                                        .on_hover_text("上へ")
                                        .clicked()
                                    {
                                        clicked_move_up = Some(idx);
                                    }
                                },
                            );
                        });
                    }
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                // ── プラグイン追加 ──
                let chain_full = chain_snapshot.len() >= MAX_CHAIN_LEN;
                ui.horizontal(|ui| {
                    let scan_label = if self.vst3_discovered.is_empty() {
                        "プラグインをスキャン"
                    } else {
                        "再スキャン"
                    };
                    if ui
                        .button(scan_label)
                        .on_hover_text(
                            "%COMMONPROGRAMFILES%\\VST3\\ 等を再帰走査して .vst3 を列挙",
                        )
                        .clicked()
                    {
                        clicked_scan = true;
                    }
                    if !self.vst3_discovered.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "({} 個検出)",
                                self.vst3_discovered.len()
                            ))
                            .weak()
                            .small(),
                        );
                    }
                    ui.add_space(8.0);
                    if chain_full {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 160, 60),
                            "上限 (10 個) に達しています",
                        );
                    }
                });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("検索:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.vst3_chain_editor_filter)
                            .hint_text("プラグイン名…")
                            .desired_width(f32::INFINITY),
                    );
                });
                ui.add_space(2.0);

                if self.vst3_discovered.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "上の「プラグインをスキャン」ボタンを押してください。",
                        )
                        .weak()
                        .small(),
                    );
                } else {
                    let filter_lower = self.vst3_chain_editor_filter.to_ascii_lowercase();
                    let existing: std::collections::HashSet<String> = chain_snapshot
                        .iter()
                        .map(|e| e.path.clone())
                        .collect();
                    egui::ScrollArea::vertical()
                        .id_salt("vst3-chain-editor-picker-scroll")
                        .max_height(220.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for plugin in &self.vst3_discovered {
                                let path_s = plugin.path.to_string_lossy().to_string();
                                let already_in_chain = existing.contains(&path_s);
                                if !filter_lower.is_empty()
                                    && !plugin
                                        .display_name
                                        .to_ascii_lowercase()
                                        .contains(&filter_lower)
                                {
                                    continue;
                                }
                                let label = if already_in_chain {
                                    format!("{}  (追加済み)", plugin.display_name)
                                } else {
                                    plugin.display_name.clone()
                                };
                                let enabled = !already_in_chain && !chain_full;
                                let resp = ui.add_enabled(
                                    enabled,
                                    egui::Button::new(label),
                                );
                                let resp = resp.on_hover_text(&path_s);
                                if resp.clicked() {
                                    clicked_add = Some(path_s);
                                }
                            }
                        });
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if ui.button("閉じる").clicked() {
                                clicked_close = true;
                            }
                        },
                    );
                });
            });

        // ── 操作の反映 (closure 外で self を mut 借用) ──
        let mut chain_changed = false;
        if let Some(idx) = clicked_remove {
            self.settings.vst3_plugins.remove(idx);
            chain_changed = true;
        }
        if let Some(idx) = clicked_move_up {
            if idx > 0 {
                self.settings.vst3_plugins.swap(idx, idx - 1);
                chain_changed = true;
            }
        }
        if let Some(idx) = clicked_move_down {
            if idx + 1 < self.settings.vst3_plugins.len() {
                self.settings.vst3_plugins.swap(idx, idx + 1);
                chain_changed = true;
            }
        }
        if let Some(path) = clicked_add {
            if self.settings.vst3_plugins.len() < MAX_CHAIN_LEN
                && !self.settings.vst3_plugins.iter().any(|e| e.path == path)
            {
                self.settings.vst3_plugins.push(Vst3PluginEntry {
                    path,
                    bypass: false,
                    state: None,
                });
                chain_changed = true;
            }
        }
        if clicked_scan {
            self.vst3_discovered = dsp::scan(&dsp::default_vst3_paths());
        }
        if chain_changed {
            self.settings.save();
            // チェーンが変わったので bridge も nuke + 再ロード (= 順序・追加・削除反映)。
            // worker thread で実施 (UI スレッドをブロックしない)。
            self.kick_off_vst3_chain_rebuild();
        }
        if clicked_close {
            open = false;
        }
        self.show_vst3_chain_editor = open;
    }

    /// `settings.vst3_plugins` を bridge に再反映する。worker thread で実行。
    /// チェーンエディタからの編集や、環境設定からの enable 状態変化で使う。
    pub(crate) fn kick_off_vst3_chain_rebuild(&self) {
        if !self.settings.vst3_enabled {
            return;
        }
        let bridge = self.dsp_bridge.clone();
        let plugins = self.settings.vst3_plugins.clone();
        std::thread::Builder::new()
            .name("vst3-chain-rebuild".into())
            .spawn(move || {
                bridge.disable();
                if let Err(e) = bridge.enable() {
                    crate::logger::log(format!("vst3 chain-rebuild enable: {e}"));
                    return;
                }
                let sample_rate =
                    crate::video::audio::default_output_sample_rate().unwrap_or(48_000);
                for entry in plugins {
                    if let Err(e) =
                        bridge.add_plugin(&entry.path, sample_rate, 480, entry.bypass)
                    {
                        crate::logger::log(format!(
                            "vst3 chain-rebuild add_plugin {} failed: {e}",
                            entry.path
                        ));
                    }
                }
            })
            .ok();
    }
}
