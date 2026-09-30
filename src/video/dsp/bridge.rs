//! C++ bridge プロセス (`mimageviewer-vst3-host.exe`) との IPC ラッパー。
//!
//! - 子プロセス起動 + stdin/stdout を握る
//! - length-prefixed UTF-8 JSON で制御メッセージを send/recv
//! - shared memory + 2 本の Windows named event で音声バッファを送受信
//!
//! Phase A 移植時はこのモジュールを `src/video/dsp/bridge.rs` にそのまま
//! コピーするのが目的。なので mIV 本体の依存 (logger, settings 等) を
//! 持ち込まない。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
#[cfg(windows)]
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
#[cfg(windows)]
use windows::Win32::System::Threading::{
    CreateEventW, GetExitCodeProcess, SetEvent, TerminateProcess, WaitForSingleObject,
};
#[cfg(windows)]
use windows::core::{HSTRING, PCWSTR};

/// T09 (v0.9.0): bridge と Rust 側の **IPC プロトコルバージョン**。`crates/vst3-host/
/// include/protocol.h` の `PROTOCOL_VERSION` と必ず一致させること。version mismatch は
/// hello/ready のハンドシェイクで両端から検出して握りつぶさない。
///
/// **bump 1 → 2** (T09 round 4): 旧 bridge は version 比較を no-op で握り潰していたので
/// 1 のままだと stale bridge を検出できなかった。2 へ上げることで v0.8.x 以前の
/// `mimageviewer-vst3-host.exe` (version=1 を返すだけ) を新 Rust 側で reject できる。
/// v5: checked GUI-thread presentation outcomes and shared suppression epochs.
pub const PROTOCOL_VERSION: u32 = 5;
pub(crate) const STATE_WATCHDOG_EXIT_CODE: u32 = 0xEFFE_C001;

static NEXT_AUDIO_PIPE_ID: AtomicU64 = AtomicU64::new(0);

/// shared memory header — C++ 側 `crates/vst3-host/include/protocol.h::ShmHeader` と
/// **同一バイナリレイアウト**でなければならない (cache line aligned 64 byte)。
#[repr(C)]
pub struct ShmHeader {
    // alignas(64) を 8 個分 padding 込みで再現 (= 1 個 64 byte で 5 個)。
    // Rust 側で AtomicU32 を使う場合、align は 4 だが C++ 側の 64 byte 揃えに合わせるため
    // 明示的に 64 byte 単位で構造体を配置する。
    pub _pad0: [u8; 0],
    pub in_write: AtomicU32,
    pub _pad1: [u8; 60],
    pub in_read: AtomicU32,
    pub _pad2: [u8; 60],
    pub out_write: AtomicU32,
    pub _pad3: [u8; 60],
    pub out_read: AtomicU32,
    pub _pad4: [u8; 60],
    pub capacity: u32,
    pub channels: u32,
    pub sample_rate: u32,
    pub block_size: u32,
}

/// ホスト側 (= tester) で確保する shared memory サイズを計算する。
/// header + in_ring (capacity * 4 bytes) + out_ring (capacity * 4 bytes)
pub fn shm_size_bytes(capacity_samples: u32) -> u64 {
    std::mem::size_of::<ShmHeader>() as u64 + (capacity_samples as u64) * 8
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd")]
#[serde(rename_all = "lowercase")]
pub enum Cmd {
    Hello {
        version: u32,
    },
    Open {
        plugin_path: String,
        sample_rate: u32,
        block_size: u32,
        shm_name: String,
        shm_size: u64,
        sig_in: String,
        sig_out: String,
        /// 起動時に復元したいプラグイン内部状態 (= base64 chunk)。bridge は
        /// `IComponent::setState` を **`audio_thread` 起動前**・control thread 上で
        /// 適用する (= Codex P2-3 race 解消、2026-05-01)。空または None なら no-op。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
        strict_state: u32,
    },
    /// プラグインを短時間だけロードし、audio/event bus 数を取得する。
    /// 環境設定のスキャンで「音声入力を受け取れない VST3」を候補から外すために使う。
    Probe {
        plugin_path: String,
    },
    /// シーク等で plugin の内部状態を flush する。`reset_id` は generation ID で、
    /// stale ack race を防ぐ (= timeout した過去 reset の ack が次回成功と誤認されない)。
    /// bridge は `Event::ResetDone { reset_id }` で同じ ID を返す。
    Reset {
        reset_id: u64,
    },
    Close,
    Shutdown,
    /// プラグイン GUI を指定 HWND にアタッチする。
    #[serde(rename = "show_gui")]
    ShowGui {
        hwnd: u64,
        visible: u32,
    },
    /// プラグイン GUI を外す。HWND の破棄は host 側の責務。
    #[serde(rename = "hide_gui")]
    HideGui,
    /// Already-attached GUI surface visibility toggle. The bridge keeps the
    /// VST3 view attached and only hides/shows its bridge-owned top-level
    /// surface.
    #[serde(rename = "set_gui_visible")]
    SetGuiVisible {
        visible: u32,
    },
    /// Already-attached bridge-owned GUI surface topmost toggle. Rust owns the
    /// host HWND, while the bridge owns the actual plugin surface HWND.
    #[serde(rename = "set_gui_topmost")]
    SetGuiTopmost {
        topmost: u32,
    },
    /// Application foreground state relay for bridge-owned GUI surfaces.
    /// When mIV loses activation, the bridge hides plugin surfaces so they do
    /// not float above the foreground application.
    #[serde(rename = "set_gui_app_active")]
    SetGuiAppActive {
        active: u32,
    },
    /// プラグインの推奨 GUI サイズだけ取得する (= attached しない)。
    /// host はこのサイズでウィンドウを作ってから ShowGui を送ることで、
    /// プラグインが子ウィンドウを正しいサイズで作成できる。
    #[serde(rename = "query_gui_size")]
    QueryGuiSize,
    /// ホストウィンドウのリサイズが起きたことをプラグインに通知する。
    /// bridge は view->onSize(rect) を呼んでプラグインの子ウィンドウを追従させる。
    #[serde(rename = "notify_host_resize")]
    NotifyHostResize {
        width: u32,
        height: u32,
    },
    /// 診断用: bridge 内で plugin を経由せずに in→out 単純コピーする。
    /// 歪みが消えれば plugin process が原因、残れば bridge パイプラインが原因。
    #[serde(rename = "set_passthrough")]
    SetPassthrough {
        enable: u32,
    },
    /// ユーザー drag によるリサイズが進行中かを bridge に通知する。
    /// `active=true` 中、bridge は plugin の `resizeView` callback で host HWND
    /// への `SetWindowPos` をスキップする (= ユーザー drag と plugin リサイズ要求の
    /// 衝突によるウィンドウ振動を抑止、Codex P4)。
    /// Rust 側 wndproc が `WM_ENTERSIZEMOVE` / `WM_EXITSIZEMOVE` を受けて発行する。
    #[serde(rename = "set_user_resizing")]
    SetUserResizing {
        active: u32,
    },
    /// プラグイン内部状態 (= EQ カーブ / chunk) を base64 文字列で取得する。
    /// 応答は `Event::PluginState`。終了時 / 永続化トリガで一度だけ呼ぶ想定。
    #[serde(rename = "query_state")]
    QueryState,
    /// 起動時の auto-restore: settings.json に保存されていた base64 state を渡し、
    /// bridge 側で `IComponent::setState` で復元する。fire-and-forget (= ack 無し)。
    /// 失敗時は bridge 側で `Event::Error` を発行する。
    #[serde(rename = "restore_state")]
    RestoreState {
        state: String,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "event")]
#[serde(rename_all = "snake_case")]
pub enum Event {
    Ready {
        version: u32,
    },
    Loaded {
        plugin_name: String,
        latency_samples: u32,
        #[serde(default)]
        slot_id: u64,
    },
    Probed {
        plugin_name: String,
        audio_input_buses: u32,
        audio_output_buses: u32,
        event_input_buses: u32,
        event_output_buses: u32,
        audio_input_channels: u32,
        audio_output_channels: u32,
        usable_audio_effect: bool,
    },
    LatencyChanged {
        latency_samples: u32,
        #[serde(default)]
        slot_id: u64,
    },
    /// `Cmd::Reset { reset_id }` への応答。同じ `reset_id` をエコーで返す。
    /// 待機側はこれを照合して「自分が送った reset の ack か」を判定する
    /// (= stale ack race 防止、Codex 助言、2026-05-01)。
    ResetDone {
        #[serde(default)]
        reset_id: u64,
    },
    Closed,
    Error {
        detail: String,
    },
    GuiAttached {
        width: u32,
        height: u32,
        #[serde(default)]
        slot_id: u64,
        #[serde(default)]
        container_hwnd: u64,
    },
    GuiDetached,
    GuiVisibilityResult {
        request_id: u64,
        slot_id: u64,
        outcome: GuiVisibilityOutcome,
    },
    GuiUserHidden {
        #[serde(default)]
        slot_id: u64,
    },
    GuiBypassToggle {
        #[serde(default)]
        slot_id: u64,
    },
    /// プラグインの推奨 GUI サイズ (query_gui_size の応答)。
    /// `resizable` は IPlugView::canResize() の結果 (= ホスト側が WS_THICKFRAME を
    /// 付けるかの判断に使う)。古い bridge との互換性のため `#[serde(default)]` で false。
    GuiSize {
        width: u32,
        height: u32,
        #[serde(default)]
        resizable: bool,
    },
    /// `Cmd::QueryState` の応答。プラグイン内部状態を base64 文字列で受け取る。
    PluginState {
        state: String,
        #[serde(default)]
        slot_id: u64,
        #[serde(default)]
        request_id: Option<u64>,
        #[serde(default)]
        error: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConcurrentStateError {
    Interrupted(String),
    HostResponse(String),
    HostExited,
}

impl std::fmt::Display for ConcurrentStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interrupted(reason) | Self::HostResponse(reason) => f.write_str(reason),
            Self::HostExited => f.write_str("host stdout closed"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuiVisibilityOutcome {
    Shown,
    Hidden,
    Cancelled,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuiVisibilitySignal {
    Closed(u64),
    Checked {
        slot_id: u64,
        outcome: GuiVisibilityOutcome,
    },
}

impl GuiVisibilitySignal {
    pub(super) fn projection(self) -> Option<(u64, bool, bool)> {
        match self {
            Self::Closed(slot) => Some((slot, false, true)),
            Self::Checked {
                slot_id,
                outcome: GuiVisibilityOutcome::Shown,
            } => Some((slot_id, true, false)),
            Self::Checked {
                slot_id,
                outcome: GuiVisibilityOutcome::Hidden,
            } => Some((slot_id, false, false)),
            _ => None,
        }
    }
}

type PendingGuiRequests = Arc<
    Mutex<
        HashMap<u64, crossbeam_channel::Sender<Result<GuiVisibilityOutcome, ConcurrentStateError>>>,
    >,
>;
fn finish_pending_gui_requests(pending: &PendingGuiRequests, error: ConcurrentStateError) {
    for (_, reply) in pending.lock().unwrap().drain() {
        let _ = reply.try_send(Err(error.clone()));
    }
}
fn route_gui_result(pending: &PendingGuiRequests, request_id: u64, outcome: GuiVisibilityOutcome) {
    if let Some(reply) = pending.lock().unwrap().remove(&request_id) {
        let result = if outcome == GuiVisibilityOutcome::Error {
            Err(ConcurrentStateError::HostResponse(
                "host rejected checked GUI visibility".into(),
            ))
        } else {
            Ok(outcome)
        };
        let _ = reply.try_send(result);
    }
}

type PendingStateQueries =
    Arc<Mutex<HashMap<u64, crossbeam_channel::Sender<Result<String, ConcurrentStateError>>>>>;

fn abort_pending_state_queries(pending: &PendingStateQueries, reason: &str) {
    finish_pending_state_queries(
        pending,
        ConcurrentStateError::Interrupted(reason.to_string()),
    );
}

fn finish_pending_state_queries(pending: &PendingStateQueries, error: ConcurrentStateError) {
    if let Ok(mut queries) = pending.lock() {
        for (_, reply) in queries.drain() {
            let _ = reply.try_send(Err(error.clone()));
        }
    }
}

fn route_concurrent_state_result(
    pending: &PendingStateQueries,
    request_id: u64,
    state: Option<String>,
    error: Option<String>,
) {
    let reply = pending
        .lock()
        .ok()
        .and_then(|mut queries| queries.remove(&request_id));
    if let Some(reply) = reply {
        let result = match (state, error) {
            (Some(state), None) => Ok(state),
            (_, Some(error)) if error == "interrupted" => {
                Err(ConcurrentStateError::Interrupted(error))
            }
            (_, Some(error)) => Err(ConcurrentStateError::HostResponse(error)),
            _ => Err(ConcurrentStateError::HostResponse(
                "invalid concurrent state response".to_string(),
            )),
        };
        let _ = reply.try_send(result);
    }
}

#[cfg(windows)]
fn audio_pipe_names(pid: u32, stamp: u128) -> (String, String, String) {
    let id = NEXT_AUDIO_PIPE_ID.fetch_add(1, Ordering::Relaxed);
    (
        format!("miv-vst-shm-{pid}-{stamp}-{id}"),
        format!("miv-vst-sigin-{pid}-{stamp}-{id}"),
        format!("miv-vst-sigout-{pid}-{stamp}-{id}"),
    )
}

#[cfg(windows)]
unsafe fn reject_existing_handle(handle: HANDLE, label: &str) -> std::io::Result<HANDLE> {
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let _ = unsafe { CloseHandle(handle) };
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{label} already exists"),
        ))
    } else {
        Ok(handle)
    }
}

/// bridge プロセスのハンドル。stdin/stdout と shared memory リソースを保持する。
pub struct Bridge {
    child: Child,
    stdin: Mutex<ChildStdin>,
    /// 同期 event 受信用 channel。spawn 時に起動した event-pump スレッドが
    /// stdout を読んで非同期 (LatencyChanged / ResetDone) 以外の event をここに流す。
    /// recv() はここから読む。
    event_rx: crossbeam_channel::Receiver<std::io::Result<Event>>,
    sync_call_mutex: Mutex<()>,
    /// プラグインの最新 latency_samples (= bridge から非同期通知された値)。
    /// 初期値 = u32::MAX (= 「未受信」マーカ)。Loaded event 受信後に通常値が入る。
    /// audio-pump が `total_latency_samples()` から定期 polling して slot.latency_samples
    /// を更新する。プラグインが UI でモード切替して `restartComponent(kLatencyChanged)`
    /// を発火すると、event-pump がここを atomically 更新する。
    cached_latency_samples: Arc<AtomicU32>,
    cached_latency_by_slot: Arc<Mutex<HashMap<u64, u32>>>,
    /// シーク時の同期 reset 用 ack channel。bridge audio thread が in/out ring drain +
    /// `loader_->reset()` を実行した後に `Event::ResetDone { reset_id }` を返してくる。
    /// 一般 `event_rx` に流すと `query_gui_size` などの同期 recv() が誤って拾うので分離。
    /// 値は `reset_id` の世代 ID で、`wait_reset_done(expected_id)` が照合に使う
    /// (= stale ack race 防止、Codex 助言、2026-05-01)。
    reset_ack_rx: crossbeam_channel::Receiver<u64>,
    gui_visibility_rx: crossbeam_channel::Receiver<GuiVisibilitySignal>,
    gui_bypass_toggle_rx: crossbeam_channel::Receiver<u64>,
    /// reset_sync helper が使う世代 ID counter。`fetch_add(1)` で発行する。
    next_reset_id: AtomicU64,
    next_request_id: AtomicU64,
    pending_state_queries: PendingStateQueries,
    pending_gui_requests: PendingGuiRequests,
    #[cfg(windows)]
    shm: Option<SharedMemory>,
    #[cfg(windows)]
    sig_in: Option<EventHandle>,
    #[cfg(windows)]
    sig_out: Option<EventHandle>,
}

type GuiSignalWake = Arc<dyn Fn() + Send + Sync>;

fn route_gui_signal(
    event: &Event,
    user_hidden_tx: &crossbeam_channel::Sender<GuiVisibilitySignal>,
    bypass_toggle_tx: &crossbeam_channel::Sender<u64>,
    wake: Option<&GuiSignalWake>,
) -> bool {
    let queued = match event {
        Event::GuiUserHidden { slot_id } => user_hidden_tx
            .try_send(GuiVisibilitySignal::Closed(*slot_id))
            .is_ok(),
        Event::GuiVisibilityResult {
            slot_id, outcome, ..
        } => user_hidden_tx
            .try_send(GuiVisibilitySignal::Checked {
                slot_id: *slot_id,
                outcome: *outcome,
            })
            .is_ok(),
        Event::GuiBypassToggle { slot_id } => bypass_toggle_tx.try_send(*slot_id).is_ok(),
        _ => return false,
    };
    if queued {
        if let Some(wake) = wake {
            wake();
        }
    }
    queued
}

#[cfg(windows)]
struct SharedMemory {
    handle: HANDLE,
    base: MEMORY_MAPPED_VIEW_ADDRESS,
    size: u64,
    name: String,
    // T08 (v0.9.0) Codex P2 反映: `open_audio_pipe` で header に書いた直後の値を
    // ここへキャッシュする。`push_audio` / `pull_audio` / `preferred_transfer_samples`
    // は本フィールドのみを参照し、共有メモリ越しの `read_unaligned` を毎回しない。
    // C++ 側の `AudioPipe::cached_capacity_` と対称。
    cached_capacity: u32,
    cached_channels: u32,
    cached_block_size: u32,
}

#[cfg(windows)]
struct EventHandle {
    handle: HANDLE,
    name: String,
}

#[cfg(windows)]
unsafe impl Send for SharedMemory {}
#[cfg(windows)]
unsafe impl Sync for SharedMemory {}
#[cfg(windows)]
unsafe impl Send for EventHandle {}
#[cfg(windows)]
unsafe impl Sync for EventHandle {}

impl Bridge {
    pub fn process_id(&self) -> u32 {
        self.child.id()
    }

    /// Called on a capture worker after stdout EOF. The child handle can lag
    /// behind pipe closure, so wait for process termination before reading code.
    pub fn wait_host_exit_code(&self, timeout: std::time::Duration) -> Option<u32> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            let handle = HANDLE(self.child.as_raw_handle());
            let wait_ms = timeout.as_millis().min(u32::MAX as u128) as u32;
            if unsafe { WaitForSingleObject(handle, wait_ms) }.0 != 0 {
                return None;
            }
            let mut code = 0_u32;
            unsafe { GetExitCodeProcess(handle, &mut code) }.ok()?;
            Some(code)
        }
        #[cfg(not(windows))]
        {
            let _ = timeout;
            None
        }
    }

    /// bridge exe を子プロセスとして起動する。
    /// `stderr_cb` は bridge プロセスの stderr に書かれた 1 行を受け取るコールバック。
    /// tester 側はこれを使ってログファイルにブリッジの内部状態 (show_gui の各ステップ等)
    /// を合流させる。バックグラウンドスレッドが子プロセス終了まで動き続ける。
    pub fn spawn<F>(exe_path: &std::path::Path, stderr_cb: F) -> std::io::Result<Self>
    where
        F: Fn(String) + Send + 'static,
    {
        Self::spawn_with_gui_signal_wake(exe_path, stderr_cb, None)
    }

    pub(crate) fn spawn_with_gui_signal_wake<F>(
        exe_path: &std::path::Path,
        stderr_cb: F,
        gui_signal_wake: Option<GuiSignalWake>,
    ) -> std::io::Result<Self>
    where
        F: Fn(String) + Send + 'static,
    {
        let mut command = Command::new(exe_path);
        command
            .arg("--parent-pid")
            .arg(std::process::id().to_string());
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Windows で bridge が console subsystem (= stdin/stdout 必須なので window
        // subsystem 化できない) で起動するときに、デフォルトでは黒い cmd ウィンドウが
        // 一瞬チラついて表示される。`CREATE_NO_WINDOW (0x08000000)` を付けると
        // コンソールが割り当てられず、ユーザー視点では完全にバックグラウンド処理になる。
        // bridge は GUI スレッドで PeekMessage ループを回すので、コンソールが無くても
        // プラグイン GUI は問題なく表示される。
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        std::thread::Builder::new()
            .name("bridge-stderr-pump".into())
            .spawn(move || {
                use std::io::BufRead;
                let reader = std::io::BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    stderr_cb(line);
                }
            })
            .ok(); // spawn 失敗しても致命ではない (= ログが流れないだけ)

        // event-pump thread: stdout を read し続けて、LatencyChanged は atomic に
        // 反映、それ以外は channel 経由で recv() に渡す。
        // この設計により:
        //   1. 同期 event (Loaded/Ready/etc) は recv() で blocking 取得できる
        //   2. 非同期 event (LatencyChanged) は誰も recv() を呼んでなくても捕捉される
        // bridge が exit すると stdout EOF → pump 終了 → channel sender drop → recv() で
        // disconnected error。
        let cached_latency = Arc::new(AtomicU32::new(u32::MAX));
        let cached_latency_by_slot = Arc::new(Mutex::new(HashMap::<u64, u32>::new()));
        let (event_tx, event_rx) = crossbeam_channel::bounded::<std::io::Result<Event>>(64);
        // ResetDone 専用 ack channel。bridge audio thread が reset 実行後に流す
        // `Event::ResetDone { reset_id }` の reset_id をここに送る。
        // `wait_reset_done(expected_id)` が照合してから受理する (= stale ack 排除)。
        // bounded(8) は十分 (= 通常 1 個ずつ即消費、複数 reset 連続でも全 ID を保持)。
        let (reset_ack_tx, reset_ack_rx) = crossbeam_channel::bounded::<u64>(8);
        let (gui_visibility_tx, gui_visibility_rx) =
            crossbeam_channel::unbounded::<GuiVisibilitySignal>();
        let (gui_bypass_toggle_tx, gui_bypass_toggle_rx) = crossbeam_channel::bounded::<u64>(64);
        let pending_state_queries: PendingStateQueries = Arc::new(Mutex::new(HashMap::new()));
        let pending_state_queries_for_pump = Arc::clone(&pending_state_queries);
        let pending_gui_requests: PendingGuiRequests = Arc::new(Mutex::new(HashMap::new()));
        let pending_gui_for_pump = Arc::clone(&pending_gui_requests);
        let cached_latency_for_pump = cached_latency.clone();
        let cached_latency_by_slot_for_pump = cached_latency_by_slot.clone();
        std::thread::Builder::new()
            .name("bridge-event-pump".into())
            .spawn(move || {
                let mut stdout = stdout;
                loop {
                    match read_event_blocking(&mut stdout) {
                        Ok(Event::LatencyChanged {
                            latency_samples,
                            slot_id,
                        }) => {
                            if slot_id == 0 {
                                cached_latency_for_pump
                                    .store(latency_samples, Ordering::Release);
                            }
                            if let Ok(mut map) = cached_latency_by_slot_for_pump.lock() {
                                map.insert(slot_id, latency_samples);
                            }
                            // 通知ログ (mIV 側 audio-pump が次に total_latency_samples を
                            // 呼んだ時に拾う想定)
                            crate::logger::log(format!(
                                "[VST3 PDC] bridge LatencyChanged: latency_samples={} ({:.3}ms@assumed-48kHz)",
                                latency_samples,
                                latency_samples as f64 / 48000.0 * 1000.0,
                            ));
                            // channel には流さない (= 同期 recv() を妨げないため)
                        }
                        Ok(Event::ResetDone { reset_id }) => {
                            // ResetDone は専用 ack channel に流す (= 一般 event_rx に
                            // 流すと query_gui_size 等の同期 recv() が誤って拾う)。
                            // reset_id を流して `wait_reset_done(expected_id)` が照合する
                            // (= stale ack race 防止、Codex 助言、2026-05-01)。
                            let _ = reset_ack_tx.try_send(reset_id);
                        }
                        Ok(event @ Event::GuiVisibilityResult { .. })
                        | Ok(event @ Event::GuiUserHidden { .. })
                        | Ok(event @ Event::GuiBypassToggle { .. }) => {
                            route_gui_signal(
                                &event,
                                &gui_visibility_tx,
                                &gui_bypass_toggle_tx,
                                gui_signal_wake.as_ref(),
                            );
                            if let Event::GuiVisibilityResult { request_id, outcome, .. } = event {
                                route_gui_result(&pending_gui_for_pump, request_id, outcome);
                            }
                        }
                        Ok(Event::PluginState {
                            request_id: Some(request_id),
                            state,
                            error,
                            ..
                        }) => {
                            route_concurrent_state_result(
                                &pending_state_queries_for_pump,
                                request_id,
                                Some(state),
                                error,
                            );
                        }
                        Ok(other) => {
                            // Loaded を受信したら cached_latency にも反映する
                            // (= 起動直後は LatencyChanged 通知が来ないプラグインに対応)
                            if let Event::Loaded {
                                latency_samples,
                                slot_id,
                                ..
                            } = &other
                            {
                                if *slot_id == 0 {
                                    cached_latency_for_pump
                                        .store(*latency_samples, Ordering::Release);
                                }
                                if let Ok(mut map) = cached_latency_by_slot_for_pump.lock() {
                                    map.insert(*slot_id, *latency_samples);
                                }
                            }
                            if event_tx.send(Ok(other)).is_err() {
                                break;  // receiver dropped
                            }
                        }
                        Err(e) => {
                            let query_error = if e.kind() == std::io::ErrorKind::InvalidData {
                                ConcurrentStateError::HostResponse(format!(
                                    "invalid host event: {e}"
                                ))
                            } else {
                                // A closed stdout is a host exit even if its
                                // process handle has not been signalled yet.
                                ConcurrentStateError::HostExited
                            };
                            finish_pending_state_queries(
                                &pending_state_queries_for_pump,
                                query_error.clone(),
                            );
                            finish_pending_gui_requests(&pending_gui_for_pump, query_error);
                            let _ = event_tx.send(Err(e));
                            break;  // EOF or error
                        }
                    }
                }
            })
            .ok();

        Ok(Self {
            child,
            stdin: Mutex::new(stdin),
            event_rx,
            sync_call_mutex: Mutex::new(()),
            cached_latency_samples: cached_latency,
            cached_latency_by_slot,
            reset_ack_rx,
            gui_visibility_rx,
            gui_bypass_toggle_rx,
            next_reset_id: AtomicU64::new(0),
            next_request_id: AtomicU64::new(0),
            pending_state_queries,
            pending_gui_requests,
            #[cfg(windows)]
            shm: None,
            #[cfg(windows)]
            sig_in: None,
            #[cfg(windows)]
            sig_out: None,
        })
    }

    pub fn drain_gui_visibility(&self) -> Vec<GuiVisibilitySignal> {
        self.gui_visibility_rx.try_iter().collect()
    }

    pub fn drain_gui_bypass_toggle_slots(&self) -> Vec<u64> {
        let mut out = Vec::new();
        while let Ok(slot_id) = self.gui_bypass_toggle_rx.try_recv() {
            out.push(slot_id);
        }
        out
    }

    /// シーク時の同期 reset (= Codex 助言、2026-05-01、ack generation ID で stale-ack race 防止):
    ///
    /// 1. world-unique な `reset_id` を発行 (= per-bridge atomic counter で +1)
    /// 2. `Cmd::Reset { reset_id }` を bridge に送る
    /// 3. ack channel から `expected_id == reset_id` の ResetDone を timeout 内で待つ
    ///    - 古い ID (= 過去 timeout した reset の遅延 ack) が来たら drop してログ
    ///    - 未来 ID は通常起きないが、来たら drop してログ
    ///    - timeout したら false (= 呼び出し側は fallback で続行)
    ///
    /// 戻り値: true = ack 一致受信、false = timeout (= 200ms 経っても合致しなかった)。
    pub fn reset_sync(&self, timeout: std::time::Duration) -> bool {
        self.reset_sync_result(timeout).is_ok()
    }

    pub fn reset_sync_result(&self, timeout: std::time::Duration) -> Result<(), String> {
        self.reset_sync_result_until(std::time::Instant::now() + timeout)
    }

    pub fn reset_sync_result_until(&self, deadline: std::time::Instant) -> Result<(), String> {
        let _guard = self.sync_call_mutex.lock().unwrap();
        if std::time::Instant::now() >= deadline {
            return Err("reset deadline expired".to_string());
        }
        // generation ID 発行 (= 0 から始まらないように +1 してから atomic に格納)。
        // wrapping_add で u64 overflow しても新規 ID として扱える (= 18 京回 reset で
        // overflow なので実用上発生しない)。
        let id = self
            .next_reset_id
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        if let Err(e) = self.send(&Cmd::Reset { reset_id: id }) {
            crate::logger::log(format!("[VST3] reset_sync: send failed for id={id}: {e}"));
            return Err(format!("reset send failed: {e}"));
        }
        // ID 照合 loop (= 一致するまで old/future を drop)
        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err("reset ack timeout".to_string());
            }
            match self.reset_ack_rx.recv_timeout(deadline - now) {
                Ok(got_id) if got_id == id => return Ok(()),
                Ok(got_id) => {
                    crate::logger::log(format!(
                        "[VST3] reset_sync: ignored stale ResetDone ack id={got_id}, expected={id}"
                    ));
                    // 続行して expected を待つ
                }
                Err(e) => return Err(format!("reset ack: {e}")),
            }
        }
    }

    /// audio-pump が定期 polling する用: bridge から非同期通知された最新の
    /// latency_samples を取得する。Loaded event 未受信なら u32::MAX を返す
    /// (= 呼び出し側は「まだ不明」として 0 扱いに fallback すべき)。
    pub fn cached_latency_samples_value(&self) -> u32 {
        self.cached_latency_samples.load(Ordering::Acquire)
    }

    pub fn cached_latency_samples_value_slot(&self, slot_id: u64) -> u32 {
        self.cached_latency_by_slot
            .lock()
            .ok()
            .and_then(|map| map.get(&slot_id).copied())
            .unwrap_or(u32::MAX)
    }

    pub fn add_plugin_to_chain(
        &self,
        slot_id: u64,
        plugin_path: &str,
        initial_state: Option<&str>,
        bypass: bool,
        strict_state: bool,
    ) -> std::io::Result<()> {
        let mut msg = serde_json::json!({
            "cmd": "add_plugin",
            "slot_id": slot_id,
            "plugin_path": plugin_path,
            "bypass": if bypass { 1 } else { 0 },
            "strict_state": if strict_state { 1 } else { 0 },
        });
        if let Some(state) = initial_state.filter(|s| !s.is_empty()) {
            msg["state"] = serde_json::Value::String(state.to_string());
        }
        self.send_value(&msg)
    }

    pub fn with_sync_call<R>(&self, f: impl FnOnce() -> R) -> R {
        let _guard = self.sync_call_mutex.lock().unwrap();
        f()
    }

    pub fn sync_call_guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.sync_call_mutex.lock().unwrap()
    }

    pub fn set_bypass_slot(&self, slot_id: u64, bypass: bool) -> std::io::Result<()> {
        self.send_value(&serde_json::json!({
            "cmd": "set_bypass",
            "slot_id": slot_id,
            "bypass": if bypass { 1 } else { 0 },
        }))
    }

    pub fn move_plugin_slot(
        &self,
        slot_id: u64,
        before_slot_id: Option<u64>,
    ) -> std::io::Result<()> {
        self.send_value(&serde_json::json!({
            "cmd": "move_plugin",
            "slot_id": slot_id,
            "before_slot_id": before_slot_id.unwrap_or(u64::MAX),
        }))
    }

    /// 既に作成済みの editor surface の owner HWND を一括更新する。
    pub fn set_chain_owner(&self, owner_hwnd: u64) -> std::io::Result<()> {
        self.send_value(&serde_json::json!({
            "cmd": "set_chain_owner",
            "owner_hwnd": owner_hwnd,
        }))
    }

    /// プラグイン内部状態 (= EQ カーブ / chunk) を base64 文字列で取得する。
    /// `Cmd::QueryState` を送って `Event::PluginState` を `timeout` 内で待つ。
    /// 期待外の event (= Error / 旧 reset 等) は **drop してログ** し、期待 event を
    /// 待ち続ける (= 他の同期 IPC と混線しないが、想定外があれば情報として残す)。
    /// 戻り値: Ok(state_b64) | Err(原因)。
    pub fn query_state_sync(&self, timeout: std::time::Duration) -> Result<String, String> {
        let _guard = self.sync_call_mutex.lock().unwrap();
        self.send(&Cmd::QueryState)
            .map_err(|e| format!("send: {e}"))?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err("timeout".to_string());
            }
            match self.event_rx.recv_timeout(deadline - now) {
                Ok(Ok(Event::PluginState { state, .. })) => return Ok(state),
                Ok(Ok(Event::Error { detail })) => return Err(detail),
                Ok(Ok(other)) => {
                    crate::logger::log(format!(
                        "[VST3] query_state: ignored unexpected event: {other:?}"
                    ));
                }
                Ok(Err(e)) => return Err(format!("io: {e}")),
                Err(_) => return Err("timeout".to_string()),
            }
        }
    }

    pub fn query_state_sync_slot(
        &self,
        slot_id: u64,
        timeout: std::time::Duration,
    ) -> Result<String, String> {
        let _guard = self.sync_call_mutex.lock().unwrap();
        self.send_value(&serde_json::json!({
            "cmd": "query_state",
            "slot_id": slot_id,
        }))
        .map_err(|e| format!("send: {e}"))?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err("timeout".to_string());
            }
            match self.event_rx.recv_timeout(deadline - now) {
                Ok(Ok(Event::PluginState {
                    state,
                    slot_id: got,
                    ..
                })) if got == slot_id => {
                    return Ok(state);
                }
                Ok(Ok(Event::Error { detail })) => return Err(detail),
                Ok(Ok(other)) => {
                    crate::logger::log(format!(
                        "[VST3] query_state(slot={slot_id}): ignored unexpected event: {other:?}"
                    ));
                }
                Ok(Err(e)) => return Err(format!("io: {e}")),
                Err(_) => return Err("timeout".to_string()),
            }
        }
    }

    /// The receiver has a caller-owned deadline. Dropping it does not cancel
    /// a running host capture; the host has its own five-second watchdog.
    pub fn query_state_concurrent(
        &self,
        slot_id: u64,
    ) -> Result<
        crossbeam_channel::Receiver<Result<String, ConcurrentStateError>>,
        ConcurrentStateError,
    > {
        let id = self.next_request_id.fetch_add(1, Ordering::AcqRel) + 1;
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        self.pending_state_queries
            .lock()
            .unwrap()
            .insert(id, reply_tx);
        if let Err(_error) = self.send_value(&serde_json::json!({
            "cmd": "query_state_concurrent",
            "slot_id": slot_id,
            "request_id": id,
        })) {
            if let Ok(mut queries) = self.pending_state_queries.lock() {
                queries.remove(&id);
            }
            return Err(ConcurrentStateError::HostExited);
        }
        Ok(reply_rx)
    }

    /// Worker-only; the pipe write is not a successful presentation outcome.
    pub fn set_gui_visibility_checked(
        &self,
        slot_id: u64,
        visible: bool,
        minimized_sequence: u64,
        remote_token: u64,
    ) -> Result<GuiVisibilityOutcome, ConcurrentStateError> {
        let id = self.next_request_id.fetch_add(1, Ordering::AcqRel) + 1;
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.pending_gui_requests.lock().unwrap().insert(id, tx);
        let result = (|| {
            self.send_value(&serde_json::json!({"cmd": "set_gui_visibility_checked", "request_id": id,
                "slot_id": slot_id, "visible": u32::from(visible), "minimized_sequence": minimized_sequence, "remote_token": remote_token}))
                .map_err(|_| ConcurrentStateError::HostExited)?;
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| {
                    ConcurrentStateError::Interrupted(format!(
                        "checked GUI visibility ACK: {error}"
                    ))
                })?
        })();
        self.pending_gui_requests.lock().unwrap().remove(&id);
        result
    }

    pub fn abort_state_queries(&self) {
        finish_pending_gui_requests(
            &self.pending_gui_requests,
            ConcurrentStateError::Interrupted("bridge shutting down".into()),
        );
        abort_pending_state_queries(
            &self.pending_state_queries,
            "interrupted: bridge shutting down",
        );
    }

    /// End the dedicated host when the application's exit fence expires.
    #[cfg(windows)]
    pub fn terminate_now(&self) {
        use std::os::windows::io::AsRawHandle;
        self.abort_state_queries();
        unsafe {
            let _ = TerminateProcess(HANDLE(self.child.as_raw_handle()), 1);
        }
    }

    /// 制御メッセージを送る。
    pub fn send(&self, cmd: &Cmd) -> std::io::Result<()> {
        let payload = serde_json::to_vec(cmd)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let len = u32::try_from(payload.len()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "message too large")
        })?;
        let mut stdin = self.stdin.lock().unwrap();
        stdin.write_all(&len.to_le_bytes())?;
        stdin.write_all(&payload)?;
        stdin.flush()?;
        Ok(())
    }

    pub fn send_value(&self, value: &serde_json::Value) -> std::io::Result<()> {
        let payload = serde_json::to_vec(value)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let len = u32::try_from(payload.len()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "message too large")
        })?;
        let mut stdin = self.stdin.lock().unwrap();
        stdin.write_all(&len.to_le_bytes())?;
        stdin.write_all(&payload)?;
        stdin.flush()?;
        Ok(())
    }

    /// イベントを 1 つ受信する (blocking)。
    /// 内部的には event-pump スレッドが stdout から読み出して channel に流したものを取得。
    /// LatencyChanged event は pump が intercept して `cached_latency_samples` に
    /// 直接反映するため、ここには来ない (= 同期待ちを妨げない)。
    pub fn recv(&self) -> std::io::Result<Event> {
        match self.event_rx.recv() {
            Ok(result) => result,
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "event channel disconnected (bridge process likely exited)",
            )),
        }
    }

    /// イベントを timeout 付きで 1 つ受信する。
    /// VST3 probe のように壊れたプラグインを短時間だけ開く用途で、子プロセスが
    /// 応答しない場合に UI 側 worker を永久待ちにしないための helper。
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> std::io::Result<Event> {
        match self.event_rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "bridge event timeout",
            )),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "event channel disconnected (bridge process likely exited)",
            )),
        }
    }
}

/// stdout から 1 event を blocking で読む。bridge::Bridge spawn 時の event-pump
/// thread から呼ばれる。
fn read_event_blocking(stdout: &mut ChildStdout) -> std::io::Result<Event> {
    let mut len_buf = [0u8; 4];
    stdout.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    // `plugin_state` event の chunk は plugin によって数百 KB になる
    // (= ML / preset 内蔵 plugin)。
    // **C++ 側 `crates/vst3-host/include/protocol.h::MAX_CONTROL_MSG_SIZE`
    // と必ず一致**させること。protocol.h を変えたらここも変える。
    const MAX_CONTROL_MSG_SIZE: usize = 32 * 1024 * 1024;
    if len > MAX_CONTROL_MSG_SIZE {
        let mut remaining = len;
        let mut scratch = [0u8; 64 * 1024];
        while remaining > 0 {
            let chunk = remaining.min(scratch.len());
            stdout.read_exact(&mut scratch[..chunk])?;
            remaining -= chunk;
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "control message too large",
        ));
    }
    let mut body = vec![0u8; len];
    stdout.read_exact(&mut body)?;
    let event: Event = serde_json::from_slice(&body)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(event)
}

impl Bridge {
    /// shared memory + named events を作って bridge にアタッチさせる。
    /// `initial_state` を渡すと bridge が audio_thread 起動前にプラグイン内部状態を
    /// 復元する (Codex P2-3、2026-05-01: race-free な auto-restore のため state は
    /// 別 cmd ではなく Open に同梱する)。
    #[cfg(windows)]
    pub fn open_audio_pipe(
        &mut self,
        plugin_path: &str,
        sample_rate: u32,
        block_size: u32,
        initial_state: Option<&str>,
        strict_state: bool,
    ) -> std::io::Result<()> {
        let pid = std::process::id();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros())
            .unwrap_or(0);
        // 名前空間プレフィックス (`Local\`) は付けない。
        // - 付けないと CreateFileMappingW のデフォルト = Local 名前空間
        //   (= セッション内有効) になり同等の挙動。
        // - 付けると JSON シリアライズで `Local\\miv-...` にエスケープされ、
        //   bridge 側の素朴 JSON 抽出 (エスケープ解除なし) で `\\` が
        //   そのまま渡って名前不一致になる。素直に英数字+ハイフンのみで作る。
        let (shm_name, sig_in_name, sig_out_name) = audio_pipe_names(pid, stamp);

        // 容量: block_size * channels * 8 (= 8 ブロック分のマージン) — sample 単位で持つ
        let capacity = block_size * 2 * 8;
        let shm_size = shm_size_bytes(capacity);

        // shared memory 作成
        unsafe {
            let wname = HSTRING::from(shm_name.as_str());
            let handle = CreateFileMappingW(
                HANDLE(usize::MAX as *mut _),
                None,
                PAGE_READWRITE,
                ((shm_size >> 32) & 0xFFFF_FFFF) as u32,
                (shm_size & 0xFFFF_FFFF) as u32,
                PCWSTR(wname.as_ptr()),
            )
            .map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("CreateFileMappingW: {e}"),
                )
            })?;
            let handle = reject_existing_handle(handle, &shm_name)?;
            let base = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, shm_size as usize);
            if base.Value.is_null() {
                let _ = CloseHandle(handle);
                return Err(std::io::Error::other("MapViewOfFile failed"));
            }
            // header 初期化
            let header = base.Value as *mut ShmHeader;
            (*header).in_write.store(0, Ordering::Relaxed);
            (*header).in_read.store(0, Ordering::Relaxed);
            (*header).out_write.store(0, Ordering::Relaxed);
            (*header).out_read.store(0, Ordering::Relaxed);
            std::ptr::addr_of_mut!((*header).capacity).write_unaligned(capacity);
            std::ptr::addr_of_mut!((*header).channels).write_unaligned(2);
            std::ptr::addr_of_mut!((*header).sample_rate).write_unaligned(sample_rate);
            std::ptr::addr_of_mut!((*header).block_size).write_unaligned(block_size);

            self.shm = Some(SharedMemory {
                handle,
                base,
                size: shm_size,
                name: shm_name.clone(),
                cached_capacity: capacity,
                cached_channels: 2,
                cached_block_size: block_size,
            });

            // events
            let win = HSTRING::from(sig_in_name.as_str());
            let sig_in = CreateEventW(None, false, false, PCWSTR(win.as_ptr())).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("CreateEventW sig_in: {e}"),
                )
            })?;
            let sig_in = reject_existing_handle(sig_in, &sig_in_name)?;
            self.sig_in = Some(EventHandle {
                handle: sig_in,
                name: sig_in_name.clone(),
            });
            let wout = HSTRING::from(sig_out_name.as_str());
            let sig_out = CreateEventW(None, false, false, PCWSTR(wout.as_ptr())).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("CreateEventW sig_out: {e}"),
                )
            })?;
            let sig_out = reject_existing_handle(sig_out, &sig_out_name)?;
            self.sig_out = Some(EventHandle {
                handle: sig_out,
                name: sig_out_name.clone(),
            });
        }

        self.send(&Cmd::Open {
            plugin_path: plugin_path.to_string(),
            sample_rate,
            block_size,
            shm_name,
            shm_size,
            sig_in: sig_in_name,
            sig_out: sig_out_name,
            state: initial_state
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            strict_state: if strict_state { 1 } else { 0 },
        })?;
        Ok(())
    }

    /// host から bridge への音声書き込み (= in_ring に push、sig_in 発火)。
    /// `samples` は f32 packed stereo。
    #[cfg(windows)]
    pub fn push_audio(&self, samples: &[f32]) -> std::io::Result<()> {
        let shm = self
            .shm
            .as_ref()
            .ok_or_else(|| std::io::Error::other("audio pipe not open"))?;
        let sig_in = self
            .sig_in
            .as_ref()
            .ok_or_else(|| std::io::Error::other("sig_in missing"))?;
        // T08 (v0.9.0) Codex P2: capacity は cached を使用。共有メモリの値を毎回
        // 読み直さない (= mid-flight 改変や TOCTOU を排除)。
        let cap = shm.cached_capacity;
        unsafe {
            let header = shm.base.Value as *mut ShmHeader;
            let in_ring =
                (shm.base.Value as *mut u8).add(std::mem::size_of::<ShmHeader>()) as *mut f32;

            let w_pos = (*header).in_write.load(Ordering::Relaxed);
            // overflow 防止のため modulo 操作を慎重に
            for (i, &s) in samples.iter().enumerate() {
                let idx = (w_pos.wrapping_add(i as u32)) % cap;
                in_ring.add(idx as usize).write(s);
            }
            (*header)
                .in_write
                .store(w_pos.wrapping_add(samples.len() as u32), Ordering::Release);
            let _ = SetEvent(sig_in.handle);
        }
        Ok(())
    }

    /// Safe host-side process helper for one bridge.
    ///
    /// The shared memory rings are intentionally small (`block_size * channels * 8` samples).
    /// Some codecs, especially WMA, can emit much larger audio frames. Pushing a whole decoded
    /// frame at once would wrap and overwrite the input ring before the bridge process can consume
    /// it, which sounds like a short fragment looping. Keep each IPC transfer at the bridge's
    /// native audio block size.
    ///
    /// T07 (v0.9.0): `timeout_ms` は **call 全体の deadline**。旧版は per-chunk 100ms で、
    /// `4096 / 960 ≒ 5` 個の chunk に分かれた呼び出しは最悪 500ms ブロックして cpal の
    /// アンダーランを誘発していた。新しいセマンティクスでは entry 時に deadline を 1 回計算し、
    /// 残時間で次の push/pull を行う。さらに short read / timeout で zero-fill して `Ok` を
    /// 返していた旧挙動を廃止し、`Err(TimedOut)` を伝播する (= caller が連続失敗で
    /// auto-disable に倒せるようにする)。
    #[cfg(windows)]
    pub fn process_audio_blocking(
        &self,
        src: &[f32],
        dst: &mut [f32],
        timeout_ms: u32,
    ) -> std::io::Result<()> {
        if src.len() != dst.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "src/dst length mismatch",
            ));
        }
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
        let chunk_samples = self.preferred_transfer_samples();
        let mut offset = 0;
        while offset < src.len() {
            // T07 (v0.9.0) Codex P1 round 2 反映: deadline を **push_audio の前にも**
            // 確認する。expired 状態で push を続けると、bridge が次 block を待っているとき
            // stale chunk を enqueue して pipe pairing がずれる (= 後続が "Ok with wrong
            // audio" で counter リセットされる desync race を縮める)。
            if std::time::Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "process_audio_blocking deadline exceeded before push at offset {offset}/{}",
                        src.len()
                    ),
                ));
            }
            let end = (offset + chunk_samples).min(src.len());
            self.push_audio(&src[offset..end])?;
            // 残時間で pull する。0 になっていたら即 TimedOut。
            let now = std::time::Instant::now();
            let remaining_ms = if now >= deadline {
                0
            } else {
                (deadline - now).as_millis().min(u32::MAX as u128) as u32
            };
            let want = end - offset;
            let n = self.pull_audio(&mut dst[offset..end], remaining_ms)?;
            if n < want {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("process_audio_blocking pulled {n} of {want} samples within deadline"),
                ));
            }
            offset = end;
        }
        Ok(())
    }

    #[cfg(windows)]
    fn preferred_transfer_samples(&self) -> usize {
        let Some(shm) = self.shm.as_ref() else {
            return 960;
        };
        // T08 (v0.9.0) Codex P2: cached_block_size / cached_channels を使う。
        let block_size = shm.cached_block_size as usize;
        let channels = shm.cached_channels as usize;
        (block_size.saturating_mul(channels).max(2) / 2) * 2
    }

    /// bridge から host への音声読み出し (= out_ring から pop、sig_out を待つ)。
    /// 戻り値: 実際に読めた sample 数 (タイムアウト時は 0〜want 未満)。
    ///
    /// **要求した dst.len() 分が揃うまでループで待つ**。
    ///
    /// 旧版は「1 度だけ sig_out を待ち、avail < want なら部分取得」だったが、
    /// bridge audio_loop は read_in_available でブロックを 480 サンプル単位に
    /// 細切れに処理するため、host が 1 回 push した 2048 サンプル (= ffmpeg AAC の
    /// 典型 1 frame) は **複数回に分けて out_ring に書かれる**。1 回しか待たない
    /// と先頭 480 サンプルだけ取れて残り 1568 サンプルは 0 fill → **クリック ノイズ
    /// が連発**する (= ユーザー報告の「ずっとブツブツ」の根本原因)。
    /// 必要量が揃うまで何度でも sig_out を待ち直し、deadline で切り上げる。
    #[cfg(windows)]
    pub fn pull_audio(&self, dst: &mut [f32], timeout_ms: u32) -> std::io::Result<usize> {
        let shm = self
            .shm
            .as_ref()
            .ok_or_else(|| std::io::Error::other("audio pipe not open"))?;
        let sig_out = self
            .sig_out
            .as_ref()
            .ok_or_else(|| std::io::Error::other("sig_out missing"))?;
        let want = dst.len() as u32;
        if want == 0 {
            return Ok(0);
        }
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
        // T08 (v0.9.0) Codex P2: cached_capacity を使用 (mid-flight 改変・TOCTOU 排除)
        let cap = shm.cached_capacity;
        let mut total_taken: u32 = 0;
        unsafe {
            let header = shm.base.Value as *mut ShmHeader;
            let out_ring = (shm.base.Value as *mut u8)
                .add(std::mem::size_of::<ShmHeader>())
                .add((cap as usize) * 4) as *mut f32;
            while total_taken < want {
                let r_pos = (*header).out_read.load(Ordering::Relaxed);
                let w_pos = (*header).out_write.load(Ordering::Acquire);
                let avail = w_pos.wrapping_sub(r_pos);
                if avail > 0 {
                    let take = avail.min(want - total_taken) as usize;
                    for i in 0..take {
                        let idx = (r_pos.wrapping_add(i as u32)) % cap;
                        dst[total_taken as usize + i] = out_ring.add(idx as usize).read();
                    }
                    (*header)
                        .out_read
                        .store(r_pos.wrapping_add(take as u32), Ordering::Release);
                    total_taken += take as u32;
                    if total_taken >= want {
                        break;
                    }
                    // まだ不足: bridge が次の chunk を書き込むのを引き続き待つ
                }
                // avail==0 か、まだ不足: deadline まで sig_out を待つ
                let now = std::time::Instant::now();
                if now >= deadline {
                    break;
                }
                let remaining = (deadline - now).as_millis().min(u32::MAX as u128) as u32;
                let res = WaitForSingleObject(sig_out.handle, remaining.max(1));
                if res.0 != 0 {
                    break; // timeout / abandoned
                }
            }
        }
        Ok(total_taken as usize)
    }

    /// graceful shutdown: bridge に shutdown 命令を送って exit を待つ。
    /// `&mut self` を取るので、Drop の自動 kill より先にユーザーが明示的に呼ぶ想定。
    pub fn shutdown(&mut self) -> std::io::Result<()> {
        self.abort_state_queries();
        let _ = self.send(&Cmd::Shutdown);
        let _ = self.child.wait();
        Ok(())
    }

    /// `Arc<Bridge>` から呼ぶ用の shutdown。`shutdown` 命令を送るだけで exit 待ちはしない。
    /// 子プロセスは shutdown を受信して自発的に exit するか、Arc の最後の参照が落ちた時点で
    /// Drop 経路で kill される。
    pub fn shutdown_async(&self) -> std::io::Result<()> {
        self.abort_state_queries();
        let _ = self.send(&Cmd::Shutdown);
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for Bridge {
    fn drop(&mut self) {
        self.abort_state_queries();
        if let Some(s) = self.shm.take() {
            unsafe {
                let _ = UnmapViewOfFile(s.base);
                let _ = CloseHandle(s.handle);
            }
            let _ = s.name; // suppress unused
            let _ = s.size;
        }
        for h in [self.sig_in.take(), self.sig_out.take()]
            .into_iter()
            .flatten()
        {
            unsafe {
                let _ = CloseHandle(h.handle);
            }
            let _ = h.name;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod concurrent_state_tests {
    use super::*;

    #[test]
    fn host_gui_user_hidden_wakes_idle_effetune_context_only() {
        use std::sync::atomic::AtomicUsize;

        let ctx = egui::Context::default();
        let repaint_count = Arc::new(AtomicUsize::new(0));
        let repaint_count_for_callback = Arc::clone(&repaint_count);
        ctx.set_request_repaint_callback(move |_| {
            repaint_count_for_callback.fetch_add(1, Ordering::SeqCst);
        });
        // Drain egui's initial two-pass repaint before simulating an idle app.
        for _ in 0..3 {
            let _ = ctx.run(Default::default(), |_| {});
        }
        repaint_count.store(0, Ordering::SeqCst);

        let wake_ctx = ctx.clone();
        let wake: GuiSignalWake = Arc::new(move || wake_ctx.request_repaint());
        let (hidden_tx, hidden_rx) = crossbeam_channel::bounded(1);
        let (bypass_tx, bypass_rx) = crossbeam_channel::bounded(1);
        let host_event: Event =
            serde_json::from_str(r#"{"event":"gui_user_hidden","slot_id":17}"#).unwrap();
        assert!(route_gui_signal(
            &host_event,
            &hidden_tx,
            &bypass_tx,
            Some(&wake),
        ));
        assert_eq!(
            hidden_rx.try_recv().unwrap(),
            GuiVisibilitySignal::Closed(17)
        );
        assert!(repaint_count.load(Ordering::SeqCst) > 0);

        let prior_repaints = repaint_count.load(Ordering::SeqCst);
        assert!(route_gui_signal(
            &Event::GuiBypassToggle { slot_id: 18 },
            &hidden_tx,
            &bypass_tx,
            None,
        ));
        assert_eq!(bypass_rx.try_recv().unwrap(), 18);
        assert_eq!(repaint_count.load(Ordering::SeqCst), prior_repaints);
    }

    #[test]
    fn checked_visibility_cancel_and_close_follow_host_order_not_ack_timing() {
        let pending: PendingGuiRequests = Arc::new(Mutex::new(HashMap::new()));
        let (ack_tx, ack_rx) = crossbeam_channel::bounded(1);
        pending.lock().unwrap().insert(19, ack_tx);
        let (visibility_tx, visibility_rx) = crossbeam_channel::unbounded();
        let (bypass_tx, _) = crossbeam_channel::unbounded();
        let outcome = Event::GuiVisibilityResult {
            request_id: 19,
            slot_id: 7,
            outcome: GuiVisibilityOutcome::Cancelled,
        };
        assert!(route_gui_signal(&outcome, &visibility_tx, &bypass_tx, None));
        route_gui_result(&pending, 19, GuiVisibilityOutcome::Cancelled);
        assert_eq!(ack_rx.recv().unwrap(), Ok(GuiVisibilityOutcome::Cancelled));
        assert!(visibility_rx.recv().unwrap().projection().is_none());

        // Host shows then closes before the worker has consumed its ACK.
        assert!(route_gui_signal(
            &Event::GuiVisibilityResult {
                request_id: 20,
                slot_id: 7,
                outcome: GuiVisibilityOutcome::Shown
            },
            &visibility_tx,
            &bypass_tx,
            None
        ));
        assert!(route_gui_signal(
            &Event::GuiUserHidden { slot_id: 7 },
            &visibility_tx,
            &bypass_tx,
            None
        ));
        assert_eq!(
            visibility_rx.recv().unwrap().projection(),
            Some((7, true, false))
        );
        assert_eq!(
            visibility_rx.recv().unwrap().projection(),
            Some((7, false, true))
        );
        assert!(route_gui_signal(
            &Event::GuiVisibilityResult {
                request_id: 21,
                slot_id: 7,
                outcome: GuiVisibilityOutcome::Hidden
            },
            &visibility_tx,
            &bypass_tx,
            None
        ));
        assert_eq!(
            visibility_rx.recv().unwrap().projection(),
            Some((7, false, false))
        );
    }

    #[test]
    fn checked_visibility_pending_requests_finish_on_error_exit_and_shutdown() {
        let pending: PendingGuiRequests = Arc::new(Mutex::new(HashMap::new()));
        for error in [
            ConcurrentStateError::HostExited,
            ConcurrentStateError::Interrupted("shutdown".into()),
        ] {
            let (tx, rx) = crossbeam_channel::bounded(1);
            pending.lock().unwrap().insert(3, tx);
            finish_pending_gui_requests(&pending, error.clone());
            assert_eq!(rx.recv().unwrap(), Err(error));
            assert!(pending.lock().unwrap().is_empty());
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        pending.lock().unwrap().insert(3, tx);
        route_gui_result(&pending, 2, GuiVisibilityOutcome::Shown); // Unrelated reply is ignored.
        assert!(rx.try_recv().is_err());
        route_gui_result(&pending, 3, GuiVisibilityOutcome::Error);
        assert!(matches!(
            rx.recv().unwrap(),
            Err(ConcurrentStateError::HostResponse(_))
        ));
    }

    #[test]
    fn state_responses_follow_request_ids_and_abort_on_bridge_exit() {
        let pending: PendingStateQueries = Arc::new(Mutex::new(HashMap::new()));
        let (first_tx, first_rx) = crossbeam_channel::bounded(1);
        let (second_tx, second_rx) = crossbeam_channel::bounded(1);
        pending.lock().unwrap().insert(11, first_tx);
        pending.lock().unwrap().insert(12, second_tx);

        route_concurrent_state_result(&pending, 12, Some("new".into()), None);
        assert_eq!(second_rx.try_recv().unwrap().unwrap(), "new");
        assert!(first_rx.try_recv().is_err());
        route_concurrent_state_result(&pending, 99, Some("stray".into()), None);
        assert!(first_rx.try_recv().is_err());
        abort_pending_state_queries(&pending, "interrupted: bridge exited");
        assert_eq!(
            first_rx.try_recv().unwrap().unwrap_err(),
            ConcurrentStateError::Interrupted("interrupted: bridge exited".into())
        );
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn state_error_does_not_enter_legacy_event_channel() {
        let pending: PendingStateQueries = Arc::new(Mutex::new(HashMap::new()));
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        pending.lock().unwrap().insert(42, reply_tx);
        route_concurrent_state_result(&pending, 42, None, Some("getState failed".into()));
        assert_eq!(
            reply_rx.try_recv().unwrap().unwrap_err(),
            ConcurrentStateError::HostResponse("getState failed".into())
        );
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        pending.lock().unwrap().insert(43, reply_tx);
        route_concurrent_state_result(&pending, 43, None, Some("interrupted".into()));
        assert_eq!(
            reply_rx.try_recv().unwrap().unwrap_err(),
            ConcurrentStateError::Interrupted("interrupted".into())
        );
    }

    #[test]
    fn stdout_eof_is_distinct_from_a_requested_capture_interruption() {
        let pending: PendingStateQueries = Arc::new(Mutex::new(HashMap::new()));
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        pending.lock().unwrap().insert(51, reply_tx);
        finish_pending_state_queries(&pending, ConcurrentStateError::HostExited);
        assert_eq!(
            reply_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            Err(ConcurrentStateError::HostExited)
        );
    }

    #[test]
    fn concurrent_capture_uses_plugin_state_event_with_request_id() {
        let event: Event =
            serde_json::from_str(r#"{"event":"plugin_state","request_id":42,"state":"YWJj"}"#)
                .unwrap();
        assert!(matches!(
            event,
            Event::PluginState {
                request_id: Some(42),
                state,
                ..
            } if state == "YWJj"
        ));
    }

    #[test]
    fn strict_open_protocol_field_is_explicit() {
        let command = Cmd::Open {
            plugin_path: "sample.vst3".into(),
            sample_rate: 48_000,
            block_size: 480,
            shm_name: "shm".into(),
            shm_size: 4096,
            sig_in: "in".into(),
            sig_out: "out".into(),
            state: Some("invalid".into()),
            strict_state: 1,
        };
        let value = serde_json::to_value(command).unwrap();
        assert_eq!(value["strict_state"], 1);
        assert_eq!(value["state"], "invalid");
    }

    #[cfg(windows)]
    #[test]
    fn audio_pipe_names_are_unique_even_at_one_timestamp() {
        let first = audio_pipe_names(123, 456);
        let second = audio_pipe_names(123, 456);
        assert_ne!(first.0, second.0);
        assert_ne!(first.1, second.1);
        assert_ne!(first.2, second.2);
    }

    #[cfg(windows)]
    #[test]
    fn existing_named_event_is_rejected() {
        let (_, event_name, _) = audio_pipe_names(std::process::id(), 0);
        let wide = HSTRING::from(event_name.as_str());
        unsafe {
            let first = CreateEventW(None, false, false, PCWSTR(wide.as_ptr())).unwrap();
            let second = CreateEventW(None, false, false, PCWSTR(wide.as_ptr())).unwrap();
            let error = reject_existing_handle(second, &event_name).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
            let _ = CloseHandle(first);
        }
    }
}

#[cfg(all(test, windows))]
mod effetune_host_handler_tests {
    use super::*;

    use std::os::windows::io::AsRawHandle;

    fn host_and_bundle() -> Option<(Bridge, String)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let host = root.join("vendor/vst3-host/mimageviewer-vst3-host.exe");
        let bundle = root.join("vendor/effetune-mixwright/EffeTune Mixwright.vst3");
        if !host.is_file() || !bundle.is_dir() {
            eprintln!("EffeTune host handler fixture unavailable; skipping native case");
            return None;
        }
        let bridge = Bridge::spawn(&host, |_| {}).unwrap();
        bridge
            .send(&Cmd::Hello {
                version: PROTOCOL_VERSION,
            })
            .unwrap();
        assert!(matches!(
            bridge
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            Event::Ready {
                version: PROTOCOL_VERSION
            }
        ));
        Some((bridge, bundle.to_string_lossy().into_owned()))
    }

    #[test]
    fn native_stdout_eof_resolves_exit_code_on_capture_worker() {
        let Some((bridge, _)) = host_and_bundle() else {
            return;
        };
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        bridge
            .pending_state_queries
            .lock()
            .unwrap()
            .insert(77, reply_tx);
        unsafe {
            TerminateProcess(
                HANDLE(bridge.child.as_raw_handle()),
                STATE_WATCHDOG_EXIT_CODE,
            )
            .unwrap();
        }
        assert_eq!(
            reply_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            Err(ConcurrentStateError::HostExited)
        );
        assert_eq!(
            bridge.wait_host_exit_code(std::time::Duration::from_secs(2)),
            Some(STATE_WATCHDOG_EXIT_CODE)
        );
    }

    #[test]
    fn strict_open_handler_rejects_invalid_restore_without_loaded_event() {
        let Some((mut bridge, bundle)) = host_and_bundle() else {
            return;
        };
        bridge
            .open_audio_pipe(&bundle, 48_000, 480, Some("not-base64!"), true)
            .unwrap();
        assert!(matches!(
            bridge.recv_timeout(std::time::Duration::from_secs(25)).unwrap(),
            Event::Error { detail } if detail.contains("restore_state: invalid base64")
        ));
        bridge
            .open_audio_pipe(&bundle, 48_000, 480, None, true)
            .unwrap();
        assert!(matches!(
            bridge
                .recv_timeout(std::time::Duration::from_secs(25))
                .unwrap(),
            Event::Loaded { slot_id: 0, .. }
        ));
    }

    #[test]
    fn strict_add_handler_rejects_invalid_restore_and_keeps_first_plugin() {
        let Some((mut bridge, bundle)) = host_and_bundle() else {
            return;
        };
        bridge
            .open_audio_pipe(&bundle, 48_000, 480, None, true)
            .unwrap();
        assert!(matches!(
            bridge
                .recv_timeout(std::time::Duration::from_secs(25))
                .unwrap(),
            Event::Loaded { slot_id: 0, .. }
        ));
        bridge
            .add_plugin_to_chain(1, &bundle, Some("not-base64!"), false, true)
            .unwrap();
        assert!(matches!(
            bridge.recv_timeout(std::time::Duration::from_secs(25)).unwrap(),
            Event::Error { detail } if detail.contains("add_plugin restore_state: invalid base64")
        ));
        let state = bridge.query_state_concurrent(0).unwrap();
        let encoded = state
            .recv_timeout(std::time::Duration::from_secs(6))
            .unwrap()
            .unwrap();
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for key in [
            "formatVersion",
            "pipelineA",
            "pipelineB",
            "currentPipeline",
            "masterBypass",
        ] {
            assert!(document.get(key).is_some(), "v0.11.1 codec field {key}");
        }
        assert_eq!(
            crate::effetune::EffectiveState::from_bytes(&bytes),
            crate::effetune::EffectiveState::Inert
        );
        assert_eq!(
            bridge
                .reset_sync_result_until(
                    std::time::Instant::now() - std::time::Duration::from_millis(1)
                )
                .unwrap_err(),
            "reset deadline expired"
        );
    }
}
