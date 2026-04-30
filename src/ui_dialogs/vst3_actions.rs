//! VST3 プラグイン GUI のホスト側操作 (App メソッド)。
//!
//! 実装の本体は [`crate::video::dsp::DspBridge`] にある。ここはそれを App から
//! 呼ぶための薄いラッパー (= V キーハンドラ / pump tick など)。

#![cfg(windows)]

use crate::app::App;

impl App {
    /// V キーハンドラ: 全プラグイン GUI を一斉トグル。
    /// 操作後の状態を `settings.vst3_gui_visible` に反映する。
    pub(crate) fn vst3_toggle_all_plugin_guis(&mut self) {
        let target = self.dsp_bridge.toggle_all_guis();
        self.settings.vst3_gui_visible = target;
    }

    /// 毎 frame 呼ぶ: 全プラグイン GUI ホストウィンドウからの close / resize シグナルを処理する。
    pub(crate) fn vst3_pump_gui_signals(&mut self) {
        self.dsp_bridge.pump_gui_signals();
    }
}
