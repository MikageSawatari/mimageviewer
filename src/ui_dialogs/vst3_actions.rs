//! VST3 プラグイン GUI のホスト側操作 (App メソッド)。
//!
//! 実装の本体は [`crate::video::dsp::DspBridge`] にある。ここはそれを App から
//! 呼ぶための薄いラッパー。

#![cfg(windows)]

use crate::app::App;
use crate::settings::{Vst3ChainPresetSlot, Vst3PluginEntry};

impl App {
    /// `settings.vst3_plugins` を **plugin_path** で検索する共通 helper。
    /// bridge slot idx と settings idx は load 失敗で詰まるとズレるため
    /// (Codex P2、2026-05-01)、path をキーに entry を引く流儀に統一する。
    /// path は preferences 側で重複追加を弾いているので一意。
    pub(crate) fn find_vst3_entry_mut(&mut self, path: &str) -> Option<&mut Vst3PluginEntry> {
        self.settings
            .vst3_plugins
            .iter_mut()
            .find(|e| e.path == path)
    }

    /// 全 Loaded プラグインの内部状態 (= EQ カーブ / chunk) を bridge から取得して、
    /// `settings.vst3_plugins` の対応する entry の `state` フィールドに **path 一致**
    /// で書き込む。**呼び出し側で `self.settings.save()` を別途呼ぶこと**。
    /// 戻り値: 更新された entry 数 (= 0 なら save 不要の判断材料に使える)。
    ///
    /// **並列で走る**: bridge ごとに別スレッドを spawn して `query_state_sync` を
    /// 同時実行する (1s timeout)。チェーン上限 10 個で worst case ~1 秒、
    /// 典型は数十 ms。on_exit / chain rebuild など save 直前のフックで 1 回だけ
    /// 呼ぶ想定 (= UI hot-path には乗らない)。
    ///
    /// **Guard は bridge.is_enabled() を見る**: preferences で OFF にトグルした直後は
    /// `self.settings.vst3_enabled == false` だが、bridge 側 plugin はまだ生きている。
    /// この経路でも snapshot を取りたい (= teardown 前) ので、settings ではなく bridge の
    /// runtime 状態をガードに使う (Codex P2-1、2026-05-01)。
    pub(crate) fn snapshot_vst3_states_into_settings(&mut self) -> usize {
        if !self.dsp_bridge.is_enabled() {
            return 0;
        }
        let snapshots = self.dsp_bridge.snapshot_all_plugin_states();
        let mut updated = 0;
        for (path, state) in snapshots {
            if let Some(entry) = self.find_vst3_entry_mut(&path) {
                let new_state = Some(state);
                if entry.state != new_state {
                    entry.state = new_state;
                    updated += 1;
                }
            }
        }
        updated
    }

    /// 全 GUI 表示済みプラグインのウィンドウ位置 + 外枠サイズを取得して
    /// `settings.vst3_plugins[*].gui_pos / gui_size` に path 一致で書き込む
    /// (= 2026-05 ユーザー要望「ウィンドウ位置を復元してほしい」)。
    /// 戻り値: 更新された entry 数。`settings.save()` は呼び出し側で。
    /// `GetWindowRect` を順次呼ぶだけなので軽量 (~us)、UI スレッドから OK。
    pub(crate) fn snapshot_vst3_window_positions_into_settings(&mut self) -> usize {
        if !self.dsp_bridge.is_enabled() {
            return 0;
        }
        let snapshots = self.dsp_bridge.snapshot_all_window_positions();
        let mut updated = 0;
        for (path, x, y, w, h) in snapshots {
            if let Some(entry) = self.find_vst3_entry_mut(&path) {
                let new_pos = Some((x, y));
                let new_size = Some((w, h));
                let changed = entry.gui_pos != new_pos || entry.gui_size != new_size;
                crate::logger::log(format!(
                    "[VST3 window snapshot] path=\"{}\" rect=({},{} {}x{}) changed={}",
                    path, x, y, w, h, changed
                ));
                if changed {
                    entry.gui_pos = new_pos;
                    entry.gui_size = new_size;
                    updated += 1;
                }
            }
        }
        updated
    }

    /// VST3 全体表示中の per-plugin GUI 表示状態を `user_hidden` に同期する。
    ///
    /// 全体非表示 (`vst3_gui_visible=false`) のときは全 slot が見えなくなるため、
    /// 個別 user-hidden と区別できない。保存時は全体表示中だけ実 HWND の可視状態を
    /// 反映し、slot preset の「一部だけ非表示」を保持する。
    pub(crate) fn snapshot_vst3_gui_visibility_into_settings(&mut self) -> usize {
        if !self.dsp_bridge.is_enabled() || !self.settings.vst3_gui_visible {
            return 0;
        }
        let snapshots = self.dsp_bridge.slots();
        let mut updated = 0;
        for slot in snapshots {
            if let Some(entry) = self.find_vst3_entry_mut(&slot.plugin_path) {
                let user_hidden = !slot.gui_visible;
                if entry.user_hidden != user_hidden {
                    entry.user_hidden = user_hidden;
                    updated += 1;
                }
            }
        }
        updated
    }

    /// 毎 frame 呼ぶ: 全プラグイン GUI ホストウィンドウからの close / resize シグナルを処理する。
    /// プラグインウィンドウの × ボタンで閉じられたスロットがあれば、settings 側の
    /// `user_hidden` を true に同期して永続化する (= 再起動後の VST 一括表示で skip)。
    ///
    /// `dsp_bridge.pump_gui_signals` は plugin_path 一覧を返す (= bridge slots と
    /// `settings.vst3_plugins` で index がズレるため、path 一致で entry を引く)。
    pub(crate) fn vst3_pump_gui_signals(&mut self) {
        let changes = self.dsp_bridge.pump_gui_signals();
        if changes.user_hidden_paths.is_empty() && changes.bypass_updates.is_empty() {
            return;
        }
        let mut changed = false;
        for path in changes.user_hidden_paths {
            if let Some(entry) = self.find_vst3_entry_mut(&path) {
                if !entry.user_hidden {
                    entry.user_hidden = true;
                    changed = true;
                }
            }
        }
        for (path, bypass) in changes.bypass_updates {
            if let Some(entry) = self.find_vst3_entry_mut(&path) {
                if entry.bypass != bypass {
                    entry.bypass = bypass;
                    changed = true;
                }
            }
        }
        if changed {
            self.settings.save();
        }
    }

    /// 現在の VST3 チェーンを 10 個のスロットへ保存する。
    /// 保存直前に plugin state と editor window 位置/サイズを snapshot する。
    pub(crate) fn save_vst3_chain_slot(&mut self, slot_idx: usize) {
        if slot_idx >= self.settings.vst3_chain_slots.slots.len() {
            return;
        }

        let states = self.snapshot_vst3_states_into_settings();
        let positions = self.snapshot_vst3_window_positions_into_settings();
        let visibility = self.snapshot_vst3_gui_visibility_into_settings();
        let existing_name = self.settings.vst3_chain_slots.slots[slot_idx]
            .as_ref()
            .map(|slot| slot.name.trim().to_string())
            .filter(|name| !name.is_empty());
        let key_label = crate::adjustment::slot_key_label(slot_idx);
        let name = existing_name.unwrap_or_else(|| format!("Slot {key_label}"));
        let plugin_count = self.settings.vst3_plugins.len();

        self.settings.vst3_chain_slots.slots[slot_idx] = Some(Vst3ChainPresetSlot {
            name: name.clone(),
            plugins: self.settings.vst3_plugins.clone(),
            gui_visible: self.settings.vst3_gui_visible,
            video_compact: self.settings.vst3_video_compact,
        });
        if let Some(slot) = self.settings.vst3_chain_slots.slots[slot_idx].as_ref() {
            for (plugin_idx, entry) in slot.plugins.iter().enumerate() {
                crate::logger::log(format!(
                    "[VST3 chain slot] save slot={} plugin={} path=\"{}\" gui_pos={:?} gui_size={:?} hidden={}",
                    key_label,
                    plugin_idx,
                    entry.path,
                    entry.gui_pos,
                    entry.gui_size,
                    entry.user_hidden
                ));
            }
        }
        self.settings.save();
        crate::logger::log(format!(
            "[VST3 chain slot] saved slot={} plugins={} state_updates={} position_updates={} visibility_updates={}",
            key_label, plugin_count, states, positions, visibility
        ));
        self.show_feedback_toast(format!(
            "[VST3 Slot {key_label}: {name} 保存 ({plugin_count}件)]"
        ));
    }

    pub(crate) fn load_vst3_chain_slot(&mut self, slot_idx: usize) {
        if slot_idx >= self.settings.vst3_chain_slots.slots.len() {
            return;
        }

        let Some(slot) = self.settings.vst3_chain_slots.slots[slot_idx].clone() else {
            let key_label = crate::adjustment::slot_key_label(slot_idx);
            self.show_feedback_toast(format!("[VST3 Slot {key_label} は空です]"));
            return;
        };

        let key_label = crate::adjustment::slot_key_label(slot_idx);
        let name = slot.name.clone();
        let plugin_count = slot.plugins.len();
        for (plugin_idx, entry) in slot.plugins.iter().enumerate() {
            crate::logger::log(format!(
                "[VST3 chain slot] load slot={} plugin={} path=\"{}\" gui_pos={:?} gui_size={:?} hidden={}",
                key_label, plugin_idx, entry.path, entry.gui_pos, entry.gui_size, entry.user_hidden
            ));
        }
        self.settings.vst3_plugins = slot.plugins;
        self.settings.vst3_gui_visible = slot.gui_visible;
        self.settings.vst3_video_compact = slot.video_compact;
        self.settings.save();
        self.kick_off_vst3_chain_rebuild_without_snapshot();
        crate::logger::log(format!(
            "[VST3 chain slot] loaded slot={} plugins={} name=\"{}\"",
            key_label, plugin_count, name
        ));
        self.show_feedback_toast(format!(
            "[VST3 Slot {key_label}: {name} 読込 ({plugin_count}件)]"
        ));
    }

    /// `settings.vst3_plugins` を bridge に再反映する (= 順序・追加・削除を bridge へ伝搬)。
    /// worker thread で bridge を nuke + re-enable + チェーン全部 add_plugin する。
    /// 環境設定→VST3 ページの編集後、enable トグル後に呼ぶ。
    ///
    /// 再構築前に **runtime プラグイン状態を snapshot して settings に保存** する
    /// (= 動作中の EQ カーブ等を re-add 時の `entry.state` で復元するため)。
    /// snapshot しないとチェーン編集 OK の度に EQ がデフォルトに戻る。
    pub(crate) fn kick_off_vst3_chain_rebuild(&mut self) {
        self.kick_off_vst3_chain_rebuild_impl(true);
    }

    fn kick_off_vst3_chain_rebuild_without_snapshot(&mut self) {
        self.kick_off_vst3_chain_rebuild_impl(false);
    }

    fn kick_off_vst3_chain_rebuild_impl(&mut self, snapshot_runtime: bool) {
        if !self.settings.vst3_enabled {
            return;
        }
        if snapshot_runtime {
            let states = self.snapshot_vst3_states_into_settings();
            let positions = self.snapshot_vst3_window_positions_into_settings();
            if states > 0 || positions > 0 {
                self.settings.save();
            }
        }
        let bridge = self.dsp_bridge.clone();
        let plugins = self.settings.vst3_plugins.clone();
        let gui_visible = self.settings.vst3_gui_visible;
        // T21 (Codex R-VST-001): 連続 rebuild / startup load との interleave 防止。
        // bump で「私が新主」を宣言し、worker は要所で stale 検出して exit する。
        let my_gen = bridge.bump_chain_rebuild_gen();
        std::thread::Builder::new()
            .name("vst3-chain-rebuild".into())
            .spawn(move || {
                if bridge.is_chain_rebuild_stale(my_gen) {
                    crate::logger::log(format!(
                        "[VST3 chain-rebuild] gen={my_gen} stale before disable, skipping"
                    ));
                    return;
                }
                bridge.disable();
                if bridge.is_chain_rebuild_stale(my_gen) {
                    crate::logger::log(format!(
                        "[VST3 chain-rebuild] gen={my_gen} stale after disable, skipping enable"
                    ));
                    return;
                }
                if let Err(e) = bridge.enable() {
                    crate::logger::log(format!("vst3 chain-rebuild enable: {e}"));
                    return;
                }
                let sample_rate =
                    crate::video::audio::default_output_sample_rate().unwrap_or(48_000);
                for entry in plugins {
                    if bridge.is_chain_rebuild_stale(my_gen) {
                        crate::logger::log(format!(
                            "[VST3 chain-rebuild] gen={my_gen} stale mid-chain, dropping remaining add_plugin"
                        ));
                        return;
                    }
                    if let Err(e) = bridge.add_plugin(
                        &entry.path,
                        sample_rate,
                        480,
                        entry.bypass,
                        entry.user_hidden,
                        entry.state.as_deref(),
                        entry.gui_pos,
                        entry.gui_size,
                    ) {
                        crate::logger::log(format!(
                            "vst3 chain-rebuild add_plugin {} failed: {e}",
                            entry.path
                        ));
                    }
                }
                if bridge.is_chain_rebuild_stale(my_gen) {
                    crate::logger::log(format!(
                        "[VST3 chain-rebuild] gen={my_gen} stale before gui show, skipping"
                    ));
                    return;
                }
                if gui_visible {
                    bridge.set_all_guis_visible(true);
                }
            })
            .ok();
    }
}
