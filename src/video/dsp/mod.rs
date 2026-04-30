//! VST3 プラグインホスト bridge との連携モジュール (動画音声 DSP 経路)。
//!
//! 設計の全体像は [`docs/vst3-integration.md`] 参照。要点だけここに記す:
//!
//! - **C++ bridge プロセス** (`mimageviewer-vst3-host.exe`) を `include_bytes!` で
//!   メイン exe に埋め込み、初回 VST3 enable 時に
//!   `%APPDATA%\mimageviewer\vst3\` に展開する (PDFium / Susie ワーカーと同パターン)。
//! - **DspBridge** がアプリ起動から終了まで生存する singleton。プラグインスロットの
//!   Vec を保持し、各スロットが独立した bridge プロセスを所有する。
//! - **プラグインチェーン**: スロットを順番に通すマルチプラグイン構成。
//!   bypass=true のスロットはスキップ。各スロットの IPC roundtrip ~1-2ms なので
//!   実用的には ~5 個までが realtime 維持の目安 (= 1024-sample frame で 21ms 予算)。
//! - 音声経路への結線は [`super::audio`] の audio-pump スレッドで行う。

#![cfg(windows)]

pub mod bridge;
pub mod extract;
pub mod gui;
pub mod scanner;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub use bridge::{Bridge, Cmd, Event};
pub use scanner::{DiscoveredPlugin, default_vst3_paths, scan};

/// DSP bridge 全体の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DspState {
    /// VST3 機能無効 (デフォルト)。bridge プロセスは 1 本も起動しない。
    Disabled,
    /// VST3 有効、スロットは 0 個以上。
    Enabled,
    /// エラー状態。再 enable で復旧。
    Error(&'static str),
}

/// 1 スロットの状態。`Loaded` のスロットだけが音声処理に参加する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    Loading,
    Loaded,
    Error,
}

/// UI に渡すための per-slot スナップショット。
#[derive(Debug, Clone)]
pub struct SlotInfo {
    pub plugin_path: String,
    pub plugin_name: Option<String>,
    pub state: SlotState,
    pub latency_samples: u32,
    pub bypass: bool,
    /// プラグイン GUI が現在可視かどうか (= ShowWindow 状態)。
    /// 旧 `gui_hwnd != 0` を「可視」の意味で使っていたが、永続 GuiHost 設計では
    /// HWND は作成後に保持され続けるので、可視状態は別フラグで管理する。
    pub gui_visible: bool,
}

/// DspBridge — VST3 プラグインホスト bridge との対話を管理する singleton。
///
/// アプリ起動から終了まで 1 個のインスタンスを保持する。
/// `Arc<DspBridge>` 化して audio-pump thread と UI thread から共有アクセス。
pub struct DspBridge {
    inner: Mutex<DspBridgeInner>,
    /// audio-pump thread が高速判定するためのフラグ。Mutex を取らずに読める。
    enabled: AtomicBool,
    /// 「処理対象スロット (= Loaded 且つ bypass=false) の個数」を atomic で公開。
    /// audio-pump はこれが 0 ならパススルーで早期 return できる。
    active_slot_count: AtomicUsize,
    /// プラグイン GUI ウィンドウを TOPMOST にしておきたいか (= フルスクリーン
    /// 動画再生中の "希望状態")。`set_all_guis_topmost` で更新され、`show_slot_gui`
    /// の新規作成・再表示パスで「現在の希望状態に合わせて」最終的な TOPMOST を
    /// 適用する。これにより fullscreen 中に後から作った HWND にも TOPMOST が
    /// 自動適用される (Codex P3 不具合 3 対応)。
    gui_topmost_desired: AtomicBool,
}

struct DspBridgeInner {
    state: DspState,
    slots: Vec<PluginSlot>,
    /// 直近 `process_block` が使う scratch バッファ。Mutex 内に持たせて毎回 alloc を回避。
    /// 2 本必要なのは ping-pong 時のみだが、シンプルさのため常に 2 本持つ。
    scratch_a: Vec<f32>,
    scratch_b: Vec<f32>,
}

pub(crate) struct PluginSlot {
    /// 子 bridge プロセス。`Arc` 化することで audio-pump が snapshot を取って
    /// inner ロックを解放してから IPC を行えるようにする。
    pub bridge: Arc<Bridge>,
    pub plugin_path: String,
    pub plugin_name: Option<String>,
    pub state: SlotState,
    pub latency_samples: u32,
    pub bypass: bool,
    /// プラグイン GUI HWND (0 = まだ作成されていない)。
    /// **一度作成されたら slot 削除まで非 0** のまま保持される (= 永続 GuiHost 設計)。
    /// 表示・非表示の切替は `gui_visible` フラグで管理し、ShowWindow(SW_HIDE/SW_SHOWNA)
    /// で実装する。これにより show/hide のたびに createView/removed をする
    /// プラグイン重い処理 (Pro-Q 4 / Insight2 等) を 1 回に抑え、DAW 並みの
    /// 高速トグルを実現する。
    pub gui_hwnd: u64,
    /// プラグイン GUI が現在可視か (= ShowWindow の状態を mIV 側で覚えておく)。
    /// `gui_hwnd != 0 && !gui_visible` = ウィンドウは作成済みだが SW_HIDE 状態。
    pub gui_visible: bool,
    /// プラグイン GUI ホスト (Win32 子ウィンドウスレッド)。slot 削除で自動終了する。
    pub gui_host: Option<gui::GuiHost>,
    /// ホストウィンドウの × ボタン押下シグナル。
    pub gui_close_signal:
        Option<Arc<Mutex<Option<std::sync::mpsc::Receiver<()>>>>>,
    /// ホストウィンドウのリサイズシグナル。
    pub gui_resize_signal:
        Option<Arc<Mutex<Option<std::sync::mpsc::Receiver<(u32, u32)>>>>>,
    /// ホストウィンドウの WM_ENTERSIZEMOVE / WM_EXITSIZEMOVE シグナル
    /// (= ユーザー drag による resize/move session 開始 / 終了、Codex P4 対応)。
    pub gui_resize_session_signal:
        Option<Arc<Mutex<Option<std::sync::mpsc::Receiver<bool>>>>>,
}

impl DspBridge {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(DspBridgeInner {
                state: DspState::Disabled,
                slots: Vec::new(),
                scratch_a: Vec::new(),
                scratch_b: Vec::new(),
            }),
            enabled: AtomicBool::new(false),
            active_slot_count: AtomicUsize::new(0),
            gui_topmost_desired: AtomicBool::new(false),
        })
    }

    /// `audio-pump` スレッドからのホットパスチェック。Mutex を取らない。
    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// `audio-pump` スレッドからのホットパスチェック。Mutex を取らない。
    /// 0 ならパススルー (= 早期 return) できる。
    #[inline]
    pub fn active_slot_count(&self) -> usize {
        self.active_slot_count.load(Ordering::Acquire)
    }

    pub fn state(&self) -> DspState {
        self.inner.lock().unwrap().state
    }

    /// 全スロットの SlotInfo を返す (UI 表示用)。
    pub fn slots(&self) -> Vec<SlotInfo> {
        self.inner
            .lock()
            .unwrap()
            .slots
            .iter()
            .map(|s| SlotInfo {
                plugin_path: s.plugin_path.clone(),
                plugin_name: s.plugin_name.clone(),
                state: s.state,
                latency_samples: s.latency_samples,
                bypass: s.bypass,
                gui_visible: s.gui_visible,
            })
            .collect()
    }

    /// 指定 idx のスロット情報を返す。
    pub fn slot(&self, idx: usize) -> Option<SlotInfo> {
        self.inner.lock().unwrap().slots.get(idx).map(|s| SlotInfo {
            plugin_path: s.plugin_path.clone(),
            plugin_name: s.plugin_name.clone(),
            state: s.state,
            latency_samples: s.latency_samples,
            bypass: s.bypass,
            gui_visible: s.gui_visible,
        })
    }

    /// VST3 機能を有効化する。bridge exe を APPDATA に展開する (子プロセスは
    /// プラグイン追加時に `add_plugin` から個別に spawn される)。
    /// 既に enable 済みなら no-op。
    pub fn enable(&self) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        if matches!(inner.state, DspState::Enabled) {
            return Ok(());
        }
        // bridge exe が APPDATA に展開できるか先にテスト (失敗時はここで早期 return)
        extract::ensure_bridge_extracted()
            .map_err(|e| format!("bridge exe 展開失敗: {e}"))?;
        inner.state = DspState::Enabled;
        self.enabled.store(true, Ordering::Release);
        Ok(())
    }

    /// VST3 機能を無効化する。全スロットを破棄して各 bridge 子プロセスを終了する。
    pub fn disable(&self) {
        self.enabled.store(false, Ordering::Release);
        self.active_slot_count.store(0, Ordering::Release);
        let mut inner = self.inner.lock().unwrap();
        for slot in inner.slots.drain(..) {
            // bridge は Arc<Bridge> なので、まだ使用中なら解放されない。
            // shutdown は best-effort で送るが、Arc の最後の参照が落ちる時点で
            // Drop 経路で kill される。
            let _ = slot.bridge.shutdown_async();
        }
        inner.state = DspState::Disabled;
    }

    /// 指定パスの VST3 プラグインを新しいスロットとしてチェーン末尾に追加する。
    /// **worker thread から呼ぶ前提** (= bridge spawn は ~数百 ms 取るので UI を止めない)。
    /// 戻り値: 追加された位置 (idx)。
    pub fn add_plugin(
        &self,
        plugin_path: &str,
        sample_rate: u32,
        block_size: u32,
        bypass: bool,
    ) -> Result<usize, String> {
        // enable 状態チェック (Mutex を保持しない)
        if !self.is_enabled() {
            return Err("VST3 が無効化されています (enable を先に)".to_string());
        }

        // bridge exe path
        let exe = extract::ensure_bridge_extracted()
            .map_err(|e| format!("bridge exe 展開失敗: {e}"))?;

        // ── 子プロセス spawn + hello 応答待ち (Mutex 外で実施) ──
        let mut bridge = Bridge::spawn(exe, |line| {
            crate::logger::log(format!("[vst3-bridge] {line}"));
        })
        .map_err(|e| format!("bridge spawn 失敗: {e}"))?;
        bridge
            .send(&Cmd::Hello { version: 1 })
            .map_err(|e| format!("hello send: {e}"))?;
        match bridge.recv() {
            Ok(Event::Ready { .. }) => {}
            Ok(other) => return Err(format!("予期しないイベント (ready 待ち): {other:?}")),
            Err(e) => return Err(format!("ready recv: {e}")),
        }
        bridge
            .open_audio_pipe(plugin_path, sample_rate, block_size)
            .map_err(|e| format!("open_audio_pipe: {e}"))?;

        // loaded イベントを待つ
        let (plugin_name, latency_samples) = loop {
            match bridge.recv() {
                Ok(Event::Loaded {
                    plugin_name,
                    latency_samples,
                }) => break (plugin_name, latency_samples),
                Ok(Event::Error { detail }) => {
                    return Err(format!("プラグインロード失敗: {detail}"));
                }
                Ok(_) => continue,
                Err(e) => return Err(format!("recv: {e}")),
            }
        };

        // ── プラグイン pre-warm: 無音ブロックを数回流し、内部状態 (フィルタ係数の
        //   FTZ 経路 / IIR ステート / 内部 alloc / ページフォルト等) を温める。
        //   これがないと「動画再生開始直後に process_block の応答が遅く、cpal が
        //   下流で underrun してプチプチノイズになる」現象が出る (動画 4-5 個目以降は
        //   別動画が来ても plugin の状態は維持されているので問題なし、初回のみ重い)。
        //   block_size * 2 channels, 20 ブロック ≈ 200ms 相当の silence を流す。
        let bridge_arc = Arc::new(bridge);
        {
            let n = (block_size as usize) * 2;
            let silence = vec![0.0f32; n];
            let mut dst = vec![0.0f32; n];
            for _ in 0..20 {
                if bridge_arc.push_audio(&silence).is_err() {
                    break;
                }
                if bridge_arc.pull_audio(&mut dst, 200).is_err() {
                    break;
                }
            }
        }

        // ── 完成したスロットを Vec に追加 (Mutex 内で短時間) ──
        let mut inner = self.inner.lock().unwrap();
        let idx = inner.slots.len();
        inner.slots.push(PluginSlot {
            bridge: bridge_arc,
            plugin_path: plugin_path.to_string(),
            plugin_name: Some(plugin_name),
            state: SlotState::Loaded,
            latency_samples,
            bypass,
            gui_hwnd: 0,
            gui_visible: false,
            gui_host: None,
            gui_close_signal: None,
            gui_resize_signal: None,
            gui_resize_session_signal: None,
        });
        self.recalc_active_count(&inner);
        Ok(idx)
    }

    /// 全 active プラグインに **無音ブロックを N 個流す**。
    ///
    /// 用途は 2 つ:
    /// 1. 動画再生終了時 (= AudioOutput drop): bridge in_ring に残った最後の audio が
    ///    プラグイン LUFS / Visualizer に表示され続けるのを止めるため、silence を
    ///    押し込んで visualizer を 0 に落とす
    /// 2. 動画再生直前の追加 warm-up: kick_off_vst3_startup から呼び、cpal callback の
    ///    最初の数ブロックに対して bridge IPC が安定して走る状態にする
    ///
    /// blocks=10 で約 100ms 相当 (block_size=480 @ 48kHz)。bridge への push が
    /// fail した時点で打ち切り (= 既にシャットダウン中の bridge をキックして
    /// 待機しないようにする)。
    pub fn flush_silence(&self, block_size: u32, blocks: u32) {
        if !self.is_enabled() {
            return;
        }
        let bridges: Vec<Arc<Bridge>> = {
            let inner = self.inner.lock().unwrap();
            inner
                .slots
                .iter()
                .filter(|s| matches!(s.state, SlotState::Loaded))
                .map(|s| s.bridge.clone())
                .collect()
        };
        if bridges.is_empty() {
            return;
        }
        let n = (block_size as usize) * 2;
        let silence = vec![0.0f32; n];
        let mut dst = vec![0.0f32; n];
        for b in &bridges {
            for _ in 0..blocks {
                if b.push_audio(&silence).is_err() {
                    break;
                }
                if b.pull_audio(&mut dst, 200).is_err() {
                    break;
                }
            }
        }
    }

    /// 指定 idx のプラグイン GUI を表示する。
    ///
    /// **永続 GuiHost 設計**: 一度作成された window は slot 削除まで保持される。
    /// 2 回目以降の呼び出しは ShowWindow(SW_SHOWNA) でウィンドウを可視化するだけ
    /// (= プラグインの createView/removed をスキップ → DAW 並みの高速トグル)。
    ///
    /// メインスレッドから呼ぶ前提。初回のみ bridge 応答待ちで ~数百 ms かかる。
    pub fn show_slot_gui(&self, idx: usize) -> Result<(), String> {
        // 既存ウィンドウが作成済みなら可視化のみで早期 return (= 高速パス)
        // **z-order は触らない**: ユーザーが手で並べた前後関係を保持する (Codex P1)。
        // ただし `gui_topmost_desired` の現在値は最後に適用する (= fullscreen 中に
        // 作った HWND にも TOPMOST が反映される、Codex P3)。
        {
            let inner = self.inner.lock().unwrap();
            if let Some(slot) = inner.slots.get(idx) {
                if !matches!(slot.state, SlotState::Loaded) {
                    return Err("プラグイン未ロード".to_string());
                }
                if slot.gui_hwnd != 0 {
                    let hwnd = slot.gui_hwnd;
                    drop(inner);
                    gui::set_window_visible(hwnd, true);
                    // 現在の topmost desired state を反映
                    let topmost = self.gui_topmost_desired.load(Ordering::Acquire);
                    gui::set_window_topmost(hwnd, topmost);
                    let mut inner2 = self.inner.lock().unwrap();
                    if let Some(s2) = inner2.slots.get_mut(idx) {
                        s2.gui_visible = true;
                    }
                    return Ok(());
                }
            } else {
                return Err("スロット範囲外".to_string());
            }
        }

        // ─ Step 1: bridge に推奨 GUI サイズを問い合わせる (Mutex 外) ─
        let bridge_arc = {
            let inner = self.inner.lock().unwrap();
            inner
                .slots
                .get(idx)
                .map(|s| s.bridge.clone())
                .ok_or_else(|| "スロット範囲外".to_string())?
        };
        let plugin_name = {
            let inner = self.inner.lock().unwrap();
            inner
                .slots
                .get(idx)
                .and_then(|s| s.plugin_name.clone())
                .unwrap_or_else(|| "VST3 Plugin".to_string())
        };
        bridge_arc
            .send(&Cmd::QueryGuiSize)
            .map_err(|e| format!("send QueryGuiSize: {e}"))?;
        let (pref_w, pref_h, resizable) = match bridge_arc.recv() {
            Ok(Event::GuiSize { width, height, resizable }) => (width, height, resizable),
            Ok(other) => {
                crate::logger::log(format!(
                    "vst3 query_gui_size: unexpected {other:?}, fallback 1200x800"
                ));
                (1200, 800, true)
            }
            Err(e) => {
                crate::logger::log(format!("vst3 query_gui_size: {e}, fallback 1200x800"));
                (1200, 800, true)
            }
        };

        // ─ Step 2: ホストウィンドウを spawn (resizable に応じてサイズ変更可否を切替) ─
        let gui_host = gui::GuiHost::spawn();
        let reply = gui_host
            .show(&plugin_name, pref_w, pref_h, resizable)
            .map_err(|e| format!("create gui window: {e}"))?;
        if reply.hwnd_u64 == 0 {
            return Err("HWND not returned".to_string());
        }
        let hwnd = reply.hwnd_u64;

        // ─ Step 3: bridge に attach 命令 (Mutex 外、bridge_arc 経由) ─
        bridge_arc
            .send(&Cmd::ShowGui { hwnd })
            .map_err(|e| format!("send ShowGui: {e}"))?;
        match bridge_arc.recv() {
            Ok(Event::GuiAttached { width, height }) => {
                if width > 0 && height > 0 && (width != pref_w || height != pref_h) {
                    gui::resize_window_client(hwnd, width, height);
                }
            }
            Ok(Event::Error { detail }) => {
                gui_host.close();
                return Err(format!("gui attach: {detail}"));
            }
            Ok(other) => {
                crate::logger::log(format!("vst3 attach: unexpected {other:?}"));
            }
            Err(e) => {
                gui_host.close();
                return Err(format!("attach recv: {e}"));
            }
        }

        // ─ Step 4: スロットに GUI 情報を書き戻す ─
        let mut inner = self.inner.lock().unwrap();
        if let Some(slot) = inner.slots.get_mut(idx) {
            slot.gui_hwnd = hwnd;
            slot.gui_visible = true;
            slot.gui_host = Some(gui_host);
            slot.gui_close_signal = Some(reply.close_signal);
            slot.gui_resize_signal = Some(reply.resize_signal);
            slot.gui_resize_session_signal = Some(reply.resize_session_signal);
        }
        drop(inner);
        // 新規作成時も「現在の topmost desired state」を反映
        // (= フルスクリーン中に作った HWND にも TOPMOST が必ず付く、Codex P3 対応)。
        let topmost = self.gui_topmost_desired.load(Ordering::Acquire);
        if topmost {
            gui::set_window_topmost(hwnd, true);
        }
        Ok(())
    }

    /// 指定 idx のプラグイン GUI を **隠す**。
    ///
    /// **永続 GuiHost 設計**: ウィンドウは破棄せず ShowWindow(SW_HIDE) で隠すだけ。
    /// プラグインの IPlugView は attached のまま (= audio はそのまま流れる、
    /// 内部状態は維持)。次の `show_slot_gui` 呼び出しは ShowWindow(SW_SHOWNA)
    /// 一発で復活する (DAW 並みの高速トグル)。
    pub fn hide_slot_gui(&self, idx: usize) {
        let hwnd = {
            let mut inner = self.inner.lock().unwrap();
            if let Some(slot) = inner.slots.get_mut(idx) {
                slot.gui_visible = false;
                slot.gui_hwnd
            } else {
                0
            }
        };
        if hwnd != 0 {
            gui::set_window_visible(hwnd, false);
        }
    }

    /// 指定 idx の GUI が表示中かをトグルする。
    pub fn toggle_slot_gui(&self, idx: usize) -> Result<(), String> {
        let visible = {
            let inner = self.inner.lock().unwrap();
            inner.slots.get(idx).map(|s| s.gui_visible).unwrap_or(false)
        };
        if visible {
            self.hide_slot_gui(idx);
            Ok(())
        } else {
            self.show_slot_gui(idx)
        }
    }

    /// 全プラグイン GUI を表示/非表示一斉トグル (V キーハンドラ)。
    /// `target_visible=true` なら表示、false なら非表示にする。
    pub fn set_all_guis_visible(&self, target_visible: bool) {
        let n = {
            let inner = self.inner.lock().unwrap();
            inner.slots.len()
        };
        for idx in 0..n {
            if target_visible {
                let _ = self.show_slot_gui(idx);
            } else {
                self.hide_slot_gui(idx);
            }
        }
    }

    /// 全プラグイン GUI ウィンドウの TOPMOST 属性を一斉切替。
    /// - フルスクリーン動画再生に入るとき `true` (= 動画の上に出す)
    /// - フルスクリーン解除時 `false` (= 通常 z-order に戻す。SSL Meter Pro 等の
    ///   ポップアップメニューが正しく動作するため非フルスクリーン中は TOPMOST 解除)
    ///
    /// **Codex P1 対応**: `SetWindowPos(HWND_TOPMOST/HWND_NOTOPMOST)` は呼び出し
    /// 順に z-order を上書きするため、ユーザーが手で並べた前後関係が壊れる。
    /// 切替前にデスクトップの top-to-bottom 順で plugin HWND の現在 z-order を
    /// snapshot し、切替後に bottom-to-top で再適用して元の前後関係を復元する。
    ///
    /// **Codex P3 対応**: `gui_topmost_desired` を更新することで、後から
    /// `show_slot_gui` で作る / 再表示する HWND にも自動的に同じ TOPMOST 状態が
    /// 適用される (= フルスクリーン中に VST ボタン 1 回目で plugin GUI を作る
    /// ケースでも見える状態になる)。
    pub fn set_all_guis_topmost(&self, topmost: bool) {
        // 希望状態を更新 (= 後続の show_slot_gui で参照される)
        self.gui_topmost_desired.store(topmost, Ordering::Release);

        // 対象 HWND を収集
        let target_hwnds: Vec<u64> = {
            let inner = self.inner.lock().unwrap();
            inner
                .slots
                .iter()
                .filter(|s| s.gui_hwnd != 0)
                .map(|s| s.gui_hwnd)
                .collect()
        };
        if target_hwnds.is_empty() {
            return;
        }

        // 現在の z-order を top-to-bottom で snapshot (= EnumWindows で desktop の
        // 前面順を走査し、target_hwnds に該当するものだけ拾う)。
        let ordered_top_to_bottom = gui::snapshot_z_order(&target_hwnds);

        // TOPMOST 切替後に bottom-to-top で SetWindowPos して元順序を復元。
        // bottom-to-top 順に呼ぶと、最後に呼んだ HWND が一番上に来るので、
        // ユーザーが見ていた順序がそのまま再現される。
        for hwnd in ordered_top_to_bottom.iter().rev() {
            gui::set_window_topmost(*hwnd, topmost);
        }
    }

    /// V キーハンドラ用: 「現在 1 個でも表示されているなら全て非表示」
    /// 「1 個も表示されていないなら全て表示」のトグル。
    /// 戻り値: 操作後の表示状態 (true = 全て表示)。
    pub fn toggle_all_guis(&self) -> bool {
        let any_visible = {
            let inner = self.inner.lock().unwrap();
            inner.slots.iter().any(|s| s.gui_visible)
        };
        let target = !any_visible;
        self.set_all_guis_visible(target);
        target
    }

    /// 毎 frame 呼ぶ: 全スロットの GUI close/resize シグナルを処理する。
    /// close 通知 (= ユーザーが × を押した) → そのスロットの GUI を **隠す** (= 永続 GuiHost
    /// 設計のため window/view は破棄しない)。
    /// resize 通知 → bridge に notify_host_resize を送る。
    pub fn pump_gui_signals(&self) {
        // close 通知の検出 (Mutex 内で全 slot を調べる)
        let mut close_targets: Vec<usize> = Vec::new();
        let mut resize_targets: Vec<(usize, u32, u32)> = Vec::new();
        let mut session_targets: Vec<(usize, bool)> = Vec::new();
        {
            let inner = self.inner.lock().unwrap();
            for (idx, slot) in inner.slots.iter().enumerate() {
                if let Some(arc) = slot.gui_close_signal.as_ref() {
                    if let Ok(guard) = arc.lock() {
                        if let Some(rx) = guard.as_ref() {
                            if matches!(rx.try_recv(), Ok(())) {
                                close_targets.push(idx);
                                continue;
                            }
                        }
                    }
                }
                if let Some(arc) = slot.gui_resize_signal.as_ref() {
                    if let Ok(guard) = arc.lock() {
                        if let Some(rx) = guard.as_ref() {
                            // 最新のリサイズだけ採用 (= drain しながら最後だけ覚える)
                            let mut latest: Option<(u32, u32)> = None;
                            while let Ok(size) = rx.try_recv() {
                                latest = Some(size);
                            }
                            if let Some((w, h)) = latest {
                                resize_targets.push((idx, w, h));
                            }
                        }
                    }
                }
                if let Some(arc) = slot.gui_resize_session_signal.as_ref() {
                    if let Ok(guard) = arc.lock() {
                        if let Some(rx) = guard.as_ref() {
                            // 直近の active 状態だけ採用 (= drain しながら最後だけ覚える)
                            let mut latest: Option<bool> = None;
                            while let Ok(active) = rx.try_recv() {
                                latest = Some(active);
                            }
                            if let Some(active) = latest {
                                session_targets.push((idx, active));
                            }
                        }
                    }
                }
            }
        }
        // close は Mutex 外で実施 (DspBridge::hide_slot_gui が再 lock するので)
        for idx in close_targets {
            self.hide_slot_gui(idx);
        }
        // session 切替は bridge に最初に伝える (= 後続の resize より先に状態確定)
        for (idx, active) in session_targets {
            let bridge_arc = {
                let inner = self.inner.lock().unwrap();
                inner.slots.get(idx).map(|s| s.bridge.clone())
            };
            if let Some(b) = bridge_arc {
                let _ = b.send(&Cmd::SetUserResizing { active: if active { 1 } else { 0 } });
            }
        }
        // resize は bridge に send (Mutex 外で bridge clone してから)
        for (idx, w, h) in resize_targets {
            let bridge_arc = {
                let inner = self.inner.lock().unwrap();
                inner.slots.get(idx).map(|s| s.bridge.clone())
            };
            if let Some(b) = bridge_arc {
                let _ = b.send(&Cmd::NotifyHostResize { width: w, height: h });
            }
        }
    }

    /// 指定 idx のスロットを削除する。bridge 子プロセスは shutdown される。
    /// **永続 GuiHost 設計**: ウィンドウが残っている場合はここで完全破棄する
    /// (= bridge にデタッチ命令 → GuiHost drop でウィンドウ destroy + thread quit)。
    pub fn remove_plugin(&self, idx: usize) {
        let mut inner = self.inner.lock().unwrap();
        if idx >= inner.slots.len() {
            return;
        }
        let mut slot = inner.slots.remove(idx);
        // GUI が作られていたら撤収
        if slot.gui_hwnd != 0 {
            // bridge にデタッチ命令 (= plugin view->removed())
            let _ = slot.bridge.send(&Cmd::HideGui);
            // GuiHost drop で Cmd::Quit が送られてスレッドが close_window 経由で
            // DestroyWindow → 自然 exit する (detach 方式)。
            if let Some(host) = slot.gui_host.take() {
                host.close();
            }
        }
        let _ = slot.bridge.shutdown_async();
        self.recalc_active_count(&inner);
    }

    /// スロットを `from` 位置から `to` 位置に移動する。to >= len なら末尾。
    pub fn move_plugin(&self, from: usize, to: usize) {
        let mut inner = self.inner.lock().unwrap();
        if from >= inner.slots.len() || from == to {
            return;
        }
        let slot = inner.slots.remove(from);
        let to = to.min(inner.slots.len());
        inner.slots.insert(to, slot);
    }

    /// 指定 idx の bypass フラグを設定する。
    pub fn set_bypass(&self, idx: usize, bypass: bool) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(slot) = inner.slots.get_mut(idx) {
            slot.bypass = bypass;
        }
        self.recalc_active_count(&inner);
    }

    /// 指定 idx の GUI HWND を更新する (UI ホスト側がウィンドウ作成 / 破棄したときに呼ぶ)。
    pub fn set_gui_hwnd(&self, idx: usize, hwnd_u64: u64) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(slot) = inner.slots.get_mut(idx) {
            slot.gui_hwnd = hwnd_u64;
        }
    }

    /// 指定 idx の bridge への参照を取得して fn を実行する。
    /// UI から GUI 関連コマンド (ShowGui/HideGui 等) を送るときに使う。
    /// `audio-pump` がロックを長時間握ることはないので blocking 待ちは ms オーダー。
    pub fn with_slot_bridge<F, R>(&self, idx: usize, f: F) -> Option<R>
    where
        F: FnOnce(&Bridge) -> R,
    {
        let inner = self.inner.lock().unwrap();
        inner.slots.get(idx).map(|s| f(&s.bridge))
    }

    /// 音声処理: チェーンの全アクティブスロットを順番に通す。
    /// `dst.len() == src.len()` が前提。
    pub fn process_block(&self, src: &[f32], dst: &mut [f32]) -> Result<(), String> {
        debug_assert_eq!(src.len(), dst.len());

        // ── ホットパス: スロットの Arc<Bridge> snapshot を Mutex 短時間保持で取る ──
        // process_block は audio-pump からのみ呼ばれるが、UI からの add/remove と
        // 競合する可能性があるので Mutex で snapshot を取る (= IPC roundtrip 中は
        // ロック解放済み)。
        let active_bridges: Vec<Arc<Bridge>> = {
            let mut inner = self.inner.lock().unwrap();
            // scratch バッファをこの timing で resize (audio frame サイズが変わる可能性)
            let n = src.len();
            if inner.scratch_a.len() != n {
                inner.scratch_a.resize(n, 0.0);
                inner.scratch_b.resize(n, 0.0);
            }
            inner
                .slots
                .iter()
                .filter(|s| !s.bypass && matches!(s.state, SlotState::Loaded))
                .map(|s| s.bridge.clone())
                .collect()
        };

        if active_bridges.is_empty() {
            dst.copy_from_slice(src);
            return Ok(());
        }

        // 単一プラグイン: 直接 src -> dst で処理 (scratch 不要)
        if active_bridges.len() == 1 {
            active_bridges[0]
                .push_audio(src)
                .map_err(|e| format!("push_audio: {e}"))?;
            let n = active_bridges[0]
                .pull_audio(dst, 100)
                .map_err(|e| format!("pull_audio: {e}"))?;
            if n < dst.len() {
                for o in &mut dst[n..] {
                    *o = 0.0;
                }
            }
            return Ok(());
        }

        // 複数プラグイン: scratch_a / scratch_b で ping-pong してから dst にコピー。
        // 短時間 Mutex 保持で scratch を切り出す (process_block は他に呼び手がいないので OK)
        let mut inner = self.inner.lock().unwrap();
        let scratch_a = std::mem::take(&mut inner.scratch_a);
        let scratch_b = std::mem::take(&mut inner.scratch_b);
        drop(inner);

        let mut buf = [scratch_a, scratch_b];
        let result = chain_process(&active_bridges, src, &mut buf, dst);

        // scratch を戻す (alloc を残して再利用)
        let mut inner = self.inner.lock().unwrap();
        let [a, b] = buf;
        inner.scratch_a = a;
        inner.scratch_b = b;
        drop(inner);

        result
    }

    /// active_slot_count atomic を再計算 (= Loaded かつ bypass=false なものの個数)。
    /// `inner` の Mutex を呼び出し側で保持している前提。
    fn recalc_active_count(&self, inner: &DspBridgeInner) {
        let count = inner
            .slots
            .iter()
            .filter(|s| !s.bypass && matches!(s.state, SlotState::Loaded))
            .count();
        self.active_slot_count.store(count, Ordering::Release);
    }
}

/// 複数プラグインを順番に通す。`buf[0]` / `buf[1]` を scratch として ping-pong。
/// 最終結果は `dst` に書き込む。
fn chain_process(
    bridges: &[Arc<Bridge>],
    src: &[f32],
    buf: &mut [Vec<f32>; 2],
    dst: &mut [f32],
) -> Result<(), String> {
    debug_assert!(bridges.len() >= 2);

    // 1: src -> buf[0] (plugin[0] が処理)
    bridges[0]
        .push_audio(src)
        .map_err(|e| format!("push_audio[0]: {e}"))?;
    let n = bridges[0]
        .pull_audio(&mut buf[0], 100)
        .map_err(|e| format!("pull_audio[0]: {e}"))?;
    if n < buf[0].len() {
        for o in &mut buf[0][n..] {
            *o = 0.0;
        }
    }

    // 2..N-1: buf[i%2] -> buf[(i+1)%2] (plugin[i] が処理)
    for i in 1..bridges.len() {
        let in_idx = (i - 1) % 2;
        // split_at_mut で buf[in_idx] と buf[1-in_idx] を別 borrow に
        let (input, output) = if in_idx == 0 {
            let (a, b) = buf.split_at_mut(1);
            (&a[0][..], &mut b[0][..])
        } else {
            let (a, b) = buf.split_at_mut(1);
            (&b[0][..], &mut a[0][..])
        };
        bridges[i]
            .push_audio(input)
            .map_err(|e| format!("push_audio[{i}]: {e}"))?;
        let n = bridges[i]
            .pull_audio(output, 100)
            .map_err(|e| format!("pull_audio[{i}]: {e}"))?;
        if n < output.len() {
            for o in &mut output[n..] {
                *o = 0.0;
            }
        }
    }

    // 最終: 最後に書き込まれた buf[(N-1)%2] を dst にコピー
    let final_idx = (bridges.len() - 1) % 2;
    dst.copy_from_slice(&buf[final_idx]);
    Ok(())
}

impl Drop for DspBridge {
    fn drop(&mut self) {
        self.disable();
    }
}
