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

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use bridge::{Bridge, Cmd, Event};
pub use scanner::{DiscoveredPlugin, default_vst3_paths, scan, scan_with_audio_probe};

/// PDC (Plugin Delay Compensation) で許容する最大遅延 (秒)。
///
/// 用途:
/// - decoder pacing 側: `PACE_LEAD + min(pdc_latency, MAX_PDC_LATENCY_SECS)` で
///   先読み許可量を計算 (= plugin に未来 input を供給するため)
/// - 自動 bypass 側: 単一 plugin が報告した latency がこれを超えたら自動で bypass=true
///   に切り替え、警告ログを出す
///
/// 2.0 秒の根拠 (Codex 助言、2026-05-01):
/// - 実用的なプラグイン latency は数十ms〜数百ms (= linear-phase EQ で 100-200ms、
///   look-ahead リミッターで 5-50ms、plate reverb pre-delay で 数百ms)
/// - 1 秒を超える「構造的遅延」は PDC で動画クロックを遅らせる範囲としては大きすぎる
///   (= 動画再生開始時に音声出力までの待ち時間が長くなる)
/// - 共有メモリ ring (= 80ms) と AudioBuffer cap (= 300ms) のジッタ吸収余裕が
///   2 秒級では不足、underrun リスクが上がる
/// - 2 秒は実用シナリオを十分カバーする上限
pub const MAX_PDC_LATENCY_SECS: f64 = 2.0;

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
    /// `MAX_PDC_LATENCY_SECS` 超過で自動 bypass にされたか。UI 側で警告バッジ表示に使う。
    pub auto_bypassed_for_latency: bool,
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
    /// audio output の sample_rate (= cpal 出力レート)。`add_plugin` の 1 回目で
    /// 設定される (= 全 slot 同一)。UI で latency を ms 表示する際に使う。
    /// 0 = 未設定 (= プラグイン未追加状態)。
    sample_rate: AtomicU32,
}

struct DspBridgeInner {
    state: DspState,
    slots: Vec<PluginSlot>,
    /// 直近 `process_block` が使う scratch バッファ。Mutex 内に持たせて毎回 alloc を回避。
    /// 2 本必要なのは ping-pong 時のみだが、シンプルさのため常に 2 本持つ。
    scratch_a: Vec<f32>,
    scratch_b: Vec<f32>,
    /// 直近 hide した時点の z-order スナップショット (top-to-bottom 順 HWND リスト)。
    /// `set_all_guis_visible(false)` 直前に取得し、`set_all_guis_visible(true)` で
    /// 復元するために使う。これにより VST ボタン toggle で z-order が保たれる
    /// (= ユーザー報告 2026-04 「登録順に戻る」の対策)。
    last_z_order_snapshot: Vec<u64>,
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
    /// ユーザーが個別に GUI × で閉じたかどうか (= 一斉表示しても再表示しない希望)。
    /// パネルの「GUI ×」ボタンで true、「GUI」ボタンで false。
    /// `set_all_guis_visible(true)` (= VST ボタン全表示) は user_hidden=true のスロット
    /// を skip する (= ユーザー報告 2026-04 「個別に閉じたものは再表示しないで」)。
    pub user_hidden: bool,
    /// 自動 bypass が発動しているか (= latency が `MAX_PDC_LATENCY_SECS` 超過で
    /// `bypass=true` に切り替えられた状態)。UI 側で「上限超過のため自動 OFF」表示や
    /// 警告アイコンを出すために使う。settings には永続化しない (= ランタイム保護)。
    /// ユーザーが手動で再 ON にしてもまた latency が超過なら同じチェックで再発火する。
    pub auto_bypassed_for_latency: bool,
    /// 新規 GUI ウィンドウ作成時の初期位置 (左上、screen coordinate)。
    /// `add_plugin` で settings から渡された値を保持し、初回 `show_slot_gui` で
    /// CreateWindowExW に渡す。None なら OS 既定 (= CW_USEDEFAULT)。
    /// ウィンドウ作成後 (= gui_hwnd != 0) は HWND の現在位置が source of truth に
    /// なるので、この field は使われない (= 復元のための初期値専用)。
    pub desired_window_pos: Option<(i32, i32)>,
    /// プラグイン GUI ホスト (Win32 子ウィンドウスレッド)。slot 削除で自動終了する。
    pub gui_host: Option<gui::GuiHost>,
    /// ホストウィンドウの × ボタン押下シグナル。
    pub gui_close_signal: Option<Arc<Mutex<Option<std::sync::mpsc::Receiver<()>>>>>,
    /// ホストウィンドウのリサイズシグナル。
    pub gui_resize_signal: Option<Arc<Mutex<Option<std::sync::mpsc::Receiver<(u32, u32)>>>>>,
    /// Latest host-window resize waiting for throttled notify_host_resize delivery.
    pub pending_resize_notify: Option<(u32, u32)>,
    /// Last notify_host_resize send time for this slot.
    pub last_resize_notify: Option<Instant>,
    /// ホストウィンドウの WM_ENTERSIZEMOVE / WM_EXITSIZEMOVE シグナル
    /// (= ユーザー drag による resize/move session 開始 / 終了、Codex P4 対応)。
    pub gui_resize_session_signal: Option<Arc<Mutex<Option<std::sync::mpsc::Receiver<bool>>>>>,
}

impl DspBridge {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(DspBridgeInner {
                state: DspState::Disabled,
                slots: Vec::new(),
                scratch_a: Vec::new(),
                scratch_b: Vec::new(),
                last_z_order_snapshot: Vec::new(),
            }),
            enabled: AtomicBool::new(false),
            active_slot_count: AtomicUsize::new(0),
            gui_topmost_desired: AtomicBool::new(false),
            sample_rate: AtomicU32::new(0),
        })
    }

    /// audio output の sample_rate (= cpal 出力レート)。0 = 未設定。UI で
    /// プラグイン latency を ms に変換する際の分母として使う。
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Acquire)
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

    /// PDC (Plugin Delay Compensation) 用: アクティブな (Loaded && !bypass) スロットの
    /// `latency_samples` を合計して返す (= プラグインチェーン全体の構造的遅延、サンプル数)。
    ///
    /// audio-pump 側はこれを sample_rate で割って秒に変換し、AudioBuffer に同梱する。
    /// `fill_output` がそれを `clock.set_audio_pts` 時に減算することで、video clock を
    /// プラグインチェーン分遅らせ、A/V sync が保たれる仕組み。
    ///
    /// **呼び出し頻度**: pump push 毎 (= ~21ms 周期)。Mutex を取るが slots iter のみで
    /// 軽量。bypass 切替や latency 変化があれば次の push で自動追従する。
    ///
    /// **動的 latency 反映**: 各スロットの bridge から `cached_latency_samples_value()` を
    /// pull して、Loaded 時の値と異なれば slot.latency_samples を更新する。
    /// プラグインが UI でモード切替して `restartComponent(kLatencyChanged)` を発火すると、
    /// bridge プロセスがそれを検知し stdout で通知 → mIV 側 event-pump が atomic 更新 →
    /// このメソッドが次回呼ばれた時に slot に反映、UI も次のフレームで新しい値が見える。
    ///
    /// **自動 bypass (Codex 助言、2026-05-01 改訂)**:
    /// 2 段階のチェックを行う:
    ///
    /// 1. **個別 latency 更新時** (= `latest != s.latency_samples`): その slot 単独で
    ///    `MAX_PDC_LATENCY_SECS` 超過なら bypass する (= 単一 plugin が大きすぎる)。
    ///    ログ連打を避けるため値変化時のみ。
    ///
    /// 2. **合計 active total** が `MAX_PDC_LATENCY_SECS` を超えるなら、active slot の
    ///    うち latency_samples 最大のものを bypass する (= 合計が cap 以下になるまで loop)。
    ///    複数 plugin の合計が原因のケース (= 個別はどれも < 2s だが合計 > 2s) に対応。
    ///    既に `auto_bypassed_for_latency && bypass` の slot にはログを再発火しない。
    ///
    /// settings には永続化しない (= ランタイム保護、再起動後は再評価される)。
    pub fn total_latency_samples(&self) -> u32 {
        let sr = self.sample_rate.load(Ordering::Acquire);
        let max_samples: u32 = if sr > 0 {
            (MAX_PDC_LATENCY_SECS * sr as f64) as u32
        } else {
            (MAX_PDC_LATENCY_SECS * 48_000.0) as u32 // 起動直後 fallback
        };

        let mut inner = self.inner.lock().unwrap();
        let mut active_changed = false;

        // ── Step 1: 個別 latency 更新の反映 + 単独超過の auto-bypass ──
        for s in inner.slots.iter_mut() {
            if !matches!(s.state, SlotState::Loaded) {
                continue;
            }
            let latest = s.bridge.cached_latency_samples_value();
            if latest != u32::MAX && latest != s.latency_samples {
                crate::logger::log(format!(
                    "[VST3 PDC] slot latency_samples updated: '{}' {} -> {}",
                    s.plugin_name.as_deref().unwrap_or("?"),
                    s.latency_samples,
                    latest
                ));
                s.latency_samples = latest;
                // 個別 plugin 単独で上限超過 → 即 bypass
                if latest > max_samples && !s.bypass {
                    crate::logger::log(format!(
                        "[VST3 PDC] AUTO-BYPASS (individual): '{}' latency {} samples ({:.1}ms) \
                         exceeds {:.1}s cap.",
                        s.plugin_name.as_deref().unwrap_or("?"),
                        latest,
                        latest as f64 / sr.max(1) as f64 * 1000.0,
                        MAX_PDC_LATENCY_SECS,
                    ));
                    s.bypass = true;
                    s.auto_bypassed_for_latency = true;
                    active_changed = true;
                }
            }
        }

        // ── Step 2: active 合計超過チェック + 最大 latency slot の auto-bypass loop ──
        // 個別では cap 内でも、合計が超えるケース (例: 1973ms + 50ms = 2023ms) に対応。
        // 合計が cap 以下になるまで、active で最大 latency の slot を bypass し続ける。
        loop {
            let total: u32 = inner
                .slots
                .iter()
                .filter(|s| !s.bypass && matches!(s.state, SlotState::Loaded))
                .map(|s| s.latency_samples)
                .fold(0u32, |a, b| a.saturating_add(b));
            if total <= max_samples {
                break;
            }
            // active で最大 latency の slot を 1 つ bypass
            let target_idx = inner
                .slots
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.bypass && matches!(s.state, SlotState::Loaded))
                .max_by_key(|(_, s)| s.latency_samples)
                .map(|(i, _)| i);
            let Some(idx) = target_idx else {
                break; // active slot が無いのに total > max は通常起きない、防御
            };
            let slot = &mut inner.slots[idx];
            // 既に auto-bypass 済の slot にはログ再発火しない (= ログ連打防止)
            let already_logged = slot.auto_bypassed_for_latency;
            slot.bypass = true;
            slot.auto_bypassed_for_latency = true;
            active_changed = true;
            if !already_logged {
                let total_ms = total as f64 / sr.max(1) as f64 * 1000.0;
                let this_ms = slot.latency_samples as f64 / sr.max(1) as f64 * 1000.0;
                crate::logger::log(format!(
                    "[VST3 PDC] AUTO-BYPASS (total): chain total {:.1}ms exceeds {:.1}s cap, \
                     disabling largest active plugin '{}' ({:.1}ms).",
                    total_ms,
                    MAX_PDC_LATENCY_SECS,
                    slot.plugin_name.as_deref().unwrap_or("?"),
                    this_ms,
                ));
            }
            // loop 継続: bypass 後の total を再計算
        }

        // active_slot_count atomic を更新
        if active_changed {
            let count = inner
                .slots
                .iter()
                .filter(|s| !s.bypass && matches!(s.state, SlotState::Loaded))
                .count();
            self.active_slot_count.store(count, Ordering::Release);
        }

        // 最終 total を返す
        inner
            .slots
            .iter()
            .filter(|s| !s.bypass && matches!(s.state, SlotState::Loaded))
            .map(|s| s.latency_samples)
            .fold(0u32, |a, b| a.saturating_add(b))
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
                auto_bypassed_for_latency: s.auto_bypassed_for_latency,
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
            auto_bypassed_for_latency: s.auto_bypassed_for_latency,
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
        extract::ensure_bridge_extracted().map_err(|e| format!("bridge exe 展開失敗: {e}"))?;
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
        user_hidden: bool,
        initial_state: Option<&str>,
        initial_window_pos: Option<(i32, i32)>,
    ) -> Result<usize, String> {
        // enable 状態チェック (Mutex を保持しない)
        if !self.is_enabled() {
            return Err("VST3 が無効化されています (enable を先に)".to_string());
        }

        // bridge exe path
        let exe =
            extract::ensure_bridge_extracted().map_err(|e| format!("bridge exe 展開失敗: {e}"))?;

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
        // initial_state は `open_audio_pipe` → `Cmd::Open` に bake され、bridge 側で
        // **audio_thread 起動前**に setState される (= race-free auto-restore、Codex P2-3)。
        bridge
            .open_audio_pipe(plugin_path, sample_rate, block_size, initial_state)
            .map_err(|e| format!("open_audio_pipe: {e}"))?;

        // loaded イベントを待つ (= bridge は state 復元 + setActive 完了後に Loaded を返す)
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
        // PDC 診断: プラグインがレポートした latency をログに残す。
        crate::logger::log(format!(
            "[VST3 PDC] plugin loaded: '{}' latency_samples={} ({:.3}ms@{}Hz)",
            plugin_name,
            latency_samples,
            latency_samples as f64 / sample_rate as f64 * 1000.0,
            sample_rate,
        ));

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

        // sample_rate を DspBridge に保存 (= UI 表示用、初回 add_plugin で設定)
        self.sample_rate.store(sample_rate, Ordering::Release);

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
            user_hidden,
            auto_bypassed_for_latency: false,
            desired_window_pos: initial_window_pos,
            gui_host: None,
            gui_close_signal: None,
            gui_resize_signal: None,
            pending_resize_notify: None,
            last_resize_notify: None,
            gui_resize_session_signal: None,
        });
        self.recalc_active_count(&inner);
        Ok(idx)
    }

    /// 全 Loaded スロットのプラグイン内部状態 (= EQ カーブ / chunk) を **並列に** query する。
    /// 戻り値: `(plugin_path, base64_state)` のリスト。失敗した slot は **含めない**
    /// (= 呼出側は path 検索で既存 state を上書きするだけ、失敗 slot の state は保持される)。
    ///
    /// **path をキーにする**: `settings.vst3_plugins` と bridge slots は load 失敗で
    /// index がズレうるため (Codex P2、2026-05-01)。path 一意性は preferences が保証。
    ///
    /// **並列実行**: 各 bridge への IPC は独立 (= 別子プロセス + 別 stdin/stdout +
    /// 別 shm) なので、N 個の `query_state_sync` を並列に走らせて total wait を
    /// `N × 1秒` から `max(1秒)` に短縮する。チェーン上限 10 個で worst case ~1 秒。
    /// shutdown / settings.save 経由で呼ばれるので UI 同期 hot-path には乗らない。
    /// bypass の slot も query する (= 内部 state はあるので保存対象)。
    pub fn snapshot_all_plugin_states(&self) -> Vec<(String, String)> {
        if !self.is_enabled() {
            return Vec::new();
        }
        let bridges: Vec<(String, Arc<Bridge>)> = {
            let inner = self.inner.lock().unwrap();
            inner
                .slots
                .iter()
                .filter(|s| matches!(s.state, SlotState::Loaded))
                .map(|s| (s.plugin_path.clone(), s.bridge.clone()))
                .collect()
        };
        if bridges.is_empty() {
            return Vec::new();
        }
        let timeout = std::time::Duration::from_secs(1);
        // 各 bridge ごとに thread を spawn して query_state_sync を並列実行。
        // bridge は Arc なのでクローンで thread に渡せる。
        let handles: Vec<std::thread::JoinHandle<(String, Result<String, String>)>> = bridges
            .into_iter()
            .map(|(path, b)| {
                std::thread::spawn(move || {
                    let result = b.query_state_sync(timeout);
                    (path, result)
                })
            })
            .collect();
        let mut out = Vec::with_capacity(handles.len());
        for h in handles {
            match h.join() {
                Ok((path, Ok(state))) => out.push((path, state)),
                Ok((path, Err(e))) => crate::logger::log(format!(
                    "[VST3] snapshot_all_plugin_states: query failed for '{}': {e}",
                    path,
                )),
                Err(_) => crate::logger::log(
                    "[VST3] snapshot_all_plugin_states: worker thread panicked".to_string(),
                ),
            }
        }
        out
    }

    /// 全 Loaded スロットのプラグイン GUI ウィンドウ位置 + 外枠サイズを取得する。
    /// 戻り値: `(plugin_path, x, y, w, h)` のリスト。HWND が無い slot
    /// (= 一度も GUI を開かなかった) や `GetWindowRect` 失敗の slot は含めない。
    ///
    /// **path をキーにする**: settings との突合せ用 (= bridge slot idx と settings idx の
    /// ズレを避ける、Codex P2、2026-05-01)。
    /// 軽量同期処理で UI スレッドから呼んで OK (= GetWindowRect は ~us)。
    pub fn snapshot_all_window_positions(&self) -> Vec<(String, i32, i32, u32, u32)> {
        if !self.is_enabled() {
            return Vec::new();
        }
        let inner = self.inner.lock().unwrap();
        inner
            .slots
            .iter()
            .filter(|s| s.gui_hwnd != 0)
            .filter_map(|s| {
                gui::get_window_rect(s.gui_hwnd)
                    .map(|(x, y, w, h)| (s.plugin_path.clone(), x, y, w, h))
            })
            .collect()
    }

    /// シーク時の同期 reset fence (= Codex 助言、2026-05-01):
    /// **アクティブ (= !bypass && Loaded)** プラグインに `Cmd::Reset` を送り、
    /// 各 bridge が `ResetDone` ack を返すまで待つ。
    ///
    /// bridge 側の挙動:
    /// 1. control thread が `reset_pending_` flag を立てる
    /// 2. audio thread が loop 先頭で flag を見て、in/out ring を drain + plugin reset
    ///    + ResetDone を返す (= process と setProcessing を同 thread で直列化、race 排除)
    ///
    /// mIV 側の効果:
    /// - シーク前 audio (= bridge in_ring + out_ring + plugin delay-line) を完全 flush
    /// - post-seek audio が plugin に流れる時点で plugin は zero state (= 正しい新規再生)
    ///
    /// **呼び出しタイミング**: pump thread から「新 seek 世代の最初の post-seek frame を
    /// process_block する直前」に呼ぶ。これにより post-seek input は reset 後の plugin で
    /// 処理される。
    ///
    /// **active filter** (Codex P2-1, 2026-05-01): bypass=true の slot は `process_block`
    /// チェーンに入っていないので reset 不要。`process_block()` / `total_latency_samples()`
    /// と同じ active 判定 (= !bypass && Loaded) で filter する。これで bypass 状態の slot
    /// (= 例 auto-bypass された plugin) で無駄な reset を走らせないので、シーク 1 回あたりの
    /// 総 reset 時間が短縮される。
    ///
    /// **timeout** (Codex P2-2, 2026-05-01): 2.0 秒に延長。
    /// 内訳: bridge audio thread `read_in_available` 100ms timeout + reset 処理 数 ms +
    /// `flush_with_silence` (= 最大 ~200ms@2s latency) + IPC roundtrip + 安全余裕。
    /// timeout 時は **CRITICAL log** を出して return (= 後続 process_block は走るので、
    /// pre-seek tail が一瞬漏れる可能性あり、log でユーザーが気付ける)。
    /// 完全な fence-fail (= 該当 bridge を mute or 一時 bypass) は将来課題 (P2-2 残)。
    pub fn reset_plugins_sync(&self) {
        if !self.is_enabled() {
            return;
        }
        let bridges: Vec<Arc<Bridge>> = {
            let inner = self.inner.lock().unwrap();
            inner
                .slots
                .iter()
                .filter(|s| !s.bypass && matches!(s.state, SlotState::Loaded))
                .map(|s| s.bridge.clone())
                .collect()
        };
        if bridges.is_empty() {
            return;
        }
        // 各 bridge ごとに `reset_sync` (= ID 付き send + ack 照合 wait) を呼ぶ。
        // 順次実行で十分 (= active bridge 数 max 10、各 reset は数 ms-数百 ms)。
        let timeout = std::time::Duration::from_secs(2);
        for b in &bridges {
            if !b.reset_sync(timeout) {
                crate::logger::log(
                    "[VST3] CRITICAL: reset_plugins_sync ResetDone ack timeout (2s), \
                     pre-seek audio may leak briefly. Plugin may be unresponsive or \
                     processing too slowly."
                        .to_string(),
                );
            }
        }
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
    /// 新規ウィンドウ作成時の初期位置は `slot.desired_window_pos` を使う
    /// (= settings から復元した値 / 終了時に保存した値、`add_plugin` で初期化、
    /// 2026-05 ユーザー要望「ウィンドウ位置を復元してほしい」)。
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
                        s2.user_hidden = false; // 明示的に show したので user_hidden 解除
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
            Ok(Event::GuiSize {
                width,
                height,
                resizable,
            }) => (width, height, resizable),
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
        // 初期位置は slot に保持されている `desired_window_pos` を使う (= settings から
        // 復元した値、または前回終了時の値)。None なら OS 既定 (= 中央付近) で開く。
        let initial_pos = {
            let inner = self.inner.lock().unwrap();
            inner.slots.get(idx).and_then(|s| s.desired_window_pos)
        };
        let gui_host = gui::GuiHost::spawn();
        let reply = gui_host
            .show(&plugin_name, pref_w, pref_h, resizable, initial_pos)
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
            // 起動後 settings から復元された user_hidden=true 状態でも、明示的な
            // show_slot_gui (= パネル「GUI」ボタン押下) で初めて作成された window
            // なので user_hidden を解除する (= 既存 hwnd!=0 の早期 return path と同様)。
            slot.user_hidden = false;
            slot.gui_host = Some(gui_host);
            slot.gui_close_signal = Some(reply.close_signal);
            slot.gui_resize_signal = Some(reply.resize_signal);
            slot.pending_resize_notify = None;
            slot.last_resize_notify = None;
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

    /// 指定 idx のプラグイン GUI を **隠す** (= 一括 hide / 内部呼出用)。
    /// `user_hidden` フラグは触らない。VST 全体トグル / フルスクリーン解除等の
    /// 暗黙 hide パスで使う。
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

    /// 指定 idx のプラグイン GUI をユーザーが明示的に閉じた。
    /// `user_hidden = true` をセットし、以降の `set_all_guis_visible(true)` (=
    /// VST ボタン全表示) では表示しない (= ユーザー報告 2026-04 「個別に閉じた
    /// ものは再表示しないでほしい」)。
    pub fn user_hide_slot_gui(&self, idx: usize) {
        let hwnd = {
            let mut inner = self.inner.lock().unwrap();
            if let Some(slot) = inner.slots.get_mut(idx) {
                slot.gui_visible = false;
                slot.user_hidden = true;
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

    /// 全プラグイン GUI を表示/非表示一斉トグル (= VST ボタン)。
    ///
    /// **z-order 保持** (Codex P1 / ユーザー報告 2026-04):
    /// - hide 直前に **現在の z-order を snapshot**
    /// - show 後に snapshot を bottom-to-top で SetWindowPos 復元
    /// これでユーザーが手で並べた前後関係が VST トグル後も保たれる。
    ///
    /// **per-slot user_hidden 尊重**: スロットごとの `user_hidden` フラグが
    /// true のスロットは、VST 全体トグルで show=true にしても表示しない
    /// (= ユーザーが個別に GUI × した状態を尊重)。
    pub fn set_all_guis_visible(&self, target_visible: bool) {
        if !target_visible {
            // ── hide 経路: 現 z-order を snapshot してから全 hide ──
            let target_hwnds: Vec<u64> = {
                let inner = self.inner.lock().unwrap();
                inner
                    .slots
                    .iter()
                    .filter(|s| s.gui_hwnd != 0 && s.gui_visible)
                    .map(|s| s.gui_hwnd)
                    .collect()
            };
            let snapshot = if target_hwnds.is_empty() {
                Vec::new()
            } else {
                gui::snapshot_z_order(&target_hwnds)
            };
            let n = {
                let mut inner = self.inner.lock().unwrap();
                inner.last_z_order_snapshot = snapshot;
                inner.slots.len()
            };
            for idx in 0..n {
                self.hide_slot_gui(idx);
            }
        } else {
            // ── show 経路: 全 SW_SHOWNA → snapshot 順序で z-order 復元 ──
            // user_hidden=true のスロットは飛ばす。
            enum ShowAction {
                Skip,
                BatchExisting(u64),
                Create,
            }

            let n = {
                let inner = self.inner.lock().unwrap();
                inner.slots.len()
            };
            let mut shown_hwnds: Vec<u64> = Vec::with_capacity(n);
            for idx in 0..n {
                let action = {
                    let mut inner = self.inner.lock().unwrap();
                    match inner.slots.get_mut(idx) {
                        Some(slot)
                            if !slot.user_hidden && matches!(slot.state, SlotState::Loaded) =>
                        {
                            if slot.gui_hwnd != 0 {
                                // Existing HWNDs are shown only by the final DeferWindowPos batch,
                                // avoiding a visible intermediate z-order before restoration.
                                slot.gui_visible = true;
                                slot.user_hidden = false;
                                ShowAction::BatchExisting(slot.gui_hwnd)
                            } else {
                                ShowAction::Create
                            }
                        }
                        _ => ShowAction::Skip,
                    }
                };

                match action {
                    ShowAction::Skip => {}
                    ShowAction::BatchExisting(hwnd) => shown_hwnds.push(hwnd),
                    ShowAction::Create => {
                        let _ = self.show_slot_gui(idx);
                        let hwnd = {
                            let inner = self.inner.lock().unwrap();
                            inner.slots.get(idx).map(|s| s.gui_hwnd).unwrap_or(0)
                        };
                        if hwnd != 0 {
                            shown_hwnds.push(hwnd);
                        }
                    }
                }
            }
            // ── z-order 復元: snapshot を bottom-to-top で SetWindowPos ──
            // snapshot に含まれる HWND だけ対象 (= user_hidden で skip した
            // スロットや、新規に追加されて snapshot に無いものは触らない)。
            let snapshot: Vec<u64> = {
                let inner = self.inner.lock().unwrap();
                inner.last_z_order_snapshot.clone()
            };
            let shown_set: std::collections::HashSet<u64> = shown_hwnds.iter().copied().collect();
            let mut restore_order: Vec<u64> = snapshot
                .into_iter()
                .filter(|hwnd| shown_set.contains(hwnd))
                .collect();
            for hwnd in shown_hwnds {
                if !restore_order.contains(&hwnd) {
                    restore_order.push(hwnd);
                }
            }
            if !restore_order.is_empty() {
                let topmost = self.gui_topmost_desired.load(Ordering::Acquire);
                gui::show_windows_in_z_order(&restore_order, topmost);
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
    ///
    /// 戻り値: `user_hidden=true` に切り替わった (= ユーザーが × で閉じた) スロットの
    /// **plugin_path 一覧**。呼出側 (App) はこれで `settings.vst3_plugins` を path 検索
    /// して `user_hidden` を反映する。idx を返さないのは bridge slots と
    /// `settings.vst3_plugins` で index がズレる (= ロード失敗で詰まる) ため
    /// (Codex P2 2026-05-01)。
    pub fn pump_gui_signals(&self) -> Vec<String> {
        // close 通知の検出 (Mutex 内で全 slot を調べる)
        let mut close_targets: Vec<usize> = Vec::new();
        let mut resize_targets: Vec<(usize, u32, u32)> = Vec::new();
        let mut session_targets: Vec<(usize, bool)> = Vec::new();
        let now = Instant::now();
        let resize_interval = Duration::from_millis(33);
        {
            let mut inner = self.inner.lock().unwrap();
            for (idx, slot) in inner.slots.iter_mut().enumerate() {
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
                            if let Some(size) = latest {
                                slot.pending_resize_notify = Some(size);
                            }
                        }
                    }
                }
                if let Some((w, h)) = slot.pending_resize_notify {
                    let ready = slot
                        .last_resize_notify
                        .map(|last| now.duration_since(last) >= resize_interval)
                        .unwrap_or(true);
                    if ready {
                        slot.pending_resize_notify = None;
                        slot.last_resize_notify = Some(now);
                        resize_targets.push((idx, w, h));
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
        // close は Mutex 外で実施 (DspBridge::user_hide_slot_gui が再 lock するので)。
        // **user_hide_slot_gui を使う**: プラグインウィンドウの × ボタン押下は
        // ユーザーの明示的な意図 = `user_hidden=true` をセットして以降の VST 全表示
        // トグルでも復活しないようにする (= ユーザー報告 2026-04 「× で閉じた状態を
        // 記憶したい」)。
        // path は user_hide_slot_gui で slot を変更する **前** に snapshot で取得する
        // (= 設計上は user_hide_slot_gui で path は変わらないが、idx の有効性も含めて
        // ここで一気に取った方が安全)。
        let mut user_hidden_paths: Vec<String> = Vec::with_capacity(close_targets.len());
        {
            let inner = self.inner.lock().unwrap();
            for &idx in &close_targets {
                if let Some(slot) = inner.slots.get(idx) {
                    user_hidden_paths.push(slot.plugin_path.clone());
                }
            }
        }
        for &idx in &close_targets {
            self.user_hide_slot_gui(idx);
        }
        // session 切替は bridge に最初に伝える (= 後続の resize より先に状態確定)
        for (idx, active) in session_targets {
            let bridge_arc = {
                let inner = self.inner.lock().unwrap();
                inner.slots.get(idx).map(|s| s.bridge.clone())
            };
            if let Some(b) = bridge_arc {
                let _ = b.send(&Cmd::SetUserResizing {
                    active: if active { 1 } else { 0 },
                });
            }
        }
        // resize は bridge に send (Mutex 外で bridge clone してから)
        for (idx, w, h) in resize_targets {
            let bridge_arc = {
                let inner = self.inner.lock().unwrap();
                inner.slots.get(idx).map(|s| s.bridge.clone())
            };
            if let Some(b) = bridge_arc {
                let _ = b.send(&Cmd::NotifyHostResize {
                    width: w,
                    height: h,
                });
            }
        }
        user_hidden_paths
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
        let sr = self.sample_rate.load(Ordering::Acquire);
        let max_samples: u32 = if sr > 0 {
            (MAX_PDC_LATENCY_SECS * sr as f64) as u32
        } else {
            (MAX_PDC_LATENCY_SECS * 48_000.0) as u32
        };

        let mut inner = self.inner.lock().unwrap();
        if let Some(slot) = inner.slots.get_mut(idx) {
            // 手動 ON 時の事前チェック (= Codex P1-G、2026-05-01):
            // この slot を ON にして active total が MAX_PDC_LATENCY_SECS を
            // 超えるなら refuse (= bypass=true 維持) + auto_bypassed_for_latency=true。
            // ユーザーは UI 上で「ON にしようとしたが上限超過のため OFF のまま」と認識できる。
            if !bypass && slot.bypass {
                let this_latency = slot.latency_samples;
                let other_active_total: u32 = inner
                    .slots
                    .iter()
                    .enumerate()
                    .filter(|(i, s)| *i != idx && !s.bypass && matches!(s.state, SlotState::Loaded))
                    .map(|(_, s)| s.latency_samples)
                    .sum();
                let would_be_total = other_active_total.saturating_add(this_latency);
                if would_be_total > max_samples {
                    let plugin_name = inner
                        .slots
                        .get(idx)
                        .and_then(|s| s.plugin_name.clone())
                        .unwrap_or_else(|| "(?)".to_string());
                    let this_ms = this_latency as f64 / sr.max(1) as f64 * 1000.0;
                    let total_ms = would_be_total as f64 / sr.max(1) as f64 * 1000.0;
                    crate::logger::log(format!(
                        "[VST3 PDC] REFUSE manual ON: '{}' ({:.1}ms) would push total to {:.1}ms (cap {:.1}s). Reduce other plugin latencies or this plugin first.",
                        plugin_name, this_ms, total_ms, MAX_PDC_LATENCY_SECS,
                    ));
                    if let Some(s) = inner.slots.get_mut(idx) {
                        s.bypass = true; // refuse: 維持
                        s.auto_bypassed_for_latency = true;
                    }
                    self.recalc_active_count(&inner);
                    return;
                }
            }
            // 通常経路: bypass を反映、ON に戻すなら auto-bypass フラグもクリア
            if let Some(s) = inner.slots.get_mut(idx) {
                s.bypass = bypass;
                if !bypass {
                    s.auto_bypassed_for_latency = false;
                }
            }
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
                .process_audio_blocking(src, dst, 100)
                .map_err(|e| format!("process_audio: {e}"))?;
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
        .process_audio_blocking(src, &mut buf[0], 100)
        .map_err(|e| format!("process_audio[0]: {e}"))?;

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
            .process_audio_blocking(input, output, 100)
            .map_err(|e| format!("process_audio[{i}]: {e}"))?;
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
