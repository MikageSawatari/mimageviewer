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
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE};
#[cfg(windows)]
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS,
    PAGE_READWRITE,
};
#[cfg(windows)]
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
#[cfg(windows)]
use windows::core::{HSTRING, PCWSTR};

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
    Hello { version: u32 },
    Open {
        plugin_path: String,
        sample_rate: u32,
        block_size: u32,
        shm_name: String,
        shm_size: u64,
        sig_in: String,
        sig_out: String,
    },
    /// シーク等で plugin の内部状態を flush する。`reset_id` は generation ID で、
    /// stale ack race を防ぐ (= timeout した過去 reset の ack が次回成功と誤認されない)。
    /// bridge は `Event::ResetDone { reset_id }` で同じ ID を返す。
    Reset { reset_id: u64 },
    Close,
    Shutdown,
    /// プラグイン GUI を指定 HWND にアタッチする。
    #[serde(rename = "show_gui")]
    ShowGui { hwnd: u64 },
    /// プラグイン GUI を外す。HWND の破棄は host 側の責務。
    #[serde(rename = "hide_gui")]
    HideGui,
    /// プラグインの推奨 GUI サイズだけ取得する (= attached しない)。
    /// host はこのサイズでウィンドウを作ってから ShowGui を送ることで、
    /// プラグインが子ウィンドウを正しいサイズで作成できる。
    #[serde(rename = "query_gui_size")]
    QueryGuiSize,
    /// ホストウィンドウのリサイズが起きたことをプラグインに通知する。
    /// bridge は view->onSize(rect) を呼んでプラグインの子ウィンドウを追従させる。
    #[serde(rename = "notify_host_resize")]
    NotifyHostResize { width: u32, height: u32 },
    /// 診断用: bridge 内で plugin を経由せずに in→out 単純コピーする。
    /// 歪みが消えれば plugin process が原因、残れば bridge パイプラインが原因。
    #[serde(rename = "set_passthrough")]
    SetPassthrough { enable: u32 },
    /// ユーザー drag によるリサイズが進行中かを bridge に通知する。
    /// `active=true` 中、bridge は plugin の `resizeView` callback で host HWND
    /// への `SetWindowPos` をスキップする (= ユーザー drag と plugin リサイズ要求の
    /// 衝突によるウィンドウ振動を抑止、Codex P4)。
    /// Rust 側 wndproc が `WM_ENTERSIZEMOVE` / `WM_EXITSIZEMOVE` を受けて発行する。
    #[serde(rename = "set_user_resizing")]
    SetUserResizing { active: u32 },
    /// プラグイン内部状態 (= EQ カーブ / chunk) を base64 文字列で取得する。
    /// 応答は `Event::PluginState`。終了時 / 永続化トリガで一度だけ呼ぶ想定。
    #[serde(rename = "query_state")]
    QueryState,
    /// 起動時の auto-restore: settings.json に保存されていた base64 state を渡し、
    /// bridge 側で `IComponent::setState` で復元する。fire-and-forget (= ack 無し)。
    /// 失敗時は bridge 側で `Event::Error` を発行する。
    #[serde(rename = "restore_state")]
    RestoreState { state: String },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "event")]
#[serde(rename_all = "snake_case")]
pub enum Event {
    Ready { version: u32 },
    Loaded {
        plugin_name: String,
        latency_samples: u32,
    },
    LatencyChanged { latency_samples: u32 },
    /// `Cmd::Reset { reset_id }` への応答。同じ `reset_id` をエコーで返す。
    /// 待機側はこれを照合して「自分が送った reset の ack か」を判定する
    /// (= stale ack race 防止、Codex 助言、2026-05-01)。
    ResetDone {
        #[serde(default)]
        reset_id: u64,
    },
    Closed,
    Error { detail: String },
    GuiAttached { width: u32, height: u32 },
    GuiDetached,
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
    PluginState { state: String },
}

/// bridge プロセスのハンドル。stdin/stdout と shared memory リソースを保持する。
pub struct Bridge {
    child: Child,
    stdin: Mutex<ChildStdin>,
    /// 同期 event 受信用 channel。spawn 時に起動した event-pump スレッドが
    /// stdout を読んで非同期 (LatencyChanged / ResetDone) 以外の event をここに流す。
    /// recv() はここから読む。
    event_rx: crossbeam_channel::Receiver<std::io::Result<Event>>,
    /// プラグインの最新 latency_samples (= bridge から非同期通知された値)。
    /// 初期値 = u32::MAX (= 「未受信」マーカ)。Loaded event 受信後に通常値が入る。
    /// audio-pump が `total_latency_samples()` から定期 polling して slot.latency_samples
    /// を更新する。プラグインが UI でモード切替して `restartComponent(kLatencyChanged)`
    /// を発火すると、event-pump がここを atomically 更新する。
    cached_latency_samples: Arc<AtomicU32>,
    /// シーク時の同期 reset 用 ack channel。bridge audio thread が in/out ring drain +
    /// `loader_->reset()` を実行した後に `Event::ResetDone { reset_id }` を返してくる。
    /// 一般 `event_rx` に流すと `query_gui_size` などの同期 recv() が誤って拾うので分離。
    /// 値は `reset_id` の世代 ID で、`wait_reset_done(expected_id)` が照合に使う
    /// (= stale ack race 防止、Codex 助言、2026-05-01)。
    reset_ack_rx: crossbeam_channel::Receiver<u64>,
    /// reset_sync helper が使う世代 ID counter。`fetch_add(1)` で発行する。
    next_reset_id: AtomicU64,
    #[cfg(windows)]
    shm: Option<SharedMemory>,
    #[cfg(windows)]
    sig_in: Option<EventHandle>,
    #[cfg(windows)]
    sig_out: Option<EventHandle>,
}

#[cfg(windows)]
struct SharedMemory {
    handle: HANDLE,
    base: MEMORY_MAPPED_VIEW_ADDRESS,
    size: u64,
    name: String,
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
    /// bridge exe を子プロセスとして起動する。
    /// `stderr_cb` は bridge プロセスの stderr に書かれた 1 行を受け取るコールバック。
    /// tester 側はこれを使ってログファイルにブリッジの内部状態 (show_gui の各ステップ等)
    /// を合流させる。バックグラウンドスレッドが子プロセス終了まで動き続ける。
    pub fn spawn<F>(exe_path: &std::path::Path, stderr_cb: F) -> std::io::Result<Self>
    where
        F: Fn(String) + Send + 'static,
    {
        let mut command = Command::new(exe_path);
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
        let (event_tx, event_rx) =
            crossbeam_channel::bounded::<std::io::Result<Event>>(64);
        // ResetDone 専用 ack channel。bridge audio thread が reset 実行後に流す
        // `Event::ResetDone { reset_id }` の reset_id をここに送る。
        // `wait_reset_done(expected_id)` が照合してから受理する (= stale ack 排除)。
        // bounded(8) は十分 (= 通常 1 個ずつ即消費、複数 reset 連続でも全 ID を保持)。
        let (reset_ack_tx, reset_ack_rx) = crossbeam_channel::bounded::<u64>(8);
        let cached_latency_for_pump = cached_latency.clone();
        std::thread::Builder::new()
            .name("bridge-event-pump".into())
            .spawn(move || {
                let mut stdout = stdout;
                loop {
                    match read_event_blocking(&mut stdout) {
                        Ok(Event::LatencyChanged { latency_samples }) => {
                            cached_latency_for_pump
                                .store(latency_samples, Ordering::Release);
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
                        Ok(other) => {
                            // Loaded を受信したら cached_latency にも反映する
                            // (= 起動直後は LatencyChanged 通知が来ないプラグインに対応)
                            if let Event::Loaded { latency_samples, .. } = &other {
                                cached_latency_for_pump
                                    .store(*latency_samples, Ordering::Release);
                            }
                            if event_tx.send(Ok(other)).is_err() {
                                break;  // receiver dropped
                            }
                        }
                        Err(e) => {
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
            cached_latency_samples: cached_latency,
            reset_ack_rx,
            next_reset_id: AtomicU64::new(0),
            #[cfg(windows)]
            shm: None,
            #[cfg(windows)]
            sig_in: None,
            #[cfg(windows)]
            sig_out: None,
        })
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
        // generation ID 発行 (= 0 から始まらないように +1 してから atomic に格納)。
        // wrapping_add で u64 overflow しても新規 ID として扱える (= 18 京回 reset で
        // overflow なので実用上発生しない)。
        let id = self
            .next_reset_id
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        if let Err(e) = self.send(&Cmd::Reset { reset_id: id }) {
            crate::logger::log(format!(
                "[VST3] reset_sync: send failed for id={id}: {e}"
            ));
            return false;
        }
        // ID 照合 loop (= 一致するまで old/future を drop)
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                return false;
            }
            match self.reset_ack_rx.recv_timeout(deadline - now) {
                Ok(got_id) if got_id == id => return true,
                Ok(got_id) => {
                    crate::logger::log(format!(
                        "[VST3] reset_sync: ignored stale ResetDone ack id={got_id}, expected={id}"
                    ));
                    // 続行して expected を待つ
                }
                Err(_) => return false,  // timeout
            }
        }
    }

    /// audio-pump が定期 polling する用: bridge から非同期通知された最新の
    /// latency_samples を取得する。Loaded event 未受信なら u32::MAX を返す
    /// (= 呼び出し側は「まだ不明」として 0 扱いに fallback すべき)。
    pub fn cached_latency_samples_value(&self) -> u32 {
        self.cached_latency_samples.load(Ordering::Acquire)
    }

    /// プラグイン内部状態 (= EQ カーブ / chunk) を base64 文字列で取得する。
    /// `Cmd::QueryState` を送って `Event::PluginState` を `timeout` 内で待つ。
    /// 期待外の event (= Error / 旧 reset 等) は **drop してログ** し、期待 event を
    /// 待ち続ける (= 他の同期 IPC と混線しないが、想定外があれば情報として残す)。
    /// 戻り値: Ok(state_b64) | Err(原因)。
    pub fn query_state_sync(
        &self,
        timeout: std::time::Duration,
    ) -> Result<String, String> {
        self.send(&Cmd::QueryState).map_err(|e| format!("send: {e}"))?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err("timeout".to_string());
            }
            match self.event_rx.recv_timeout(deadline - now) {
                Ok(Ok(Event::PluginState { state })) => return Ok(state),
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
}

/// stdout から 1 event を blocking で読む。bridge::Bridge spawn 時の event-pump
/// thread から呼ばれる。
fn read_event_blocking(stdout: &mut ChildStdout) -> std::io::Result<Event> {
    let mut len_buf = [0u8; 4];
    stdout.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    // `plugin_state` event の chunk は plugin によって数百 KB になる
    // (= ML / preset 内蔵 plugin)。C++ 側 `MAX_CONTROL_MSG_SIZE` と揃える。
    const MAX_CONTROL_MSG_SIZE: usize = 4 * 1024 * 1024;
    if len > MAX_CONTROL_MSG_SIZE {
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
    #[cfg(windows)]
    pub fn open_audio_pipe(
        &mut self,
        plugin_path: &str,
        sample_rate: u32,
        block_size: u32,
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
        let shm_name = format!("miv-vst-shm-{}-{}", pid, stamp);
        let sig_in_name = format!("miv-vst-sigin-{}-{}", pid, stamp);
        let sig_out_name = format!("miv-vst-sigout-{}-{}", pid, stamp);

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
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("CreateFileMappingW: {e}")))?;
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
            });

            // events
            let win = HSTRING::from(sig_in_name.as_str());
            let sig_in = CreateEventW(None, false, false, PCWSTR(win.as_ptr()))
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("CreateEventW sig_in: {e}")))?;
            let wout = HSTRING::from(sig_out_name.as_str());
            let sig_out = CreateEventW(None, false, false, PCWSTR(wout.as_ptr()))
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("CreateEventW sig_out: {e}")))?;
            self.sig_in = Some(EventHandle {
                handle: sig_in,
                name: sig_in_name.clone(),
            });
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
        })?;
        Ok(())
    }

    /// host から bridge への音声書き込み (= in_ring に push、sig_in 発火)。
    /// `samples` は f32 packed stereo。
    #[cfg(windows)]
    pub fn push_audio(&self, samples: &[f32]) -> std::io::Result<()> {
        let shm = self.shm.as_ref().ok_or_else(|| std::io::Error::other("audio pipe not open"))?;
        let sig_in = self.sig_in.as_ref().ok_or_else(|| std::io::Error::other("sig_in missing"))?;
        unsafe {
            let header = shm.base.Value as *mut ShmHeader;
            let cap = std::ptr::addr_of!((*header).capacity).read_unaligned();
            let in_ring = (shm.base.Value as *mut u8).add(std::mem::size_of::<ShmHeader>()) as *mut f32;

            let w_pos = (*header).in_write.load(Ordering::Relaxed);
            // overflow 防止のため modulo 操作を慎重に
            for (i, &s) in samples.iter().enumerate() {
                let idx = (w_pos.wrapping_add(i as u32)) % cap;
                in_ring.add(idx as usize).write(s);
            }
            (*header).in_write.store(w_pos.wrapping_add(samples.len() as u32), Ordering::Release);
            let _ = SetEvent(sig_in.handle);
        }
        Ok(())
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
        let shm = self.shm.as_ref().ok_or_else(|| std::io::Error::other("audio pipe not open"))?;
        let sig_out = self.sig_out.as_ref().ok_or_else(|| std::io::Error::other("sig_out missing"))?;
        let want = dst.len() as u32;
        if want == 0 {
            return Ok(0);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
        let mut total_taken: u32 = 0;
        unsafe {
            let header = shm.base.Value as *mut ShmHeader;
            let cap = std::ptr::addr_of!((*header).capacity).read_unaligned();
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
        let _ = self.send(&Cmd::Shutdown);
        let _ = self.child.wait();
        Ok(())
    }

    /// `Arc<Bridge>` から呼ぶ用の shutdown。`shutdown` 命令を送るだけで exit 待ちはしない。
    /// 子プロセスは shutdown を受信して自発的に exit するか、Arc の最後の参照が落ちた時点で
    /// Drop 経路で kill される。
    pub fn shutdown_async(&self) -> std::io::Result<()> {
        let _ = self.send(&Cmd::Shutdown);
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for Bridge {
    fn drop(&mut self) {
        if let Some(s) = self.shm.take() {
            unsafe {
                let _ = UnmapViewOfFile(s.base);
                let _ = CloseHandle(s.handle);
            }
            let _ = s.name; // suppress unused
            let _ = s.size;
        }
        for h in [self.sig_in.take(), self.sig_out.take()].into_iter().flatten() {
            unsafe {
                let _ = CloseHandle(h.handle);
            }
            let _ = h.name;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
