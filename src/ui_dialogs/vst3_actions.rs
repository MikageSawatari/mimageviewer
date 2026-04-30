//! VST3 プラグイン GUI のホスト側操作 (App メソッド)。
//!
//! 実装の本体は [`crate::video::dsp::DspBridge`] にある。ここはそれを App から
//! 呼ぶための薄いラッパー。

#![cfg(windows)]

use crate::app::App;

impl App {
    /// 毎 frame 呼ぶ: 全プラグイン GUI ホストウィンドウからの close / resize シグナルを処理する。
    pub(crate) fn vst3_pump_gui_signals(&mut self) {
        self.dsp_bridge.pump_gui_signals();
    }
}
