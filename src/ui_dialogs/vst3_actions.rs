//! VST3 プラグイン GUI のホスト側操作 (App メソッド)。
//!
//! 実装の本体は [`crate::video::dsp::DspBridge`] にある。ここはそれを App から
//! 呼ぶための薄いラッパー。

#![cfg(windows)]

use crate::app::App;
use crate::settings::Vst3PluginEntry;

impl App {
    /// `settings.vst3_plugins` を **plugin_path** で検索する共通 helper。
    /// bridge slot idx と settings idx は load 失敗で詰まるとズレるため
    /// (Codex P2、2026-05-01)、path をキーに entry を引く流儀に統一する。
    /// path は preferences 側で重複追加を弾いているので一意。
    pub(crate) fn find_vst3_entry_mut(
        &mut self,
        path: &str,
    ) -> Option<&mut Vst3PluginEntry> {
        self.settings.vst3_plugins.iter_mut().find(|e| e.path == path)
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
                if entry.gui_pos != new_pos || entry.gui_size != new_size {
                    entry.gui_pos = new_pos;
                    entry.gui_size = new_size;
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
        let user_hidden_paths = self.dsp_bridge.pump_gui_signals();
        if user_hidden_paths.is_empty() {
            return;
        }
        let mut changed = false;
        for path in user_hidden_paths {
            if let Some(entry) = self.find_vst3_entry_mut(&path) {
                if !entry.user_hidden {
                    entry.user_hidden = true;
                    changed = true;
                }
            }
        }
        if changed {
            self.settings.save();
        }
    }

    /// `settings.vst3_plugins` を bridge に再反映する (= 順序・追加・削除を bridge へ伝搬)。
    /// worker thread で bridge を nuke + re-enable + チェーン全部 add_plugin する。
    /// 環境設定→VST3 ページの編集後、enable トグル後に呼ぶ。
    ///
    /// 再構築前に **runtime プラグイン状態を snapshot して settings に保存** する
    /// (= 動作中の EQ カーブ等を re-add 時の `entry.state` で復元するため)。
    /// snapshot しないとチェーン編集 OK の度に EQ がデフォルトに戻る。
    pub(crate) fn kick_off_vst3_chain_rebuild(&mut self) {
        if !self.settings.vst3_enabled {
            return;
        }
        let states = self.snapshot_vst3_states_into_settings();
        let positions = self.snapshot_vst3_window_positions_into_settings();
        if states > 0 || positions > 0 {
            self.settings.save();
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
                    if let Err(e) = bridge.add_plugin(
                        &entry.path,
                        sample_rate,
                        480,
                        entry.bypass,
                        entry.user_hidden,
                        entry.state.as_deref(),
                        entry.gui_pos,
                    ) {
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
