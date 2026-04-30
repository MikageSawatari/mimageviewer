//! VST3 プラグイン GUI のホスト側操作 (App メソッド)。
//!
//! - `vst3_show_plugin_gui`: プラグイン GUI ホストウィンドウを開いてプラグインを attach
//! - `vst3_hide_plugin_gui`: bridge にデタッチ命令を送ってホストウィンドウを閉じる
//! - `vst3_toggle_plugin_gui`: V キー / 管理ウィンドウから呼ばれるトグル
//! - `vst3_pump_gui_signals`: ホストウィンドウ × ボタンや リサイズ通知を tick で処理
//!
//! メインスレッドから呼ぶ前提 (= bridge 応答待ちが ms オーダーなので OK)。

#![cfg(windows)]

use crate::app::App;
use crate::video::dsp::{Cmd as DspCmd, DspState, Event as DspEvent};

impl App {
    /// プラグイン GUI を表示する。`bridge` がロード済みでなければ no-op。
    pub(crate) fn vst3_show_plugin_gui(&mut self) {
        if !matches!(self.dsp_bridge.state(), DspState::Loaded) {
            crate::logger::log("vst3 show_gui: plugin not loaded");
            return;
        }
        if let Some(hwnd) = self.vst3_gui_hwnd {
            crate::video::dsp::gui::bring_to_front(hwnd);
            return;
        }

        let title = self
            .dsp_bridge
            .plugin_name()
            .unwrap_or_else(|| "VST3 Plugin".to_string());

        // ─ Step 1: プラグインの推奨 GUI サイズを取得 ─
        let (pref_w, pref_h) =
            match self.dsp_bridge.send_recv(&DspCmd::QueryGuiSize) {
                Ok(DspEvent::GuiSize { width, height }) => (width, height),
                Ok(other) => {
                    crate::logger::log(format!(
                        "vst3 query_gui_size: unexpected event {other:?}, fallback 1200x800"
                    ));
                    (1200u32, 800u32)
                }
                Err(e) => {
                    crate::logger::log(format!(
                        "vst3 query_gui_size: {e}, fallback 1200x800"
                    ));
                    (1200u32, 800u32)
                }
            };

        // ─ Step 2: ホストウィンドウを作成 ─
        if self.vst3_gui_host.is_none() {
            self.vst3_gui_host = Some(crate::video::dsp::gui::GuiHost::spawn());
        }
        let gui_host = self.vst3_gui_host.as_ref().unwrap();
        let reply = match gui_host.show(&title, pref_w, pref_h) {
            Ok(r) => r,
            Err(e) => {
                crate::logger::log(format!("vst3 create gui window: {e}"));
                return;
            }
        };
        if reply.hwnd_u64 == 0 {
            crate::logger::log("vst3 create gui window: HWND not returned");
            return;
        }
        self.vst3_gui_hwnd = Some(reply.hwnd_u64);
        self.vst3_gui_close_signal = Some(reply.close_signal);
        self.vst3_gui_resize_signal = Some(reply.resize_signal);

        // ─ Step 3: bridge に attach 命令を送る ─
        let attach_result = self.dsp_bridge.send_recv(&DspCmd::ShowGui {
            hwnd: reply.hwnd_u64,
        });
        match attach_result {
            Ok(DspEvent::GuiAttached { width, height }) => {
                if width > 0 && height > 0 && (width != pref_w || height != pref_h) {
                    crate::video::dsp::gui::resize_window_client(
                        reply.hwnd_u64,
                        width,
                        height,
                    );
                }
                crate::logger::log(format!(
                    "vst3 gui attached: {}x{}",
                    width, height
                ));
            }
            Ok(DspEvent::Error { detail }) => {
                crate::logger::log(format!("vst3 attach error: {detail}"));
                self.vst3_hide_plugin_gui();
                return;
            }
            Ok(other) => {
                crate::logger::log(format!("vst3 attach: unexpected event {other:?}"));
            }
            Err(e) => {
                crate::logger::log(format!("vst3 attach: recv {e}"));
                self.vst3_hide_plugin_gui();
                return;
            }
        }
        self.settings.vst3_gui_visible = true;
    }

    /// プラグイン GUI を閉じる。bridge にデタッチ命令を送ってからホストウィンドウを破棄する。
    pub(crate) fn vst3_hide_plugin_gui(&mut self) {
        // bridge デタッチを先に送る (順序逆だと crash 報告あり)。応答 (gui_detached) は
        // best-effort として待たない (送信エラーは無視)。
        if self.vst3_gui_hwnd.is_some() {
            let _ = self.dsp_bridge.send_oneway(&DspCmd::HideGui);
        }
        if let Some(host) = self.vst3_gui_host.as_ref() {
            host.close();
        }
        self.vst3_gui_hwnd = None;
        self.vst3_gui_close_signal = None;
        self.vst3_gui_resize_signal = None;
        self.settings.vst3_gui_visible = false;
    }

    /// プラグイン GUI 表示状態をトグル。V キー / 管理ウィンドウのトグルボタン共用。
    pub(crate) fn vst3_toggle_plugin_gui(&mut self) {
        if self.vst3_gui_hwnd.is_some() {
            self.vst3_hide_plugin_gui();
        } else {
            self.vst3_show_plugin_gui();
        }
    }

    /// 毎 frame 呼ぶ: GUI ホストウィンドウからの close / resize シグナルを処理する。
    pub(crate) fn vst3_pump_gui_signals(&mut self) {
        // close 通知 (= ユーザーが × を押した)
        let mut closed = false;
        if let Some(arc) = self.vst3_gui_close_signal.as_ref() {
            if let Ok(guard) = arc.lock() {
                if let Some(rx) = guard.as_ref() {
                    if matches!(rx.try_recv(), Ok(())) {
                        closed = true;
                    }
                }
            }
        }
        if closed {
            self.vst3_hide_plugin_gui();
            return;
        }

        // resize 通知 (= ユーザーがホストウィンドウをドラッグでリサイズした)
        let mut latest_size: Option<(u32, u32)> = None;
        if let Some(arc) = self.vst3_gui_resize_signal.as_ref() {
            if let Ok(guard) = arc.lock() {
                if let Some(rx) = guard.as_ref() {
                    while let Ok(size) = rx.try_recv() {
                        latest_size = Some(size);
                    }
                }
            }
        }
        if let Some((w, h)) = latest_size {
            let _ = self.dsp_bridge.send_oneway(&DspCmd::NotifyHostResize {
                width: w,
                height: h,
            });
        }
    }
}
