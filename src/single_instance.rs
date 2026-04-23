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
pub struct SingleInstanceGuard {
    #[cfg(windows)]
    _handle: windows::Win32::Foundation::HANDLE,
    is_first: bool,
}

/// インストーラの `AppMutex` と一致させる mutex 名。
///
/// - `Global\` プレフィックス: ターミナルサービス (リモートデスクトップ) 配下でも
///   全セッションを通じて一意にする。管理者権限なしでも作成可能。
/// - `_v1`: 将来の破壊的変更時に名前空間を切り替える余地。
pub const MUTEX_NAME: &str = "Global\\mImageViewerInstance_v1";

impl SingleInstanceGuard {
    /// `MUTEX_NAME` で Named Mutex を作成する。既存なら `is_first_instance() = false`。
    pub fn acquire() -> Self {
        #[cfg(windows)]
        {
            use windows::core::PCWSTR;
            use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
            use windows::Win32::System::Threading::CreateMutexW;

            let name_wide: Vec<u16> = MUTEX_NAME.encode_utf16().chain([0]).collect();
            match unsafe { CreateMutexW(None, false, PCWSTR(name_wide.as_ptr())) } {
                Ok(handle) => {
                    // GetLastError が ERROR_ALREADY_EXISTS なら既存 mutex を開いたので 2 個目。
                    let is_first = unsafe { GetLastError() } != ERROR_ALREADY_EXISTS;
                    SingleInstanceGuard {
                        _handle: handle,
                        is_first,
                    }
                }
                Err(e) => {
                    // 作成失敗は通常起きないが、起きたら「自分が 1 個目」扱いにして続行。
                    // (GetLastError = ACCESS_DENIED などは Global\ prefix 制約だがここでは
                    //  管理者権限不要の作成範囲なので基本起きない)
                    crate::logger::log(format!(
                        "single_instance: CreateMutexW failed: {e:?} — continuing as first"
                    ));
                    SingleInstanceGuard {
                        _handle: windows::Win32::Foundation::HANDLE::default(),
                        is_first: true,
                    }
                }
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

/// 2 重起動時に既存インスタンスに投げる「起きて前面に来い」シグナルの Named Event 名。
///
/// Global\ プレフィックスで全セッション共有。Auto-reset (マニュアル reset=FALSE) に
/// することで、シグナルが届いた瞬間に自動で non-signaled に戻る → 連続再トリガ可能。
pub const ACTIVATE_EVENT_NAME: &str = "Global\\mImageViewerActivate_v1";

/// 既存インスタンスに「ウィンドウを復帰させろ」と通知する。Windows 専用。
/// 2 重起動検出時に呼び、既存プロセスが応答してから自プロセスを終了する想定。
pub fn signal_activate_existing() -> bool {
    #[cfg(windows)]
    {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};

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

/// アクティベーションリスナースレッドのハンドル。
/// Drop でシャットダウン: `stop_event` を signal → waiter スレッドが抜ける。
pub struct ActivationListener {
    #[cfg(windows)]
    stop_event: windows::Win32::Foundation::HANDLE,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// メインプロセス側のアクティベーションリスナーを起動する。
/// 2 重起動を試みた別プロセスが `signal_activate_existing()` でイベントを発火したら、
/// waiter スレッドが `activate_window(hwnd)` を呼んでウィンドウを復帰させる。
///
/// `egui_ctx` は活性化後に UI スレッドを起こすための clone。
pub fn spawn_activation_listener(
    hwnd_raw: isize,
    egui_ctx: eframe::egui::Context,
) -> Option<ActivationListener> {
    #[cfg(windows)]
    {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        use windows::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{
            CreateEventW, WaitForMultipleObjects, INFINITE,
        };
        use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_SHOW};

        let name_wide: Vec<u16> = ACTIVATE_EVENT_NAME.encode_utf16().chain([0]).collect();
        let activate_event: HANDLE = match unsafe {
            // bManualReset=false (auto-reset), bInitialState=false
            CreateEventW(None, false, false, PCWSTR(name_wide.as_ptr()))
        } {
            Ok(h) => h,
            Err(e) => {
                crate::logger::log(format!(
                    "single_instance: CreateEventW(activate) failed: {e:?}"
                ));
                return None;
            }
        };
        let stop_event: HANDLE = match unsafe {
            // manual-reset=true で stop を立てたら以降ずっと signaled
            CreateEventW(None, true, false, PCWSTR::null())
        } {
            Ok(h) => h,
            Err(e) => {
                crate::logger::log(format!(
                    "single_instance: CreateEventW(stop) failed: {e:?}"
                ));
                unsafe {
                    let _ = CloseHandle(activate_event);
                }
                return None;
            }
        };

        // スレッド間で HANDLE (isize 互換) を Send するための包み
        struct HandlePair {
            activate: isize,
            stop: isize,
        }
        let handles = HandlePair {
            activate: activate_event.0 as isize,
            stop: stop_event.0 as isize,
        };

        let thread = std::thread::Builder::new()
            .name("mimv-activate-listener".into())
            .spawn(move || {
                let activate = HANDLE(handles.activate as *mut _);
                let stop = HANDLE(handles.stop as *mut _);
                let wait_handles = [activate, stop];
                loop {
                    let r =
                        unsafe { WaitForMultipleObjects(&wait_handles, false, INFINITE) };
                    if r == WAIT_OBJECT_0 {
                        // activate: 既存インスタンスへの起動要求
                        crate::logger::log(
                            "single_instance: activate event signaled — restoring window",
                        );
                        unsafe {
                            let hwnd = windows::Win32::Foundation::HWND(hwnd_raw as *mut _);
                            let _ = ShowWindow(hwnd, SW_SHOW);
                            let _ = SetForegroundWindow(hwnd);
                        }
                        egui_ctx.request_repaint();
                    } else if r.0 == WAIT_OBJECT_0.0 + 1 {
                        // stop: シャットダウン
                        break;
                    } else if r == WAIT_FAILED {
                        crate::logger::log("single_instance: WaitForMultipleObjects failed");
                        break;
                    } else {
                        // その他 (WAIT_ABANDONED 等)、無視して続行
                        continue;
                    }
                }
                unsafe {
                    let _ = CloseHandle(activate);
                    // stop は呼び出し側の Drop で閉じる。
                }
                crate::logger::log("single_instance: listener thread exiting");
            })
            .ok()?;

        Some(ActivationListener {
            stop_event,
            thread: Some(thread),
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd_raw, egui_ctx);
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
            if let Some(th) = self.thread.take() {
                let _ = th.join();
            }
            unsafe {
                let _ = CloseHandle(self.stop_event);
            }
        }
    }
}
