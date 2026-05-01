//! VST3 プラグイン再生時パネル (= 動画再生中の ON/OFF + GUI トグル UI)。
//!
//! v0.9.0 の途中までは「管理ウィンドウ」として add/remove/reorder/scan の全機能
//! を抱えていたが、ユーザーフィードバックで以下の方針に変更:
//!
//! - **編集 (= 追加・削除・並べ替え) は環境設定→動画タブのチェーンエディタに移動**。
//!   プラグインの選択は腰を据えた作業 (= 起動時に決める) なので環境設定で OK。
//! - **再生中はこのパネル**で、設定済みプラグインの **バイパス ON/OFF + GUI 表示**
//!   だけを高速にトグルする。
//! - 配色は **黒ベースのフレーム** にして、フルスクリーン動画 (黒背景) との違和感
//!   を抑える (egui::Window のデフォルト白フレームだと再生中目立つ問題への対応)。
//!
//! ## レイアウト
//!
//! ```text
//! ┌─────────────────────────────┐
//! │ VST3                    ×  │ ← タイトルバー (黒)
//! ├─────────────────────────────┤
//! │ 動画:  [フル][右上 1/4]      │
//! │ ──────────────────          │
//! │ ☑ 1. Pro-Q 4    [GUI]       │
//! │ ☐ 2. Insight 2  [GUI]       │
//! │ ──────────────────          │
//! │ チェーン編集は 環境設定→動画 │
//! └─────────────────────────────┘
//! ```
//!
//! `☑` = ON (= bypass=false)、`☐` = OFF (= bypass=true)。
//! `[GUI]` ボタンクリックで個別プラグイン GUI を表示・非表示。

#![cfg(windows)]

use crate::app::App;
use crate::video::dsp::{DspState, SlotState};

impl App {
    pub(crate) fn show_vst3_manager(&mut self, ctx: &egui::Context) {
        if !self.show_vst3_manager {
            return;
        }
        let mut open = self.show_vst3_manager;

        // ── ボタンクリック処理は closure 外で行うため、closure 内ではフラグだけ立てる ──
        // **plugin_path も同時に取る**: bridge slots の idx と `settings.vst3_plugins` の
        // idx は **ロード失敗で詰まるとズレる** (Codex P2 2026-05-01)。bridge を idx で
        // 操作しつつ、settings を path で引くために両方持つ。
        let mut clicked_show_gui: Option<(usize, String)> = None;
        let mut clicked_hide_gui: Option<(usize, String)> = None;
        let mut clicked_toggle_bypass: Option<(usize, String, bool)> = None;

        let bridge = self.dsp_bridge.clone();
        let state = bridge.state();
        let slots = bridge.slots();

        // 動画サイズ切替の初期値
        let video_compact = self.settings.vst3_video_compact;
        let mut clicked_video_size: Option<bool> = None;

        // 動画再生中の使い勝手を考慮した小さめ初期サイズ。チェーン編集を含まない
        // ので幅は狭くて済む (= プラグイン名 + バイパス + GUI ボタンが収まれば OK)。
        // **位置は固定 ID で永続化** (= 旧版で RichText タイトルを使ったため Window の
        // 内部 ID がフレームごとに変動し、ドラッグ位置が記憶されず 1 フレームごとに
        // ずれてしまうユーザー報告に対応)。
        let initial_pos = ctx.content_rect().min + egui::vec2(60.0, 60.0);

        // ── タイトルバー含めた全体の dark Frame ──
        // egui::Window のタイトルバーは ctx.style() の visuals を使うため、
        // .show(ctx, |ui| ui.style_mut().visuals = dark()) では **タイトルバーだけ
        // light のまま** になる (= "再表示すると白背景" の根本原因)。
        // .frame(custom_frame) でウィンドウ全体の outer Frame を黒で塗り、その上に
        // 内側の widget も dark visuals で描画することで、再表示しても黒背景で安定。
        let bg = egui::Color32::from_rgba_unmultiplied(20, 20, 20, 245);
        let stroke = egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(120, 120, 120, 200),
        );
        let frame = egui::Frame::window(&ctx.style())
            .fill(bg)
            .stroke(stroke);

        egui::Window::new("VST3")
            .id(egui::Id::new("vst3-playback-panel"))
            .frame(frame)
            .open(&mut open)
            .default_pos(initial_pos)
            .default_width(280.0)
            .min_width(220.0)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                // ── 内側 widget の visuals を dark に ──
                // (タイトルバーの色は frame() で固定済。ここは widget 群の bg/fg)
                let style = ui.style_mut();
                style.visuals = egui::Visuals::dark();
                style.visuals.window_fill = bg;
                style.visuals.panel_fill = bg;
                style.visuals.window_stroke = stroke;
                // ボタン地色を少し明るめにして可読性を確保 (= 黒背景に薄灰ボタン)
                style.visuals.widgets.inactive.weak_bg_fill =
                    egui::Color32::from_rgb(50, 50, 50);
                style.visuals.widgets.inactive.bg_fill =
                    egui::Color32::from_rgb(60, 60, 60);
                style.visuals.widgets.hovered.weak_bg_fill =
                    egui::Color32::from_rgb(80, 80, 80);
                style.visuals.widgets.hovered.bg_fill =
                    egui::Color32::from_rgb(90, 90, 90);
                style.visuals.widgets.active.weak_bg_fill =
                    egui::Color32::from_rgb(100, 100, 100);
                style.visuals.widgets.active.bg_fill =
                    egui::Color32::from_rgb(110, 110, 110);
                // テキストは全状態で白系
                style.visuals.widgets.inactive.fg_stroke.color =
                    egui::Color32::from_rgb(230, 230, 230);
                style.visuals.widgets.hovered.fg_stroke.color = egui::Color32::WHITE;
                style.visuals.widgets.active.fg_stroke.color = egui::Color32::WHITE;
                style.visuals.widgets.noninteractive.fg_stroke.color =
                    egui::Color32::from_rgb(220, 220, 220);

            if matches!(state, DspState::Error(_)) {
                if let DspState::Error(e) = state {
                    ui.colored_label(
                        egui::Color32::from_rgb(240, 130, 130),
                        format!("エラー: {e}"),
                    );
                }
                return;
            }
            if matches!(state, DspState::Disabled) {
                ui.colored_label(
                    egui::Color32::from_rgb(230, 180, 80),
                    "VST3 機能は環境設定→動画 タブから\n有効にしてください。",
                );
                return;
            }

            // ── 動画表示サイズ ──
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
                        "動画を右上 1/4 に縮小し、左下 3/4 をプラグイン GUI 用に空ける。",
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

            // ── プラグイン一覧 (ON/OFF + GUI のみ) ──
            if slots.is_empty() {
                ui.label(
                    egui::RichText::new("プラグイン未設定")
                        .color(egui::Color32::from_rgb(190, 190, 190)),
                );
                ui.label(
                    egui::RichText::new(
                        "環境設定→動画 タブで\nプラグインをチェーンに追加してください。",
                    )
                    .small()
                    .color(egui::Color32::from_rgb(170, 170, 170)),
                );
            } else {
                let sample_rate = self.dsp_bridge.sample_rate();
                for (idx, slot) in slots.iter().enumerate() {
                    ui.horizontal(|ui| {
                        // ON/OFF: bypass を反転して「ON = 効いている」表示にする
                        let mut on = !slot.bypass;
                        let label = format!(
                            "{}. {}{}",
                            idx + 1,
                            slot.plugin_name.as_deref().unwrap_or("(不明)"),
                            match slot.state {
                                SlotState::Loading => " (…)",
                                SlotState::Loaded => "",
                                SlotState::Error => " (エラー)",
                            },
                        );
                        if ui
                            .checkbox(&mut on, label)
                            .on_hover_text(
                                "ON: このプラグインを通して音声を処理。\n\
                                 OFF: パススルー (= バイパス)。",
                            )
                            .changed()
                        {
                            // on=true → bypass=false に
                            clicked_toggle_bypass =
                                Some((idx, slot.plugin_path.clone(), !on));
                        }
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if slot.gui_visible {
                                    if ui
                                        .small_button("GUI ×")
                                        .on_hover_text("プラグイン GUI を閉じる")
                                        .clicked()
                                    {
                                        clicked_hide_gui =
                                            Some((idx, slot.plugin_path.clone()));
                                    }
                                } else if ui
                                    .small_button("GUI")
                                    .on_hover_text("プラグイン GUI を表示")
                                    .clicked()
                                {
                                    clicked_show_gui =
                                        Some((idx, slot.plugin_path.clone()));
                                }
                                // ── latency 表示 (= プラグインが報告した遅延) ──
                                // bypass=true や Loaded 以外なら表示しない (= 影響しない)
                                if !slot.bypass
                                    && matches!(slot.state, SlotState::Loaded)
                                    && slot.latency_samples > 0
                                {
                                    let ms_text = if sample_rate > 0 {
                                        format!(
                                            "{:.1}ms",
                                            slot.latency_samples as f64 / sample_rate as f64
                                                * 1000.0
                                        )
                                    } else {
                                        format!("{}sm", slot.latency_samples)
                                    };
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(ms_text)
                                            .small()
                                            .color(egui::Color32::from_rgb(255, 200, 100)),
                                    )
                                    .on_hover_text(format!(
                                        "プラグインが報告したレイテンシ\n\
                                         {} samples @ {}Hz\n\
                                         (PDC で動画クロックを後ろにずらして同期補正済み)",
                                        slot.latency_samples,
                                        if sample_rate > 0 { sample_rate } else { 48000 },
                                    ));
                                }
                                // ── 自動 OFF バッジ (= 上限超過で auto-bypass) ──
                                // bypass=true && auto_bypassed_for_latency=true の組み合わせで判定。
                                // ユーザーが手動で再 ON にすると set_bypass で auto フラグ解除される。
                                if slot.auto_bypassed_for_latency && slot.bypass {
                                    let latency_ms = if sample_rate > 0 {
                                        slot.latency_samples as f64 / sample_rate as f64 * 1000.0
                                    } else {
                                        0.0
                                    };
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new("[!] auto-OFF")
                                            .small()
                                            .strong()
                                            .background_color(egui::Color32::from_rgb(180, 30, 30))
                                            .color(egui::Color32::WHITE),
                                    )
                                    .on_hover_text(format!(
                                        "レイテンシが上限 (2.0 秒) を超えたため自動 OFF\n\
                                         検出値: {:.1}ms\n\
                                         プラグイン側で遅延を減らしてから手動で再 ON してください。",
                                        latency_ms,
                                    ));
                                }
                            },
                        );
                    });
                }
            }

            ui.separator();
            ui.label(
                egui::RichText::new(
                    "プラグインの追加・並べ替えは 環境設定→VST3 プラグイン から行います。",
                )
                .small()
                .color(egui::Color32::from_rgb(160, 160, 160)),
            );
        });

        // ── ボタンクリック処理 (Window closure 外で実行 = self の借用解放後) ──
        // bridge への命令は **bridge slot idx**、settings 検索は **plugin_path** で行う。
        if let Some((idx, path)) = clicked_show_gui {
            if let Err(e) = self.dsp_bridge.show_slot_gui(idx) {
                crate::logger::log(format!("vst3 show_slot_gui: {e}"));
            } else {
                // 明示的な show は user_hidden 解除も意味する (= 起動時 settings から
                // 復元した user_hidden=true を、ユーザーが「GUI」ボタンで上書き)。
                let mut changed = !self.settings.vst3_gui_visible;
                self.settings.vst3_gui_visible = true;
                if let Some(entry) = self.find_vst3_entry_mut(&path) {
                    if entry.user_hidden {
                        entry.user_hidden = false;
                        changed = true;
                    }
                }
                if changed {
                    self.settings.save();
                }
            }
        }
        if let Some((idx, path)) = clicked_hide_gui {
            // ユーザーが個別に GUI × した → user_hidden=true をセット
            // (= 以降の VST 全表示でも skip される、再起動後も維持)
            self.dsp_bridge.user_hide_slot_gui(idx);
            if let Some(entry) = self.find_vst3_entry_mut(&path) {
                if !entry.user_hidden {
                    entry.user_hidden = true;
                    self.settings.save();
                }
            }
        }
        if let Some((idx, path, bypass)) = clicked_toggle_bypass {
            self.dsp_bridge.set_bypass(idx, bypass);
            // settings 側も同期 (= 永続化、次回起動時にこの bypass で復元される)
            if let Some(entry) = self.find_vst3_entry_mut(&path) {
                entry.bypass = bypass;
                self.settings.save();
            }
        }
        if let Some(compact) = clicked_video_size {
            self.settings.vst3_video_compact = compact;
            self.settings.save();
        }

        self.show_vst3_manager = open;
    }
}
