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
use std::sync::atomic::{AtomicU32, Ordering};
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
    Reset,
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
    ResetDone,
    Closed,
    Error { detail: String },
    GuiAttached { width: u32, height: u32 },
    GuiDetached,
    /// プラグインの推奨 GUI サイズ (query_gui_size の応答)。
    GuiSize { width: u32, height: u32 },
}

/// bridge プロセスのハンドル。stdin/stdout と shared memory リソースを保持する。
pub struct Bridge {
    child: Child,
    stdin: Mutex<ChildStdin>,
    // stdout は recv で blocking read するので Arc<Mutex<>> で保持
    stdout: Arc<Mutex<ChildStdout>>,
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
        let mut child = Command::new(exe_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
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
        Ok(Self {
            child,
            stdin: Mutex::new(stdin),
            stdout: Arc::new(Mutex::new(stdout)),
            #[cfg(windows)]
            shm: None,
            #[cfg(windows)]
            sig_in: None,
            #[cfg(windows)]
            sig_out: None,
        })
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
    pub fn recv(&self) -> std::io::Result<Event> {
        let mut stdout = self.stdout.lock().unwrap();
        let mut len_buf = [0u8; 4];
        stdout.read_exact(&mut len_buf)?;
        let len = u32::from_le_bytes(len_buf) as usize;
        if len > 64 * 1024 {
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
