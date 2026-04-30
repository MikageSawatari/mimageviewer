//! VST3 プラグイン GUI のホスト側操作 (App メソッド)。
//!
//! 実装の本体は [`crate::video::dsp::DspBridge`] にある。ここはそれを App から
//! 呼ぶための薄いラッパー。

#![cfg(windows)]

use crate::app::App;

impl App {
    /// 毎 frame 呼ぶ: 全プラグイン GUI ホストウィンドウからの close / resize シグナルを処理する。
    /// プラグインウィンドウの × ボタンで閉じられたスロットがあれば、settings 側の
    /// `user_hidden` を true に同期して永続化する (= 再起動後の VST 一括表示で skip)。
    pub(crate) fn vst3_pump_gui_signals(&mut self) {
        let user_hidden_indices = self.dsp_bridge.pump_gui_signals();
        if user_hidden_indices.is_empty() {
            return;
        }
        let mut changed = false;
        for idx in user_hidden_indices {
            if let Some(entry) = self.settings.vst3_plugins.get_mut(idx) {
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
                    if let Err(e) = bridge.add_plugin(
                        &entry.path,
                        sample_rate,
                        480,
                        entry.bypass,
                        entry.user_hidden,
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
