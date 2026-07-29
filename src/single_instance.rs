//! シングルインスタンス検出 + 起動要求 (v0.9)。
//!
//! 目的:
//! 1. トレイ常駐中に exe が 2 重起動されても 2 個並列には動かないこと
//!    (同じ APPDATA 配下の SQLite DB に別々の writer が付くのを避ける)。
//! 2. インストーラ (Inno Setup) の `AppMutex` と名前を合わせることで、
//!    アップデート時に「mImageViewer を閉じてください」ダイアログが自動で出る。
//! 3. 2 重起動しようとしたときに、既存インスタンスのウィンドウを復帰させる
//!    (= タスクトレイ常駐中でも "再度 mIV を起動" でウィンドウが戻ってくる体験)。
//!
//! ## 動作
//!
//! 起動直後に Windows の Named Mutex (`Global\mImageViewerInstance_v1`) を作成する。
//! 既に存在していた場合 (= 別プロセスが掴んでいる) は、2 重起動と判断して
//! `is_first_instance() = false` が返る。そのときは `signal_activate_existing()` で
//! 共有 Named Event を SetEvent して既存インスタンスに「起きろ」と通知してから exit する。
//!
//! 既存インスタンス側では `spawn_activation_listener(hwnd)` で waiter スレッドを
//! 起動しておく。SetEvent で起床して `ShowWindow + SetForegroundWindow` を直接呼ぶ。
//!
//! ## 名前の互換性
//!
//! - Mutex 名: `installer/mimageviewer.iss` の `AppMutex` と一致必須
//! - Event 名: アプリ内部のみで使用するので変更可
//!
//! ## 非 Windows
//!
//! 非 Windows では常に「1 個目」扱いにして機能を no-op 化する。

/// プロセス終了まで保持される単一インスタンスガード。
/// 1 個目のインスタンスは mutex + activate event の両方を取得し、2 個目以降は
/// mutex 取得失敗で `is_first_instance() = false` となる。
pub struct SingleInstanceGuard {
    #[cfg(windows)]
    _mutex: windows::Win32::Foundation::HANDLE,
    is_first: bool,
}

/// 1 個目のインスタンスが `SingleInstanceGuard::acquire` 内で作成した
/// activate event の生 HANDLE 値を保持する process-wide スロット。
/// `spawn_activation_listener` が読んで待機に使う。
///
/// mutex 取得と同じ `acquire` 内で作られるので、2 個目が `OpenEventW` に失敗する
/// レース (「mutex は存在するが event はまだ」) が起きない。
#[cfg(windows)]
static ACTIVATE_EVENT_RAW: std::sync::OnceLock<isize> = std::sync::OnceLock::new();

/// Inno Setup インストーラから呼ばれ、既存インスタンスにクリーン終了を要求するための
/// shutdown event の生 HANDLE。トレイ常駐中でも、インストーラが `SetEvent` すれば
/// アプリは tray "終了" と同じ on_exit 経由で抜けるので、ユーザが手動でトレイを
/// 操作しなくても次ステップに進めるようになる。
#[cfg(windows)]
static SHUTDOWN_EVENT_RAW: std::sync::OnceLock<isize> = std::sync::OnceLock::new();

/// インストーラの `AppMutex` と一致させる mutex 名。
///
/// - `Global\` プレフィックス: ターミナルサービス (リモートデスクトップ) 配下でも
///   全セッションを通じて一意にする。管理者権限なしでも作成可能。
/// - `_v1`: 将来の破壊的変更時に名前空間を切り替える余地。
///
/// portable ビルドは別名前空間 (`_portable_v1`) にして、インストール版がトレイ常駐中でも
/// ポータブル版を独立起動できるようにする (別 data_dir なので DB 衝突なし)。
/// ⚠ **非 portable の定義を先に置くこと**: launcher の `build.rs` (`extract_const`) が
/// 最初の `pub const MUTEX_NAME` 行を拾う。launcher は常に非 portable なので非 portable 名が
/// 先頭でないと埋め込み名がずれる。ACTIVATE_EVENT_NAME も同様。
#[cfg(not(feature = "portable"))]
pub const MUTEX_NAME: &str = "Global\\mImageViewerInstance_v1";
#[cfg(feature = "portable")]
pub const MUTEX_NAME: &str = "Global\\mImageViewerInstance_portable_v1";

impl SingleInstanceGuard {
    /// `MUTEX_NAME` で Named Mutex を作成する。既存なら `is_first_instance() = false`。
    /// 1 個目の場合は activate event も一緒に作成して後段の listener が使えるようにする。
    pub fn acquire() -> Self {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
            use windows::Win32::System::Threading::{CreateEventW, CreateMutexW};
            use windows::core::PCWSTR;

            let mutex_name: Vec<u16> = MUTEX_NAME.encode_utf16().chain([0]).collect();
            let (mutex_handle, is_first) =
                match unsafe { CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr())) } {
                    Ok(h) => {
                        let is_first = unsafe { GetLastError() } != ERROR_ALREADY_EXISTS;
                        (h, is_first)
                    }
                    Err(e) => {
                        crate::logger::log(format!(
                            "single_instance: CreateMutexW failed: {e:?} — continuing as first"
                        ));
                        (windows::Win32::Foundation::HANDLE::default(), true)
                    }
                };

            // 1 個目のインスタンスは activate / shutdown event も即座に作成する。
            // 2 個目起動は mutex check → `signal_activate_existing` で `OpenEventW`
            // するので、event の存在が mutex の存在と同期している必要がある。
            // インストーラ (`SignalAppShutdown` from Inno Setup) も同様に shutdown event を
            // `OpenEventW` するので、こちらも同時に作る。
            if is_first {
                let activate_wide: Vec<u16> =
                    ACTIVATE_EVENT_NAME.encode_utf16().chain([0]).collect();
                // auto-reset (`bManualReset=false`) + 初期 non-signaled
                match unsafe { CreateEventW(None, false, false, PCWSTR(activate_wide.as_ptr())) } {
                    Ok(h) => {
                        let _ = ACTIVATE_EVENT_RAW.set(h.0 as isize);
                    }
                    Err(e) => {
                        crate::logger::log(format!(
                            "single_instance: CreateEventW(activate) failed: {e:?}"
                        ));
                    }
                }
                let shutdown_wide: Vec<u16> =
                    SHUTDOWN_EVENT_NAME.encode_utf16().chain([0]).collect();
                match unsafe { CreateEventW(None, false, false, PCWSTR(shutdown_wide.as_ptr())) } {
                    Ok(h) => {
                        let _ = SHUTDOWN_EVENT_RAW.set(h.0 as isize);
                    }
                    Err(e) => {
                        crate::logger::log(format!(
                            "single_instance: CreateEventW(shutdown) failed: {e:?}"
                        ));
                    }
                }
            }

            SingleInstanceGuard {
                _mutex: mutex_handle,
                is_first,
            }
        }
        #[cfg(not(windows))]
        {
            SingleInstanceGuard { is_first: true }
        }
    }

    /// 今のプロセスが 1 個目 (mutex の新規作成者) か。
    /// false のときは既にもう 1 つ mImageViewer が動いている。
    pub fn is_first_instance(&self) -> bool {
        self.is_first
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::CloseHandle;
            unsafe {
                if !self._mutex.is_invalid() {
                    let _ = CloseHandle(self._mutex);
                }
            }
            // activate event は listener が CloseHandle するので触らない。
        }
    }
}

/// 2 重起動時に既存インスタンスに投げる「起きて前面に来い」シグナルの Named Event 名。
///
/// Global\ プレフィックスで全セッション共有。Auto-reset (マニュアル reset=FALSE) に
/// することで、シグナルが届いた瞬間に自動で non-signaled に戻る → 連続再トリガ可能。
/// portable は別名前空間にして、インストール版の activate event を叩かないようにする。
#[cfg(not(feature = "portable"))]
pub const ACTIVATE_EVENT_NAME: &str = "Global\\mImageViewerActivate_v1";
#[cfg(feature = "portable")]
pub const ACTIVATE_EVENT_NAME: &str = "Global\\mImageViewerActivate_portable_v1";

/// 2 重起動されたプロセスから既存インスタンスへ「このパスを開いてほしい」を渡す
/// Named Pipe。activate event は値を運べないので、ウィンドウ復帰通知とは別に
/// 小さな byte stream IPC を使う。
#[cfg(not(feature = "portable"))]
pub const OPEN_PATH_PIPE_NAME: &str = r"\\.\pipe\mImageViewerOpenPath_v1";
#[cfg(feature = "portable")]
pub const OPEN_PATH_PIPE_NAME: &str = r"\\.\pipe\mImageViewerOpenPath_portable_v1";

/// インストーラ (Inno Setup `[Code]` / `SignalAppShutdown`) が、既存の mIV インスタンスに
/// クリーン終了を要求するための Named Event 名。トレイ常駐中のインスタンスでも
/// この event を拾うと `on_exit` 経由で DB 書き出し + 設定保存してから抜けるので、
/// 「インストール時にトレイの "終了" を手動で押す」手順が要らなくなる。
#[cfg(not(feature = "portable"))]
pub const SHUTDOWN_EVENT_NAME: &str = "Global\\mImageViewerShutdown_v1";
// portable はインストーラ非経由なので shutdown event 連携は使わないが、名前空間は分離しておく。
#[cfg(feature = "portable")]
pub const SHUTDOWN_EVENT_NAME: &str = "Global\\mImageViewerShutdown_portable_v1";

/// 既存インスタンスに「ウィンドウを復帰させろ」と通知する。Windows 専用。
/// 2 重起動検出時に呼び、既存プロセスが応答してから自プロセスを終了する想定。
pub fn signal_activate_existing() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
        use windows::core::PCWSTR;

        let name_wide: Vec<u16> = ACTIVATE_EVENT_NAME.encode_utf16().chain([0]).collect();
        unsafe {
            match OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(name_wide.as_ptr())) {
                Ok(handle) => {
                    let ok = SetEvent(handle).is_ok();
                    let _ = CloseHandle(handle);
                    if !ok {
                        crate::logger::log("single_instance: SetEvent failed");
                    }
                    ok
                }
                Err(e) => {
                    crate::logger::log(format!(
                        "single_instance: OpenEventW failed (existing instance may not be ready): {e:?}"
                    ));
                    false
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
const OPEN_PATH_MAX_U16: usize = 32_767;
#[cfg(windows)]
const OPEN_PATH_PIPE_BUFFER_BYTES: u32 = 64 * 1024;
#[cfg(windows)]
const OPEN_PATH_CONNECT_ATTEMPTS: usize = 20;
#[cfg(windows)]
const OPEN_PATH_NOT_FOUND_RETRY_MS: u64 = 25;
#[cfg(windows)]
const OPEN_PATH_BUSY_WAIT_MS: u32 = 50;

/// 2 重起動時に既存インスタンスへ「開くパス」を送る。Windows 専用。
/// 成否に関わらず呼び出し側は `signal_activate_existing()` で前面化も要求する。
pub fn send_open_path_to_existing(path: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        let Some(message) = encode_open_path_message(path) else {
            crate::logger::log(format!(
                "single_instance: open path is too long or empty: {}",
                path.display()
            ));
            return false;
        };
        send_open_path_message_to_existing(&message, true)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

#[cfg(windows)]
fn encode_open_path_message(path: &std::path::Path) -> Option<Vec<u8>> {
    use std::os::windows::ffi::OsStrExt;

    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.is_empty() || wide.len() > OPEN_PATH_MAX_U16 {
        return None;
    }
    let mut message = Vec::with_capacity(4 + wide.len() * 2);
    message.extend_from_slice(&(wide.len() as u32).to_le_bytes());
    for unit in wide {
        message.extend_from_slice(&unit.to_le_bytes());
    }
    Some(message)
}

#[cfg(windows)]
fn decode_open_path_message(message: &[u8]) -> Option<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    if message.len() < 4 {
        return None;
    }
    let len = u32::from_le_bytes(message[0..4].try_into().ok()?) as usize;
    if len == 0 || len > OPEN_PATH_MAX_U16 {
        return None;
    }
    let body_len = len.checked_mul(2)?;
    if message.len() != 4 + body_len {
        return None;
    }
    let mut units = Vec::with_capacity(len);
    for chunk in message[4..].chunks_exact(2) {
        units.push(u16::from_le_bytes([chunk[0], chunk[1]]));
    }
    Some(std::path::PathBuf::from(std::ffi::OsString::from_wide(
        &units,
    )))
}

#[cfg(windows)]
fn open_path_pipe_name_wide() -> Vec<u16> {
    OPEN_PATH_PIPE_NAME.encode_utf16().chain([0]).collect()
}

#[cfg(windows)]
fn send_open_path_message_to_existing(message: &[u8], retry_not_found: bool) -> bool {
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GetLastError,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };
    use windows::Win32::System::Pipes::WaitNamedPipeW;
    use windows::core::PCWSTR;

    let name_wide = open_path_pipe_name_wide();
    let max_attempts = if retry_not_found {
        OPEN_PATH_CONNECT_ATTEMPTS
    } else {
        1
    };
    for attempt in 0..max_attempts {
        let handle = unsafe {
            CreateFileW(
                PCWSTR(name_wide.as_ptr()),
                FILE_GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };
        match handle {
            Ok(handle) => {
                let ok = write_pipe_all(handle, message);
                unsafe {
                    let _ = CloseHandle(handle);
                }
                if !ok {
                    crate::logger::log("single_instance: open path pipe write failed");
                }
                return ok;
            }
            Err(e) => {
                let last = unsafe { GetLastError() };
                if last == ERROR_PIPE_BUSY {
                    unsafe {
                        let _ = WaitNamedPipeW(PCWSTR(name_wide.as_ptr()), OPEN_PATH_BUSY_WAIT_MS);
                    }
                    continue;
                }
                if retry_not_found && last == ERROR_FILE_NOT_FOUND && attempt + 1 < max_attempts {
                    std::thread::sleep(std::time::Duration::from_millis(
                        OPEN_PATH_NOT_FOUND_RETRY_MS,
                    ));
                    continue;
                }
                crate::logger::log(format!(
                    "single_instance: open path pipe connect failed: {e:?}"
                ));
                return false;
            }
        }
    }
    crate::logger::log("single_instance: open path pipe connect timed out");
    false
}

#[cfg(windows)]
fn write_pipe_all(handle: windows::Win32::Foundation::HANDLE, mut bytes: &[u8]) -> bool {
    use windows::Win32::Storage::FileSystem::WriteFile;

    while !bytes.is_empty() {
        let mut written = 0_u32;
        if unsafe { WriteFile(handle, Some(bytes), Some(&mut written), None) }.is_err()
            || written == 0
        {
            return false;
        }
        bytes = &bytes[written as usize..];
    }
    true
}

#[cfg(windows)]
fn poke_open_path_listener() {
    let message = 0_u32.to_le_bytes();
    let _ = send_open_path_message_to_existing(&message, false);
}

#[cfg(windows)]
fn spawn_open_path_listener(
    stop_event_raw: isize,
    egui_ctx: eframe::egui::Context,
    open_path_tx: std::sync::mpsc::Sender<std::path::PathBuf>,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("mimv-open-path-listener".into())
        .spawn(move || open_path_listener_loop(stop_event_raw, egui_ctx, open_path_tx))
        .ok()
}

#[cfg(windows)]
fn open_path_listener_loop(
    stop_event_raw: isize,
    egui_ctx: eframe::egui::Context,
    open_path_tx: std::sync::mpsc::Sender<std::path::PathBuf>,
) {
    use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, GetLastError, HANDLE};
    use windows::Win32::Storage::FileSystem::PIPE_ACCESS_INBOUND;
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, NAMED_PIPE_MODE,
        PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows::core::PCWSTR;

    let stop_event = HANDLE(stop_event_raw as *mut _);
    let name_wide = open_path_pipe_name_wide();
    loop {
        if stop_event_is_set(stop_event) {
            break;
        }

        // PIPE_REJECT_REMOTE_CLIENTS: named pipe は既定で SMB 経由のリモート接続も
        // 受け付けるが、これは同一マシン内の単一インスタンス連携専用なのでリモート
        // クライアントを拒否して攻撃面を縮小する (post-v1.3.0 backlog 堅牢化)。
        let pipe_mode = NAMED_PIPE_MODE(
            PIPE_TYPE_BYTE.0 | PIPE_READMODE_BYTE.0 | PIPE_WAIT.0 | PIPE_REJECT_REMOTE_CLIENTS.0,
        );
        let pipe = unsafe {
            CreateNamedPipeW(
                PCWSTR(name_wide.as_ptr()),
                PIPE_ACCESS_INBOUND,
                pipe_mode,
                1,
                0,
                OPEN_PATH_PIPE_BUFFER_BYTES,
                0,
                None,
            )
        };
        if pipe.is_invalid() {
            crate::logger::log("single_instance: CreateNamedPipeW(open path) failed");
            break;
        }

        let connected = unsafe {
            ConnectNamedPipe(pipe, None).is_ok() || GetLastError() == ERROR_PIPE_CONNECTED
        };
        if !connected {
            unsafe {
                let _ = CloseHandle(pipe);
            }
            continue;
        }

        let path = read_open_path_message(pipe);
        unsafe {
            let _ = DisconnectNamedPipe(pipe);
            let _ = CloseHandle(pipe);
        }

        if stop_event_is_set(stop_event) {
            break;
        }
        let Some(path) = path else {
            continue;
        };
        crate::logger::log(format!(
            "single_instance: received open path: {}",
            path.display()
        ));
        if open_path_tx.send(path).is_err() {
            break;
        }
        egui_ctx.request_repaint();
    }
    crate::logger::log("single_instance: open path listener thread exiting");
}

#[cfg(windows)]
fn stop_event_is_set(stop_event: windows::Win32::Foundation::HANDLE) -> bool {
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::System::Threading::WaitForSingleObject;

    unsafe { WaitForSingleObject(stop_event, 0) == WAIT_OBJECT_0 }
}

#[cfg(windows)]
fn read_open_path_message(pipe: windows::Win32::Foundation::HANDLE) -> Option<std::path::PathBuf> {
    let mut message = read_pipe_exact(pipe, 4)?;
    let len = u32::from_le_bytes(message[0..4].try_into().ok()?) as usize;
    if len == 0 {
        return None;
    }
    if len > OPEN_PATH_MAX_U16 {
        crate::logger::log(format!(
            "single_instance: open path message too long: {len} u16"
        ));
        return None;
    }
    let body = read_pipe_exact(pipe, len.checked_mul(2)?)?;
    message.extend_from_slice(&body);
    decode_open_path_message(&message)
}

#[cfg(windows)]
fn read_pipe_exact(handle: windows::Win32::Foundation::HANDLE, len: usize) -> Option<Vec<u8>> {
    use windows::Win32::Storage::FileSystem::ReadFile;

    let mut buf = vec![0_u8; len];
    let mut offset = 0;
    while offset < len {
        let mut read = 0_u32;
        if unsafe { ReadFile(handle, Some(&mut buf[offset..]), Some(&mut read), None) }.is_err()
            || read == 0
        {
            return None;
        }
        offset += read as usize;
    }
    Some(buf)
}

/// アクティベーションリスナースレッドのハンドル。
/// Drop でシャットダウン: `stop_event` を signal → waiter スレッドが抜ける。
pub struct ActivationListener {
    #[cfg(windows)]
    stop_event: windows::Win32::Foundation::HANDLE,
    thread: Option<std::thread::JoinHandle<()>>,
    #[cfg(windows)]
    path_thread: Option<std::thread::JoinHandle<()>>,
}

/// メインプロセス側のシングルインスタンスリスナーを起動する。3 種のイベントを待機:
/// - **activate**: 2 重起動を試みた別プロセスが `signal_activate_existing()` を呼んだら、
///   `placement_slot` (あれば) を復元 + `ShowWindow` + `SetForegroundWindow` で前面に戻す
/// - **shutdown**: インストーラ (`SignalAppShutdown`) が発火したら、`shutdown_requested`
///   を立てて WM_CLOSE を投げ、tray 右クリック「終了」と同じクリーン終了経路に合流
///   (トレイ常駐で非表示なら DWM cloak を併用してフラッシュを抑える)
/// - **stop**: Drop 時にハンドルを解放してスレッドを畳む内部用
///
/// - `placement_slot`: トレイ hide 時に保存された `WINDOWPLACEMENT`。ある場合は
///   `SetWindowPlacement` で DPI 丸めを回避しつつ元のサイズ・位置で復元する。
/// - `shutdown_requested`: shutdown event を受けたときに `true` を書き込む共有フラグ。
///   `App::maybe_intercept_close` が見て「このクローズは本当に終了」と判断する。
///
/// activate / shutdown event は `SingleInstanceGuard::acquire` が OnceLock に既に登録
/// しているのでここで作成は不要。
pub fn spawn_activation_listener(
    hwnd_raw: isize,
    egui_ctx: eframe::egui::Context,
    placement_slot: crate::tray::PlacementSlot,
    shutdown_requested: std::sync::Arc<std::sync::atomic::AtomicBool>,
    open_path_tx: std::sync::mpsc::Sender<std::path::PathBuf>,
) -> Option<ActivationListener> {
    #[cfg(windows)]
    {
        use std::sync::atomic::Ordering;
        use windows::Win32::Foundation::{CloseHandle, HANDLE, LPARAM, WPARAM};
        use windows::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0};
        use windows::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
        use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForMultipleObjects};
        use windows::Win32::UI::WindowsAndMessaging::{
            IsWindowVisible, PostMessageW, SW_SHOW, SW_SHOWNOACTIVATE, SetForegroundWindow,
            ShowWindow, WM_CLOSE,
        };
        use windows::core::PCWSTR;

        let Some(&activate_event_raw) = ACTIVATE_EVENT_RAW.get() else {
            crate::logger::log(
                "single_instance: ACTIVATE_EVENT_RAW not set (guard did not create event) — skipping listener",
            );
            return None;
        };
        // shutdown event は None でも activate listener 自体は動かせるように扱う
        // (古い DLL / まれに CreateEvent 失敗した場合のフォールバック)。
        let shutdown_event_raw = SHUTDOWN_EVENT_RAW.get().copied().unwrap_or(0);
        let stop_event: HANDLE = match unsafe { CreateEventW(None, true, false, PCWSTR::null()) } {
            Ok(h) => h,
            Err(e) => {
                crate::logger::log(format!("single_instance: CreateEventW(stop) failed: {e:?}"));
                return None;
            }
        };

        struct HandleSet {
            activate: isize,
            shutdown: isize,
            stop: isize,
        }
        let handles = HandleSet {
            activate: activate_event_raw,
            shutdown: shutdown_event_raw,
            stop: stop_event.0 as isize,
        };
        let path_thread =
            spawn_open_path_listener(stop_event.0 as isize, egui_ctx.clone(), open_path_tx);

        let thread = std::thread::Builder::new()
            .name("mimv-activate-listener".into())
            .spawn(move || {
                let activate = HANDLE(handles.activate as *mut _);
                let stop = HANDLE(handles.stop as *mut _);
                // shutdown が作成失敗していた場合は、stop イベントを代わりに並べて
                // 「発火しない 2 番目」として扱う。インデックスは activate / shutdown / stop
                // で固定しておき、見やすさを保つ。
                let shutdown = if handles.shutdown != 0 {
                    HANDLE(handles.shutdown as *mut _)
                } else {
                    stop
                };
                let wait_handles = [activate, shutdown, stop];
                loop {
                    let r = unsafe { WaitForMultipleObjects(&wait_handles, false, INFINITE) };
                    let hwnd = windows::Win32::Foundation::HWND(hwnd_raw as *mut _);
                    if r == WAIT_OBJECT_0 {
                        crate::logger::log(
                            "single_instance: activate event signaled — restoring window",
                        );
                        // placement_slot に hide 時の WINDOWPLACEMENT があれば先に復元し、
                        // DPI 丸めによるサイズ / 位置のズレを回避する (トレイ Open と同じ挙動)。
                        // ⚠ ロックを握ったまま `SetWindowPlacement` を呼ばないこと。対象 HWND は
                        // UI スレッド所有なので、この呼び出しは UI スレッドがメッセージを
                        // 処理するまで戻らない。その間に UI スレッドがトレイ格納で同じ
                        // `placement_slot` を書きに来ると相互待ちになる
                        // (`tray_integration.rs` の hide 経路)。トレイスレッド側は同じ形で
                        // 自己デッドロックしていた (2026-07-30 実害)。
                        // 取り出しだけロック内で行い、guard を落としてから Win32 を呼ぶ。
                        let saved = placement_slot.lock().unwrap().take();
                        let used_placement = if let Some(p) = saved {
                            crate::tray::restore_window_placement(hwnd_raw, &p);
                            true
                        } else {
                            false
                        };
                        unsafe {
                            if !used_placement {
                                let _ = ShowWindow(hwnd, SW_SHOW);
                            }
                            let _ = SetForegroundWindow(hwnd);
                        }
                        egui_ctx.request_repaint();
                    } else if r.0 == WAIT_OBJECT_0.0 + 1 && handles.shutdown != 0 {
                        crate::logger::log(
                            "single_instance: shutdown event signaled — requesting clean exit",
                        );
                        shutdown_requested.store(true, Ordering::SeqCst);
                        // トレイ常駐で非表示なら DWM cloak してから ShowWindow → WM_CLOSE。
                        // (tray Quit と同じ経路で、終了時のウィンドウ一瞬復帰を抑制する)
                        unsafe {
                            if !IsWindowVisible(hwnd).as_bool() {
                                let cloak_true: i32 = 1;
                                let _ = DwmSetWindowAttribute(
                                    hwnd,
                                    DWMWA_CLOAK,
                                    &cloak_true as *const _ as *const _,
                                    std::mem::size_of::<i32>() as u32,
                                );
                                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                            }
                            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                        }
                        egui_ctx.request_repaint();
                    } else if r.0 == WAIT_OBJECT_0.0 + 2
                        || (handles.shutdown == 0 && r.0 == WAIT_OBJECT_0.0 + 1)
                    {
                        // shutdown 作成失敗時は wait_handles = [activate, stop, stop] で
                        // stop が index 1 として最小一致で返る。その index 1 もここで
                        // break させないと、index-1 分岐 (上, shutdown != 0 gate) を skip
                        // して else { continue } に落ち、stop が立ったまま busy-spin →
                        // Drop の join() が永久ブロックする (v1.0.0 安定性レビュー P3-1)。
                        break;
                    } else if r == WAIT_FAILED {
                        crate::logger::log("single_instance: WaitForMultipleObjects failed");
                        break;
                    } else {
                        continue;
                    }
                }
                unsafe {
                    let _ = CloseHandle(activate);
                    if handles.shutdown != 0 {
                        let _ = CloseHandle(HANDLE(handles.shutdown as *mut _));
                    }
                }
                crate::logger::log("single_instance: listener thread exiting");
            })
            .ok()?;

        Some(ActivationListener {
            stop_event,
            thread: Some(thread),
            path_thread,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (
            hwnd_raw,
            egui_ctx,
            placement_slot,
            shutdown_requested,
            open_path_tx,
        );
        None
    }
}

impl Drop for ActivationListener {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::CloseHandle;
            use windows::Win32::System::Threading::SetEvent;
            unsafe {
                let _ = SetEvent(self.stop_event);
            }
            poke_open_path_listener();
            if let Some(th) = self.thread.take() {
                let _ = th.join();
            }
            if let Some(th) = self.path_thread.take() {
                let _ = th.join();
            }
            unsafe {
                let _ = CloseHandle(self.stop_event);
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn open_path_message_round_trips_unicode_path() {
        let path = std::path::PathBuf::from(r"C:\books\日本語\book.cbr");
        let message = encode_open_path_message(&path).expect("encoded");
        assert_eq!(decode_open_path_message(&message), Some(path));
    }

    #[test]
    fn open_path_message_rejects_empty_wake_message() {
        assert_eq!(decode_open_path_message(&0_u32.to_le_bytes()), None);
    }
}
