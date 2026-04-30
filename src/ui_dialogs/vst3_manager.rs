//! VST3 プラグイン管理ウィンドウ (チェーンエディタ)。
//!
//! 環境設定の「VST3 プラグイン管理を開く…」やメニューから呼び出される。
//! 機能:
//! - VST3 プラグイン候補のスキャン (`%COMMONPROGRAMFILES%\VST3\` 等を再帰)
//! - **チェーン**: 現在ロード中のプラグインを順序付きで表示。
//!   各エントリの操作: 上へ/下へ / バイパス / GUI 表示・非表示 / 削除
//! - 候補一覧から「チェーンに追加」してプラグインをチェーン末尾にロード
//! - チェーン全体は順番に音声を通す (= バス上のラックのイメージ)

#![cfg(windows)]

use crate::app::App;
use crate::settings::Vst3PluginEntry;
use crate::video::dsp::{DspState, SlotState};

impl App {
    pub(crate) fn show_vst3_manager(&mut self, ctx: &egui::Context) {
        if !self.show_vst3_manager {
            return;
        }
        let mut open = self.show_vst3_manager;

        // 検索フィルタ用の一時 String
        let id = egui::Id::new("vst3-manager-filter");
        let mut filter: String = ctx
            .data(|d| d.get_temp::<String>(id))
            .unwrap_or_default();

        // ── ボタンクリック処理は closure 外で行うため、closure 内ではフラグだけ立てる ──
        let mut clicked_scan = false;
        // チェーン操作用フラグ (idx: usize、複数 idx を一度のフレームで扱うのは UX 上ない)
        let mut clicked_show_gui: Option<usize> = None;
        let mut clicked_hide_gui: Option<usize> = None;
        let mut clicked_remove: Option<usize> = None;
        let mut clicked_move_up: Option<usize> = None;
        let mut clicked_move_down: Option<usize> = None;
        let mut clicked_toggle_bypass: Option<(usize, bool)> = None;
        let mut clicked_add: Option<String> = None;

        let bridge = self.dsp_bridge.clone();
        let state = bridge.state();
        let slots = bridge.slots();

        // 動画サイズ切替の初期値を読み取る (動画パネルのトグルで変化する)
        let video_compact = self.settings.vst3_video_compact;
        let mut clicked_video_size: Option<bool> = None;

        // 旧コミット既定 720x540 はサイズが大きすぎる。チェーン編集と簡易スキャン
        // に必要な最小限まで絞り、ユーザーがリサイズで広げられる構成にする。
        let initial_pos = ctx.content_rect().min + egui::vec2(60.0, 60.0);

        egui::Window::new("VST3 プラグイン管理")
            .open(&mut open)
            .default_pos(initial_pos)
            .default_width(360.0)
            .default_height(380.0)
            .min_width(280.0)
            .min_height(220.0)
            .resizable(true)
            .show(ctx, |ui| {
                // ── 状態 + 動画サイズ切替 (1 行に集約) ──
                if matches!(state, DspState::Error(_)) {
                    if let DspState::Error(e) = state {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 90, 90),
                            format!("エラー: {e}"),
                        );
                    }
                    return;
                }
                if matches!(state, DspState::Disabled) {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 160, 60),
                        "VST3 機能は環境設定 → 動画 タブから有効にしてください。",
                    );
                    return;
                }

                // ── 動画表示サイズトグル ──
                // プラグイン GUI のためにスペースを空けたいユーザー向け。
                // フル / 右上 1/4 の 2 段階。
                ui.horizontal(|ui| {
                    ui.label("動画:");
                    let mut new_compact = video_compact;
                    if ui
                        .selectable_label(!new_compact, "フル")
                        .on_hover_text("動画をフルスクリーン全体に表示する (= 既定)")
                        .clicked()
                    {
                        new_compact = false;
                    }
                    if ui
                        .selectable_label(new_compact, "右上 1/4")
                        .on_hover_text(
                            "動画を右上 1/4 に縮小し、左下 3/4 をプラグイン GUI 用に空ける。\n\
                             プラグイン分析時の作業領域として使う。",
                        )
                        .clicked()
                    {
                        new_compact = true;
                    }
                    if new_compact != video_compact {
                        clicked_video_size = Some(new_compact);
                    }
                });
                ui.separator();

                // ── プラグインチェーン (= 現在ロード中のスロット一覧) ──
                ui.label(
                    egui::RichText::new(format!("チェーン ({} 個)", slots.len())).strong(),
                );

                if slots.is_empty() {
                    ui.label(egui::RichText::new("(空)").weak());
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("vst3-chain-scroll")
                        .max_height(140.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for (idx, slot) in slots.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    // 番号
                                    ui.label(
                                        egui::RichText::new(format!("{}.", idx + 1))
                                            .weak(),
                                    );

                                    // プラグイン名 (state を suffix)
                                    let name = slot.plugin_name.as_deref().unwrap_or("(不明)");
                                    let state_suffix = match slot.state {
                                        SlotState::Loading => " (…)",
                                        SlotState::Loaded => "",
                                        SlotState::Error => " (エラー)",
                                    };
                                    ui.label(
                                        egui::RichText::new(format!("{name}{state_suffix}"))
                                            .strong(),
                                    )
                                    .on_hover_text(slot.plugin_path.as_str());

                                    // 右寄せでアクションボタン
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .small_button("✕")
                                                .on_hover_text("チェーンから削除")
                                                .clicked()
                                            {
                                                clicked_remove = Some(idx);
                                            }
                                            let down_enabled = idx + 1 < slots.len();
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
                                            if slot.gui_hwnd != 0 {
                                                if ui
                                                    .small_button("GUI ✕")
                                                    .on_hover_text("プラグイン GUI を閉じる")
                                                    .clicked()
                                                {
                                                    clicked_hide_gui = Some(idx);
                                                }
                                            } else if ui
                                                .small_button("GUI")
                                                .on_hover_text("プラグイン GUI を表示")
                                                .clicked()
                                            {
                                                clicked_show_gui = Some(idx);
                                            }
                                            let mut bypass = slot.bypass;
                                            if ui
                                                .checkbox(&mut bypass, "バイパス")
                                                .on_hover_text(
                                                    "ON: このスロットをスキップ (= 音声をパススルー)",
                                                )
                                                .changed()
                                            {
                                                clicked_toggle_bypass = Some((idx, bypass));
                                            }
                                        },
                                    );
                                });
                            }
                        });
                }

                ui.separator();

                // ── プラグイン追加 (検索 + 候補リスト) ──
                ui.horizontal(|ui| {
                    if ui
                        .small_button(if self.vst3_discovered.is_empty() {
                            "スキャン"
                        } else {
                            "再スキャン"
                        })
                        .on_hover_text(
                            "%COMMONPROGRAMFILES%\\VST3\\ 等を再帰走査して .vst3 を列挙",
                        )
                        .clicked()
                    {
                        clicked_scan = true;
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut filter)
                            .hint_text("検索…")
                            .desired_width(f32::INFINITY),
                    );
                });

                if self.vst3_discovered.is_empty() {
                    ui.label(egui::RichText::new("(スキャン未実行)").weak().small());
                } else {
                    let filter_lower = filter.to_ascii_lowercase();
                    egui::ScrollArea::vertical()
                        .id_salt("vst3-discovered-scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for plugin in &self.vst3_discovered {
                                if !filter_lower.is_empty()
                                    && !plugin
                                        .display_name
                                        .to_ascii_lowercase()
                                        .contains(&filter_lower)
                                {
                                    continue;
                                }
                                if ui
                                    .add(
                                        egui::Button::new(&plugin.display_name)
                                            .min_size(egui::vec2(
                                                ui.available_width(),
                                                0.0,
                                            )),
                                    )
                                    .on_hover_text(plugin.path.to_string_lossy())
                                    .clicked()
                                {
                                    clicked_add =
                                        Some(plugin.path.to_string_lossy().to_string());
                                }
                            }
                        });
                }
            });

        // フィルタを保存
        ctx.data_mut(|d| d.insert_temp(id, filter));

        // ── ボタンクリック処理 (Window closure 外で実行 = self の借用解放後) ──
        if clicked_scan {
            self.vst3_discovered =
                crate::video::dsp::scan(&crate::video::dsp::default_vst3_paths());
        }

        if let Some(idx) = clicked_show_gui {
            // メインスレッド同期: ms オーダーなので OK
            if let Err(e) = self.dsp_bridge.show_slot_gui(idx) {
                crate::logger::log(format!("vst3 show_slot_gui: {e}"));
            } else {
                self.settings.vst3_gui_visible = true;
            }
        }
        if let Some(idx) = clicked_hide_gui {
            self.dsp_bridge.hide_slot_gui(idx);
        }
        if let Some((idx, bypass)) = clicked_toggle_bypass {
            self.dsp_bridge.set_bypass(idx, bypass);
            // settings 側も同期
            if let Some(entry) = self.settings.vst3_plugins.get_mut(idx) {
                entry.bypass = bypass;
                self.settings.save();
            }
        }
        if let Some(idx) = clicked_remove {
            self.dsp_bridge.remove_plugin(idx);
            if idx < self.settings.vst3_plugins.len() {
                self.settings.vst3_plugins.remove(idx);
                self.settings.save();
            }
        }
        if let Some(idx) = clicked_move_up {
            if idx > 0 {
                self.dsp_bridge.move_plugin(idx, idx - 1);
                let v = &mut self.settings.vst3_plugins;
                if idx < v.len() {
                    v.swap(idx, idx - 1);
                    self.settings.save();
                }
            }
        }
        if let Some(idx) = clicked_move_down {
            if idx + 1 < slots.len() {
                self.dsp_bridge.move_plugin(idx, idx + 1);
                let v = &mut self.settings.vst3_plugins;
                if idx + 1 < v.len() {
                    v.swap(idx, idx + 1);
                    self.settings.save();
                }
            }
        }
        if let Some(compact) = clicked_video_size {
            self.settings.vst3_video_compact = compact;
            self.settings.save();
        }
        if let Some(path) = clicked_add {
            // settings 側に先に登録してから worker thread で実ロード
            self.settings.vst3_plugins.push(Vst3PluginEntry {
                path: path.clone(),
                bypass: false,
                state: None,
            });
            self.settings.save();
            let bridge_clone = self.dsp_bridge.clone();
            std::thread::Builder::new()
                .name("vst3-add-plugin".into())
                .spawn(move || {
                    let sample_rate = crate::video::audio::default_output_sample_rate()
                        .unwrap_or(48_000);
                    if let Err(e) =
                        bridge_clone.add_plugin(&path, sample_rate, 480, false)
                    {
                        crate::logger::log(format!("vst3 add_plugin failed: {e}"));
                    }
                })
                .ok();
        }

        self.show_vst3_manager = open;
    }
}
