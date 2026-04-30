//! VST3 プラグイン管理ウィンドウ。
//!
//! 環境設定の「VST3 プラグイン管理を開く…」ボタンから呼び出される。
//! 機能:
//! - VST3 プラグイン候補のスキャン (`%COMMONPROGRAMFILES%\VST3\` 等を再帰)
//! - 候補一覧 (検索フィルタ付き) からの選択 + 「ロード」ボタン
//! - 現在ロード中のプラグイン情報表示
//! - 「GUI 表示」「GUI 閉じる」「V キー一斉トグル」相当のボタン

#![cfg(windows)]

use crate::app::App;
use crate::video::dsp::DspState;

impl App {
    pub(crate) fn show_vst3_manager(&mut self, ctx: &egui::Context) {
        if !self.show_vst3_manager {
            return;
        }
        let mut open = self.show_vst3_manager;

        // 検索フィルタ用の一時 String (App に持たないので egui_id 経由で永続化)
        let id = egui::Id::new("vst3-manager-filter");
        let mut filter: String = ctx
            .data(|d| d.get_temp::<String>(id))
            .unwrap_or_default();

        let mut clicked_scan = false;
        let mut clicked_load: Option<String> = None;
        let mut clicked_unload = false;
        let mut clicked_show_gui = false;
        let mut clicked_hide_gui = false;
        let mut clicked_toggle_gui = false;

        let bridge = self.dsp_bridge.clone();
        let state = bridge.state();
        let plugin_name = bridge.plugin_name();
        let plugin_path = bridge.plugin_path();
        let latency_samples = bridge.latency_samples();

        let initial_pos = ctx.content_rect().min + egui::vec2(60.0, 60.0);

        egui::Window::new("VST3 プラグイン管理")
            .open(&mut open)
            .default_pos(initial_pos)
            .default_width(640.0)
            .default_height(480.0)
            .resizable(true)
            .show(ctx, |ui| {
                // ── 状態表示 ──
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("状態:").strong());
                    let label = match state {
                        DspState::Disabled => "無効 (環境設定で有効化してください)",
                        DspState::Idle => "待機中 (プラグイン未ロード)",
                        DspState::Loading => "ロード中…",
                        DspState::Loaded => "ロード済み",
                        DspState::Error(e) => return ui.label(format!("エラー: {e}")),
                    };
                    ui.label(label)
                });
                ui.add_space(4.0);

                if matches!(state, DspState::Disabled) {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 160, 60),
                        "VST3 機能は環境設定 → 動画 タブから有効にしてください。",
                    );
                    return;
                }

                // ── ロード中プラグイン情報 ──
                if matches!(state, DspState::Loaded) {
                    ui.group(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "ロード中: {}",
                                plugin_name.as_deref().unwrap_or("(不明)")
                            ))
                            .strong(),
                        );
                        if let Some(p) = &plugin_path {
                            ui.label(egui::RichText::new(p.as_str()).small().weak());
                        }
                        ui.label(format!(
                            "プラグイン latency: {latency_samples} samples (≈{:.1} ms @ 48kHz)",
                            latency_samples as f64 / 48.0
                        ));
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button("GUI 表示").clicked() {
                                clicked_show_gui = true;
                            }
                            if ui.button("GUI 閉じる").clicked() {
                                clicked_hide_gui = true;
                            }
                            if ui
                                .button("GUI トグル (V キー相当)")
                                .on_hover_text(
                                    "メイン画面で V キーを押しても同じ動作をします。",
                                )
                                .clicked()
                            {
                                clicked_toggle_gui = true;
                            }
                            if ui.button("アンロード").clicked() {
                                clicked_unload = true;
                            }
                        });
                    });
                    ui.add_space(8.0);
                }

                ui.separator();
                ui.add_space(6.0);

                // ── プラグイン候補スキャン + フィルタ ──
                ui.horizontal(|ui| {
                    if ui
                        .button(if self.vst3_discovered.is_empty() {
                            "プラグインをスキャン"
                        } else {
                            "再スキャン"
                        })
                        .on_hover_text(
                            "%COMMONPROGRAMFILES%\\VST3\\ と %LOCALAPPDATA%\\Programs\\Common\\VST3\\\n\
                             以下を再帰的に走査して .vst3 を列挙します。",
                        )
                        .clicked()
                    {
                        clicked_scan = true;
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut filter)
                            .hint_text("検索…")
                            .desired_width(200.0),
                    );
                });
                ui.add_space(4.0);

                if self.vst3_discovered.is_empty() {
                    ui.label("「プラグインをスキャン」ボタンを押してください。");
                } else {
                    ui.label(format!(
                        "{} 個のプラグインが見つかりました",
                        self.vst3_discovered.len()
                    ));
                    let filter_lower = filter.to_ascii_lowercase();
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
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
                                ui.horizontal(|ui| {
                                    let is_current = plugin_path
                                        .as_deref()
                                        .map(|p| p == plugin.path.to_string_lossy())
                                        .unwrap_or(false);
                                    let label = if is_current {
                                        format!("● {}", plugin.display_name)
                                    } else {
                                        plugin.display_name.clone()
                                    };
                                    if ui.button(label).clicked() {
                                        clicked_load =
                                            Some(plugin.path.to_string_lossy().to_string());
                                    }
                                    ui.label(
                                        egui::RichText::new(
                                            plugin.path.to_string_lossy().to_string(),
                                        )
                                        .small()
                                        .weak(),
                                    );
                                });
                            }
                        });
                }
            });

        // フィルタを保存
        ctx.data_mut(|d| d.insert_temp(id, filter));

        // クリック処理は Window closure の外で (= self の借用解放後)
        if clicked_scan {
            self.vst3_discovered =
                crate::video::dsp::scan(&crate::video::dsp::default_vst3_paths());
        }
        if let Some(path) = clicked_load {
            // ロードは worker thread で (bridge 応答待ちで UI を止めない)。
            // 起動後の sample_rate / block_size は cpal 既定 (= 48kHz, 480 frames) を使う。
            let bridge_clone = bridge.clone();
            let restore = self
                .settings
                .vst3_plugin_state
                .as_ref()
                .and_then(|b64| {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD.decode(b64).ok()
                });
            self.settings.vst3_plugin_path = Some(path.clone());
            self.settings.save();
            std::thread::Builder::new()
                .name("vst3-load".into())
                .spawn(move || {
                    let sample_rate = crate::video::audio::default_output_sample_rate()
                        .unwrap_or(48_000);
                    if let Err(e) = bridge_clone.load_plugin(
                        &path,
                        sample_rate,
                        480,
                        restore.as_deref(),
                    ) {
                        crate::logger::log(format!("vst3 load_plugin failed: {e}"));
                    }
                })
                .ok();
        }
        if clicked_unload {
            // 単体 close は v0.9.0 では bridge 側を再 spawn する形で簡略実装。
            // = disable + enable (= プラグインアンロード相当)。
            self.dsp_bridge.disable();
            let bridge_clone = self.dsp_bridge.clone();
            std::thread::Builder::new()
                .name("vst3-reenable".into())
                .spawn(move || {
                    if let Err(e) = bridge_clone.enable() {
                        crate::logger::log(format!("vst3 re-enable failed: {e}"));
                    }
                })
                .ok();
            self.settings.vst3_plugin_path = None;
            self.settings.save();
        }
        if clicked_show_gui {
            self.vst3_show_plugin_gui();
        }
        if clicked_hide_gui {
            self.vst3_hide_plugin_gui();
        }
        if clicked_toggle_gui {
            self.vst3_toggle_plugin_gui();
        }

        self.show_vst3_manager = open;
    }
}
