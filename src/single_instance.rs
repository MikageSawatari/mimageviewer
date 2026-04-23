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

/// インストーラの `AppMutex` と一致させる mutex 名。
///
/// - `Global\` プレフィックス: ターミナルサービス (リモートデスクトップ) 配下でも
///   全セッションを通じて一意にする。管理者権限なしでも作成可能。
/// - `_v1`: 将来の破壊的変更時に名前空間を切り替える余地。
pub const MUTEX_NAME: &str = "Global\\mImageViewerInstance_v1";

impl SingleInstanceGuard {
    /// `MUTEX_NAME` で Named Mutex を作成する。既存なら `is_first_instance() = false`。
    /// 1 個目の場合は activate event も一緒に作成して後段の listener が使えるようにする。
    pub fn acquire() -> Self {
        #[cfg(windows)]
        {
            use windows::core::PCWSTR;
            use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
            use windows::Win32::System::Threading::{CreateEventW, CreateMutexW};

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

            // 1 個目のインスタンスは activate event も即座に作成する。
            // 2 個目起動は mutex check → `signal_activate_existing` で `OpenEventW`
            // するので、event の存在が mutex の存在と同期している必要がある。
            if is_first {
                let event_name: Vec<u16> =
                    ACTIVATE_EVENT_NAME.encode_utf16().chain([0]).collect();
                // auto-reset (`bManualReset=false`) + 初期 non-signaled
                match unsafe { CreateEventW(None, false, false, PCWSTR(event_name.as_ptr())) }
                {
                    Ok(h) => {
                        let _ = ACTIVATE_EVENT_RAW.set(h.0 as isize);
                    }
                    Err(e) => {
                        crate::logger::log(format!(
                            "single_instance: CreateEventW(activate) failed: {e:?}"
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
/// waiter スレッドが placement_slot (あれば) を復元 + `ShowWindow` + `SetForegroundWindow`
/// で復帰させる。
///
/// - `placement_slot`: トレイ hide 時に保存された `WINDOWPLACEMENT`。ある場合は
///   `SetWindowPlacement` で DPI 丸めを回避しつつ元のサイズ・位置で復元する。
///
/// activate event は `SingleInstanceGuard::acquire` が `ACTIVATE_EVENT_RAW` OnceLock に
/// 既に登録しているのでここで作成は不要。
pub fn spawn_activation_listener(
    hwnd_raw: isize,
    egui_ctx: eframe::egui::Context,
    placement_slot: crate::tray::PlacementSlot,
) -> Option<ActivationListener> {
    #[cfg(windows)]
    {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        use windows::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects, INFINITE};
        use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_SHOW};

        let Some(&activate_event_raw) = ACTIVATE_EVENT_RAW.get() else {
            crate::logger::log(
                "single_instance: ACTIVATE_EVENT_RAW not set (guard did not create event) — skipping listener",
            );
            return None;
        };
        let stop_event: HANDLE = match unsafe {
            CreateEventW(None, true, false, PCWSTR::null())
        } {
            Ok(h) => h,
            Err(e) => {
                crate::logger::log(format!(
                    "single_instance: CreateEventW(stop) failed: {e:?}"
                ));
                return None;
            }
        };

        struct HandlePair {
            activate: isize,
            stop: isize,
        }
        let handles = HandlePair {
            activate: activate_event_raw,
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
                        crate::logger::log(
                            "single_instance: activate event signaled — restoring window",
                        );
                        // placement_slot に hide 時の WINDOWPLACEMENT があれば先に復元し、
                        // DPI 丸めによるサイズ / 位置のズレを回避する (トレイ Open と同じ挙動)。
                        let hwnd =
                            windows::Win32::Foundation::HWND(hwnd_raw as *mut _);
                        let used_placement = {
                            let mut slot = placement_slot.lock().unwrap();
                            if let Some(p) = slot.take() {
                                crate::tray::restore_window_placement(hwnd_raw, &p);
                                true
                            } else {
                                false
                            }
                        };
                        unsafe {
                            if !used_placement {
                                let _ = ShowWindow(hwnd, SW_SHOW);
                            }
                            let _ = SetForegroundWindow(hwnd);
                        }
                        egui_ctx.request_repaint();
                    } else if r.0 == WAIT_OBJECT_0.0 + 1 {
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
