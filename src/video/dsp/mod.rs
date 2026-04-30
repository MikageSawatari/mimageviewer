//! VST3 プラグインホスト bridge との連携モジュール (動画音声 DSP 経路)。
//!
//! 設計の全体像は [`docs/vst3-integration.md`] 参照。要点だけここに記す:
//!
//! - **C++ bridge プロセス** (`mimageviewer-vst3-host.exe`) を `include_bytes!` で
//!   メイン exe に埋め込み、初回 VST3 enable 時に
//!   `%APPDATA%\mimageviewer\vst3\` に展開する (PDFium / Susie ワーカーと同パターン)。
//! - **DspBridge** がアプリ起動から終了まで生存する singleton。プラグインは
//!   1 度だけロードし、動画切替で再ロードしない (= EQ カーブ等の状態が保持される)。
//! - 音声経路への結線は [`super::audio`] の audio-pump スレッドで行う。
//!   bridge IPC roundtrip のレイテンシ (~1-2ms) は AudioBuffer の depth (1.5s) で吸収する。
//!
//! v0.9.0 では **単一プラグイン**。チェーン (複数挿入) は将来拡張。

#![cfg(windows)]

pub mod bridge;
pub mod extract;
pub mod gui;
pub mod scanner;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

pub use bridge::{Bridge, Cmd, Event};
pub use scanner::{DiscoveredPlugin, default_vst3_paths, scan};

/// DSP bridge の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DspState {
    /// VST3 機能無効 (デフォルト)。bridge プロセスは起動しない。
    Disabled,
    /// bridge プロセスは起動済みだがプラグイン未ロード。
    Idle,
    /// プラグインロード中 (open コマンド送信〜loaded イベント受信前)。
    Loading,
    /// プラグインロード完了、音声処理可能。
    Loaded,
    /// エラー状態。再 enable で復旧。
    Error(&'static str),
}

/// DspBridge — VST3 プラグインホスト bridge との対話を管理する singleton。
///
/// アプリ起動から終了まで 1 個のインスタンスを保持する。
/// `Arc<DspBridge>` 化して audio-pump thread と UI thread から共有アクセス。
pub struct DspBridge {
    inner: Mutex<DspBridgeInner>,
    /// audio-pump thread が高速に判定するためのフラグ。Mutex を取らずに読める。
    enabled: AtomicBool,
    /// 現在ロード済みのプラグインのサンプルレート / ブロックサイズ。
    sample_rate: AtomicU32,
    block_size: AtomicU32,
}

struct DspBridgeInner {
    state: DspState,
    bridge: Option<Bridge>,
    plugin_name: Option<String>,
    plugin_path: Option<String>,
    latency_samples: u32,
    /// 直近 query_state で取得したプラグイン状態 (= IComponent::getState の chunk)。
    /// settings.json に Base64 で保存される。
    /// v0.9.0 では bridge 側未実装のため未使用 (将来 query_state プロトコル追加時に有効化)。
    #[allow(dead_code)]
    last_state: Option<Vec<u8>>,
}

impl DspBridge {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(DspBridgeInner {
                state: DspState::Disabled,
                bridge: None,
                plugin_name: None,
                plugin_path: None,
                latency_samples: 0,
                last_state: None,
            }),
            enabled: AtomicBool::new(false),
            sample_rate: AtomicU32::new(0),
            block_size: AtomicU32::new(0),
        })
    }

    /// `audio-pump` スレッドからのホットパスチェック。Mutex を取らない。
    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    pub fn state(&self) -> DspState {
        self.inner.lock().unwrap().state
    }

    pub fn plugin_name(&self) -> Option<String> {
        self.inner.lock().unwrap().plugin_name.clone()
    }

    pub fn plugin_path(&self) -> Option<String> {
        self.inner.lock().unwrap().plugin_path.clone()
    }

    pub fn latency_samples(&self) -> u32 {
        self.inner.lock().unwrap().latency_samples
    }

    /// VST3 機能を有効化する。bridge exe を APPDATA に展開して子プロセスを起動する。
    /// 既に enable 済みなら no-op。
    pub fn enable(&self) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        if matches!(inner.state, DspState::Idle | DspState::Loading | DspState::Loaded) {
            return Ok(());
        }
        let exe = extract::ensure_bridge_extracted()
            .map_err(|e| format!("bridge exe 展開失敗: {e}"))?;
        let bridge = Bridge::spawn(exe, |line| {
            crate::logger::log(format!("[vst3-bridge] {line}"));
        })
        .map_err(|e| format!("bridge spawn 失敗: {e}"))?;
        // hello を送って ready 待ち
        bridge
            .send(&Cmd::Hello { version: 1 })
            .map_err(|e| format!("hello send: {e}"))?;
        match bridge.recv() {
            Ok(Event::Ready { .. }) => {}
            Ok(other) => return Err(format!("予期しないイベント (ready 待ち): {other:?}")),
            Err(e) => return Err(format!("ready recv: {e}")),
        }
        inner.bridge = Some(bridge);
        inner.state = DspState::Idle;
        self.enabled.store(true, Ordering::Release);
        Ok(())
    }

    /// VST3 機能を無効化する。プラグインをアンロードして子プロセスも終了する。
    pub fn disable(&self) {
        self.enabled.store(false, Ordering::Release);
        let mut inner = self.inner.lock().unwrap();
        if let Some(mut bridge) = inner.bridge.take() {
            let _ = bridge.shutdown();
        }
        inner.state = DspState::Disabled;
        inner.plugin_name = None;
        inner.plugin_path = None;
        inner.latency_samples = 0;
        // last_state は維持 (= 次回 enable 時の復元に使う)
    }

    /// プラグインをロードする。既存ロード済みなら一旦 close してから再ロード。
    /// `restore_state` が Some なら ロード後に setState する (= 前回終了時の状態復元)。
    ///
    /// この関数は worker thread から呼ばれることを想定している。UI スレッドから直接
    /// 呼ぶと bridge 応答待ちでブロックする (~数百 ms 〜 数秒)。
    pub fn load_plugin(
        &self,
        plugin_path: &str,
        sample_rate: u32,
        block_size: u32,
        restore_state: Option<&[u8]>,
    ) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        if inner.bridge.is_none() {
            return Err("bridge が起動していません (enable が必要)".to_string());
        }

        // 既存プラグイン close — 借用 scope を分離
        let already_loaded = matches!(inner.state, DspState::Loaded | DspState::Loading);
        if already_loaded {
            let bridge = inner.bridge.as_ref().unwrap();
            let _ = bridge.send(&Cmd::Close);
            // closed イベントを待つ (timeout 簡略化)
            for _ in 0..10 {
                if let Ok(ev) = bridge.recv() {
                    if matches!(ev, Event::Closed) {
                        break;
                    }
                }
            }
        }

        inner.state = DspState::Loading;
        // open_audio_pipe は &mut Bridge が要る (= shm/event を Self に保存するため)
        {
            let bridge = inner.bridge.as_mut().unwrap();
            bridge
                .open_audio_pipe(plugin_path, sample_rate, block_size)
                .map_err(|e| format!("open_audio_pipe: {e}"))?;
        }

        // loaded イベントを待つ (typical < 1s、heavy plugin で数秒)
        loop {
            let ev_result = {
                let bridge = inner.bridge.as_ref().unwrap();
                bridge.recv()
            };
            match ev_result {
                Ok(Event::Loaded {
                    plugin_name,
                    latency_samples,
                }) => {
                    inner.plugin_name = Some(plugin_name);
                    inner.plugin_path = Some(plugin_path.to_string());
                    inner.latency_samples = latency_samples;
                    inner.state = DspState::Loaded;
                    break;
                }
                Ok(Event::Error { detail }) => {
                    inner.state = DspState::Error("load failed");
                    return Err(format!("プラグインロード失敗: {detail}"));
                }
                Ok(_) => continue, // latency_changed 等は無視
                Err(e) => {
                    inner.state = DspState::Error("recv failed");
                    return Err(format!("recv: {e}"));
                }
            }
        }

        self.sample_rate.store(sample_rate, Ordering::Release);
        self.block_size.store(block_size, Ordering::Release);

        // 状態復元 (= restore_state 機能は v0.9.0 では bridge 側未実装なので
        // 現時点では何もしない。query_state / restore_state コマンドを実装する
        // 段階で有効化する)。
        let _ = restore_state;

        Ok(())
    }

    /// 単発コマンド送信 → 1 イベント受信。GUI 表示などの同期 RPC 用。
    /// メインスレッドから呼ぶ前提 (= bridge 応答待ちが ms オーダーで OK)。
    pub fn send_recv(&self, cmd: &Cmd) -> Result<Event, String> {
        let inner = self.inner.lock().unwrap();
        let bridge = inner
            .bridge
            .as_ref()
            .ok_or_else(|| "bridge not running".to_string())?;
        bridge.send(cmd).map_err(|e| format!("send: {e}"))?;
        bridge.recv().map_err(|e| format!("recv: {e}"))
    }

    /// 単発コマンド送信のみ (応答を待たない)。リサイズ通知など fire-and-forget 用。
    pub fn send_oneway(&self, cmd: &Cmd) -> Result<(), String> {
        let inner = self.inner.lock().unwrap();
        let bridge = inner
            .bridge
            .as_ref()
            .ok_or_else(|| "bridge not running".to_string())?;
        bridge.send(cmd).map_err(|e| format!("send: {e}"))
    }

    /// 音声処理: in→out。samples は f32 packed stereo。
    /// `dst.len() == src.len()` でなければならない。
    /// ロード済みでない場合は src をそのまま dst にコピー (= bypass)。
    pub fn process_block(&self, src: &[f32], dst: &mut [f32]) -> Result<(), String> {
        debug_assert_eq!(src.len(), dst.len());
        let inner = self.inner.lock().unwrap();
        let bridge = match inner.bridge.as_ref() {
            Some(b) => b,
            None => {
                dst.copy_from_slice(src);
                return Ok(());
            }
        };
        if !matches!(inner.state, DspState::Loaded) {
            dst.copy_from_slice(src);
            return Ok(());
        }
        bridge.push_audio(src).map_err(|e| format!("push_audio: {e}"))?;
        // 100ms timeout: 通常 ms オーダーで返る。返らなければ bridge 側で詰まり
        let n = bridge
            .pull_audio(dst, 100)
            .map_err(|e| format!("pull_audio: {e}"))?;
        if n < dst.len() {
            // タイムアウト or 部分受信: 残りは silence
            for o in &mut dst[n..] {
                *o = 0.0;
            }
        }
        Ok(())
    }
}

impl Drop for DspBridge {
    fn drop(&mut self) {
        self.disable();
    }
}
