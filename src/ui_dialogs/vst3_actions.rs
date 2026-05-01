//! VST3 プラグイン GUI のホスト側操作 (App メソッド)。
//!
//! 実装の本体は [`crate::video::dsp::DspBridge`] にある。ここはそれを App から
//! 呼ぶための薄いラッパー。

#![cfg(windows)]

use crate::app::App;

impl App {
    /// 全 Loaded プラグインの内部状態 (= EQ カーブ / chunk) を bridge から取得して、
    /// `settings.vst3_plugins` の対応する entry の `state` フィールドに **path 一致**
    /// で書き込む。**呼び出し側で `self.settings.save()` を別途呼ぶこと**。
    /// 戻り値: 更新された entry 数 (= 0 なら save 不要の判断材料に使える)。
    ///
    /// **同期で走る**: 各 plugin への IPC roundtrip × 通常 10ms / hung 時 1000ms。
    /// チェーン上限 10 個で worst case 10 秒だが、典型は数十 ms。on_exit / chain
    /// rebuild など save 直前のフックで 1 回だけ呼ぶ想定 (= UI hot-path には乗らない)。
    pub(crate) fn snapshot_vst3_states_into_settings(&mut self) -> usize {
        if !self.settings.vst3_enabled {
            return 0;
        }
        let snapshots = self.dsp_bridge.snapshot_all_plugin_states();
        let mut updated = 0;
        for (path, state) in snapshots {
            if let Some(entry) =
                self.settings.vst3_plugins.iter_mut().find(|e| e.path == path)
            {
                let new_state = Some(state);
                if entry.state != new_state {
                    entry.state = new_state;
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
            if let Some(entry) =
                self.settings.vst3_plugins.iter_mut().find(|e| e.path == path)
            {
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
        let snapshotted = self.snapshot_vst3_states_into_settings();
        if snapshotted > 0 {
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
