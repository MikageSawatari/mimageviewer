//! VST3 プラグインの GUI を表示するためのホストウィンドウ管理。
//!
//! ## 設計
//!
//! - **eframe (winit) には触らない**: eframe は内部で winit::EventLoop を握って
//!   いるので、ここから別 winit ウィンドウを作ると衝突する。代わりに **Win32
//!   API を直接呼んで `WS_OVERLAPPEDWINDOW` の独立ウィンドウ**を作る。
//! - **専用スレッド**: VST3 GUI は attached された HWND と同じスレッドで
//!   メッセージを処理する必要がある (= スレッドアフィニティ)。GUI ウィンドウ
//!   作成からメッセージループ、destroy まで 1 本のスレッドで完結させる。
//! - **コマンド**: メイン (eframe) スレッドからは `Cmd` enum をチャネル経由で
//!   投げる (`Show` で HWND を返してもらう、`Close` でウィンドウを閉じる)。
//! - **HWND の受け渡し**: ウィンドウ作成完了時、メインスレッドに `u64` 化した
//!   HWND を返す。これを bridge の `show_gui` コマンドに渡す。
//!
//! ## メッセージループ
//!
//! GetMessage ベースの典型的なループ。`WM_CLOSE` で「閉じてほしい」シグナルを
//! メインスレッドに通知し、そこで bridge に `hide_gui` を送ってから
//! `DestroyWindow` する。直接 destroy しないのは、プラグインから
//! `removed()` を先に呼ぶ必要があるため (順序逆だと crash 報告あり)。

#![cfg(windows)]

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_WINDOW, ClientToScreen, GetMonitorInfoW, HBRUSH, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromRect,
};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
    HWND_NOTOPMOST, IDC_ARROW, IsIconic, LoadCursorW, MSG, PostQuitMessage, RegisterClassExW,
    SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow,
    SetWindowPos, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WM_ACTIVATEAPP, WM_CLOSE,
    WM_DESTROY, WM_LBUTTONDOWN, WM_MOVE, WM_PARENTNOTIFY, WM_SIZE, WNDCLASSEXW, WS_CLIPCHILDREN,
    WS_MAXIMIZEBOX, WS_OVERLAPPEDWINDOW, WS_THICKFRAME,
};
use windows::core::{HSTRING, PCWSTR};

const WINDOW_CLASS: &str = "MivVst3PluginHostWindow";

// WM_ENTERSIZEMOVE = 0x0231, WM_EXITSIZEMOVE = 0x0232 (Win32 SDK 定義値)
// windows-rs から直接定数を引くのが面倒なので生数値を使う (= 値は安定).
const WM_ENTERSIZEMOVE_VAL: u32 = 0x0231;
const WM_EXITSIZEMOVE_VAL: u32 = 0x0232;
const MIN_VISIBLE_WINDOW_AREA: i64 = 64 * 64;

#[derive(Debug)]
pub enum Cmd {
    /// 新規ウィンドウを作って HWND を返す。返り値: (hwnd_u64, close_signal_rx)。
    /// close_signal_rx に値が来たらユーザーが × を押した = メインスレッドが
    /// bridge に `hide_gui` 送信 → このスレッドに `Close` を投げてウィンドウ破棄、
    /// の流れにする。
    /// `resizable=false` ならウィンドウ枠を固定 (= WS_THICKFRAME 抜き) して
    /// ユーザーがドラッグでリサイズしても無効になる (SSL Meter Pro 等の
    /// canResize=false プラグイン用)。
    Show {
        title: String,
        width: u32,
        height: u32,
        resizable: bool,
        visible: bool,
        /// 復元したいウィンドウ位置 (= 前回終了時の `GetWindowRect` の左上座標)。
        /// None ならデフォルト中央配置 (= `CW_USEDEFAULT`) を使う。
        /// 範囲外 (= 旧モニターが取り外された等) は OS 側 `SetWindowPos` で
        /// クランプされるので追加チェックなし (= 大きく変な位置でも無事可視範囲に収まる)。
        initial_pos: Option<(i32, i32)>,
        reply: Sender<ShowReply>,
    },
    /// 既存ウィンドウを閉じる (= プラグイン側 `removed()` 完了後に呼ぶ)。
    Close,
    /// Bridge-owned plugin surface HWND. It is a separate top-level window in
    /// the bridge process, so the host WndProc keeps it aligned during native
    /// move/resize sessions instead of waiting for the main UI polling path.
    SetBridgeContainerHwnd { hwnd: u64 },
    /// GUI スレッド自体を終了する (tester 終了時)。
    Quit,
}

#[derive(Debug)]
pub struct ShowReply {
    pub hwnd_u64: u64,
    /// 実際のクライアント領域サイズ (デバッグ用)。要求値と一致しなければ DPI 計算がズレている。
    pub actual_client_w: u32,
    pub actual_client_h: u32,
    /// AdjustWindowRectExForDpi に渡した DPI 値 (デバッグ用)。
    pub used_dpi: u32,
    /// ユーザーが × を押したときに Sender 側から「閉じてほしい」が届く。
    /// メインスレッドはこれを polling するか recv して、bridge に hide_gui を送る。
    pub close_signal: Arc<Mutex<Option<Receiver<()>>>>,
    /// ユーザーがホストウィンドウをリサイズしたときに新クライアント領域サイズが届く。
    /// メインスレッドはこれを polling して、bridge に notify_host_resize を送る。
    pub resize_signal: Arc<Mutex<Option<Receiver<(u32, u32)>>>>,
    /// ユーザー drag による resize/move session の開始 / 終了通知。
    /// `true` = 開始 (WM_ENTERSIZEMOVE)、`false` = 終了 (WM_EXITSIZEMOVE)。
    /// メインスレッドはこれを polling し、bridge に SetUserResizing で伝える。
    pub resize_session_signal: Arc<Mutex<Option<Receiver<bool>>>>,
    /// mIV process activation changes from the host HWND (WM_ACTIVATEAPP).
    /// The bridge surface uses this to hide while another app is foreground.
    pub app_active_signal: Arc<Mutex<Option<Receiver<bool>>>>,
}

/// 専用スレッドで Win32 メッセージループを回す GUI ホスト。
///
/// **スレッドは detach 方式**で管理する: drop 時に `Cmd::Quit` を送信するだけで
/// `join` は呼ばない。理由はプラグイン (Pro-Q 4 / Insight2 等) の `removed()`
/// が時間を取るケースで、show/hide を高速にトグルするとメインスレッドが
/// 連鎖的にブロックされ「重くなって固まる」ユーザー報告 (2026-04) が発生した。
/// detach なら Cmd::Quit 送信だけで即座にメインに戻り、スレッドはバックグラウンド
/// で自然に exit する (= 通常 100ms 以内)。万一 Quit が届く前にプロセスが終了
/// してもデーモンスレッドとして OS が回収するので問題ない。
pub struct GuiHost {
    cmd_tx: Sender<Cmd>,
    /// JoinHandle は保持するだけ (drop 時に detach されるため)。
    /// `Option::take` は使わないが、`thread` の所有権を保持して revealed lifetime
    /// が混乱しないようにするため `Option` のまま残す。
    _thread: std::thread::JoinHandle<()>,
}

impl GuiHost {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = channel::<Cmd>();
        let thread = std::thread::Builder::new()
            .name("vst3-plugin-gui".into())
            .spawn(move || run_gui_thread(cmd_rx))
            .expect("spawn gui thread");
        Self {
            cmd_tx,
            _thread: thread,
        }
    }

    pub fn show(
        &self,
        title: &str,
        width: u32,
        height: u32,
        resizable: bool,
        initial_pos: Option<(i32, i32)>,
        visible: bool,
    ) -> std::io::Result<ShowReply> {
        let (tx, rx) = channel::<ShowReply>();
        self.cmd_tx
            .send(Cmd::Show {
                title: title.to_string(),
                width,
                height,
                resizable,
                initial_pos,
                visible,
                reply: tx,
            })
            .map_err(|_| std::io::Error::other("gui thread terminated"))?;
        rx.recv()
            .map_err(|_| std::io::Error::other("gui thread did not reply"))
    }

    pub fn close(&self) {
        let _ = self.cmd_tx.send(Cmd::Close);
    }

    pub fn set_bridge_container_hwnd(&self, hwnd: u64) {
        let _ = self.cmd_tx.send(Cmd::SetBridgeContainerHwnd { hwnd });
    }
}

impl Drop for GuiHost {
    fn drop(&mut self) {
        // Quit を送るだけで join しない (= detach)。
        // join するとプラグインの removed() の重さがメインスレッドに伝搬して
        // show/hide 高速トグル時に GUI が重くなる / 固まる (ユーザー報告 2026-04)。
        let _ = self.cmd_tx.send(Cmd::Quit);
        // _thread はそのままドロップ → 内部の OwnedHandle のみ閉じ、
        // OS スレッド自体は Cmd::Quit を受信した時点で自発的に exit する。
    }
}

// ── スレッド実装 ──

struct ThreadState {
    hwnd: Option<HWND>,
    /// ユーザーが × を押した瞬間、メインスレッドに通知するための sender。
    close_tx: Option<Sender<()>>,
    /// ユーザーがホストウィンドウをリサイズしたとき、新クライアントサイズを通知。
    /// メインスレッドはこれを受けて bridge に notify_host_resize を送る。
    resize_tx: Option<Sender<(u32, u32)>>,
    /// ユーザー drag による resize session の開始 / 終了通知 (= true=開始, false=終了)。
    /// メインスレッドはこれを受けて bridge に SetUserResizing を送り、bridge 側の
    /// resizeView feedback を抑止する。Codex P4 対応。
    resize_session_tx: Option<Sender<bool>>,
    /// WM_ACTIVATEAPP relay. This is intentionally process-level, not viewport
    /// focus, so clicking the plugin itself does not hide the plugin surface.
    app_active_tx: Option<Sender<bool>>,
    bridge_container_hwnd: Option<HWND>,
    class_registered: bool,
}

unsafe impl Send for ThreadState {}

fn set_bridge_container_hwnd_for_thread(hwnd_u64: u64) {
    THREAD_STATE.with(|s| {
        let mut state = s.borrow_mut();
        let Some(st) = state.as_mut() else {
            return;
        };
        st.bridge_container_hwnd = (hwnd_u64 != 0).then_some(HWND(hwnd_u64 as *mut _));
        if st.bridge_container_hwnd.is_some()
            && let Some(host_hwnd) = st.hwnd
        {
            sync_bridge_container_to_host(host_hwnd);
        }
    });
}

fn sync_bridge_container_to_host(host_hwnd: HWND) {
    let container_hwnd =
        THREAD_STATE.with(|s| s.borrow().as_ref().and_then(|st| st.bridge_container_hwnd));
    let Some(container_hwnd) = container_hwnd else {
        return;
    };
    unsafe {
        if container_hwnd.0.is_null() || IsIconic(host_hwnd).as_bool() {
            return;
        }
        let mut client = RECT::default();
        if GetClientRect(host_hwnd, &mut client).is_err() {
            return;
        }
        let width = (client.right - client.left).max(1);
        let height = (client.bottom - client.top).max(1);
        let mut origin = POINT { x: 0, y: 0 };
        if !ClientToScreen(host_hwnd, &mut origin).as_bool() {
            return;
        }
        let _ = SetWindowPos(
            container_hwnd,
            None,
            origin.x,
            origin.y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PARENTNOTIFY => {
                // 子ウィンドウ (= プラグインの GUI 本体) でクリックが起きたら
                // ホストウィンドウを **明示的に foreground** にする。ただし右クリックは
                // bridge 側 child-HWND subclass が `SetForegroundWindow + SetFocus(child)`
                // を行うため、ここでは触らない。後追いで host に foreground を戻すと
                // SSL Meter Pro のグラフィカルな右クリックメニューが閉じる可能性がある。
                let event_msg = (wparam.0 & 0xFFFF) as u32;
                if event_msg == WM_LBUTTONDOWN {
                    let _ = SetForegroundWindow(hwnd);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_CLOSE => {
                // ユーザーが × を押した。メインスレッドに通知し、
                // メインスレッドからの Cmd::Close を待つ (= ここでは破棄しない)。
                let tx_opt: Option<Sender<()>> =
                    THREAD_STATE.with(|s| s.borrow().as_ref().and_then(|st| st.close_tx.clone()));
                if let Some(tx) = tx_opt {
                    let _ = tx.send(());
                }
                LRESULT(0)
            }
            WM_SIZE | WM_MOVE => {
                if IsIconic(hwnd).as_bool() {
                    return DefWindowProcW(hwnd, msg, wparam, lparam);
                }
                // ユーザーがホストウィンドウを移動/リサイズした。
                // bridge 側 top-level plugin surface の位置同期にも使うため、
                // WM_MOVE でも現在のクライアント領域サイズを通知する。
                let (w, h) = if msg == WM_SIZE {
                    let lparam_v = lparam.0 as u32;
                    (
                        (lparam_v & 0xFFFF) as u32,
                        ((lparam_v >> 16) & 0xFFFF) as u32,
                    )
                } else {
                    let mut rect = RECT::default();
                    if GetClientRect(hwnd, &mut rect).is_ok() {
                        (
                            (rect.right - rect.left).max(0) as u32,
                            (rect.bottom - rect.top).max(0) as u32,
                        )
                    } else {
                        (0, 0)
                    }
                };
                if w > 0 && h > 0 {
                    sync_bridge_container_to_host(hwnd);
                    let tx_opt: Option<Sender<(u32, u32)>> = THREAD_STATE
                        .with(|s| s.borrow().as_ref().and_then(|st| st.resize_tx.clone()));
                    if let Some(tx) = tx_opt {
                        let _ = tx.send((w, h));
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_ENTERSIZEMOVE_VAL | WM_EXITSIZEMOVE_VAL => {
                // ユーザーが drag による resize/move session を開始 / 終了した。
                // bridge に通知して plugin の resizeView による host SetWindowPos を
                // 抑止する (Codex P4)。
                let active = msg == WM_ENTERSIZEMOVE_VAL;
                let tx_opt: Option<Sender<bool>> = THREAD_STATE.with(|s| {
                    s.borrow()
                        .as_ref()
                        .and_then(|st| st.resize_session_tx.clone())
                });
                if let Some(tx) = tx_opt {
                    let _ = tx.send(active);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_ACTIVATEAPP => {
                let active = wparam.0 != 0;
                if !active {
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_NOTOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
                let tx_opt: Option<Sender<bool>> = THREAD_STATE
                    .with(|s| s.borrow().as_ref().and_then(|st| st.app_active_tx.clone()));
                if let Some(tx) = tx_opt {
                    let _ = tx.send(active);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

thread_local! {
    static THREAD_STATE: std::cell::RefCell<Option<Box<ThreadState>>> =
        std::cell::RefCell::new(None);
}

fn run_gui_thread(cmd_rx: Receiver<Cmd>) {
    // VST3 GUI は STA (Single-Threaded Apartment) を要求する。
    // GUI スレッドで COM を STA 初期化しておかないとプラグイン GUI が
    // 真っ白でハングするケースがある (Pro-Q 4 など)。
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    let co_hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };

    // GUI スレッドの per-thread state を初期化
    THREAD_STATE.with(|s| {
        *s.borrow_mut() = Some(Box::new(ThreadState {
            hwnd: None,
            close_tx: None,
            resize_tx: None,
            resize_session_tx: None,
            app_active_tx: None,
            bridge_container_hwnd: None,
            class_registered: false,
        }));
    });

    loop {
        // ウィンドウ未作成時はコマンドを blocking で待つ。
        let cmd = match cmd_rx.recv() {
            Ok(c) => c,
            Err(_) => break,
        };
        match cmd {
            Cmd::Quit => break,
            Cmd::Close => {
                close_window();
            }
            Cmd::SetBridgeContainerHwnd { hwnd } => {
                set_bridge_container_hwnd_for_thread(hwnd);
            }
            Cmd::Show {
                title,
                width,
                height,
                resizable,
                initial_pos,
                visible,
                reply,
            } => {
                let result = create_window(&title, width, height, resizable, initial_pos, visible);
                match result {
                    Ok((
                        hwnd_u64,
                        close_rx,
                        resize_rx,
                        resize_session_rx,
                        app_active_rx,
                        actual_w,
                        actual_h,
                        used_dpi,
                    )) => {
                        let _ = reply.send(ShowReply {
                            hwnd_u64,
                            actual_client_w: actual_w,
                            actual_client_h: actual_h,
                            used_dpi,
                            close_signal: Arc::new(Mutex::new(Some(close_rx))),
                            resize_signal: Arc::new(Mutex::new(Some(resize_rx))),
                            resize_session_signal: Arc::new(Mutex::new(Some(resize_session_rx))),
                            app_active_signal: Arc::new(Mutex::new(Some(app_active_rx))),
                        });
                        // ウィンドウ作成成功 → メッセージループを回す。
                        // 並行して cmd_rx も polling する必要があるので、
                        // PeekMessage + try_recv のループにする。
                        run_message_loop(&cmd_rx);
                        // ループを抜けた = ウィンドウが destroy された or Quit。
                        // どちらでも outer loop の cmd_rx.recv() に戻る。
                    }
                    Err(e) => {
                        let _ = reply.send(ShowReply {
                            hwnd_u64: 0,
                            actual_client_w: 0,
                            actual_client_h: 0,
                            used_dpi: 0,
                            close_signal: Arc::new(Mutex::new(None)),
                            resize_signal: Arc::new(Mutex::new(None)),
                            resize_session_signal: Arc::new(Mutex::new(None)),
                            app_active_signal: Arc::new(Mutex::new(None)),
                        });
                        eprintln!("create_window failed: {e}");
                    }
                }
            }
        }
    }
    close_window();
    // クラス解除はプロセス終了時に OS が片付けるので明示的には呼ばない
    // (UnregisterClassW の HINSTANCE 渡しが windows-rs のバージョン差で揺れるため)。
    if co_hr.is_ok() {
        unsafe {
            CoUninitialize();
        }
    }
}

fn run_message_loop(cmd_rx: &Receiver<Cmd>) {
    use windows::Win32::UI::WindowsAndMessaging::PM_REMOVE;
    use windows::Win32::UI::WindowsAndMessaging::PeekMessageW;
    unsafe {
        loop {
            // PeekMessage で非ブロッキング
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_QUIT {
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            // メインスレッドからのコマンドを polling
            match cmd_rx.try_recv() {
                Ok(Cmd::Close) => {
                    close_window();
                    return;
                }
                Ok(Cmd::SetBridgeContainerHwnd { hwnd }) => {
                    set_bridge_container_hwnd_for_thread(hwnd);
                }
                Ok(Cmd::Quit) => {
                    close_window();
                    return;
                }
                Ok(Cmd::Show { reply, .. }) => {
                    // すでにウィンドウがあるのに Show 来た。reply に既存 HWND を返さず、
                    // とりあえずエラー扱い (UI 側でガードしているはず)。
                    let _ = reply.send(ShowReply {
                        hwnd_u64: 0,
                        actual_client_w: 0,
                        actual_client_h: 0,
                        used_dpi: 0,
                        close_signal: Arc::new(Mutex::new(None)),
                        resize_signal: Arc::new(Mutex::new(None)),
                        resize_session_signal: Arc::new(Mutex::new(None)),
                        app_active_signal: Arc::new(Mutex::new(None)),
                    });
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
            // 16ms スリープ (60fps 目安、UI 操作の反応性確保)
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
}

fn rect_intersection_area(a: &RECT, b: &RECT) -> i64 {
    let left = a.left.max(b.left);
    let top = a.top.max(b.top);
    let right = a.right.min(b.right);
    let bottom = a.bottom.min(b.bottom);
    let w = (right - left).max(0) as i64;
    let h = (bottom - top).max(0) as i64;
    w * h
}

fn monitor_work_rect_for(rect: &RECT) -> Option<RECT> {
    unsafe {
        let monitor = MonitorFromRect(rect, MONITOR_DEFAULTTONEAREST);
        if monitor.0.is_null() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            Some(info.rcWork)
        } else {
            None
        }
    }
}

fn rect_visible_on_some_work_area(rect: &RECT) -> bool {
    monitor_work_rect_for(rect)
        .map(|work| rect_intersection_area(rect, &work) >= MIN_VISIBLE_WINDOW_AREA)
        .unwrap_or(true)
}

fn clamp_rect_origin_to_nearest_work_area(x: i32, y: i32, width: i32, height: i32) -> (i32, i32) {
    let width = width.max(1);
    let height = height.max(1);
    let rect = RECT {
        left: x,
        top: y,
        right: x.saturating_add(width),
        bottom: y.saturating_add(height),
    };
    let Some(work) = monitor_work_rect_for(&rect) else {
        return (x, y);
    };
    if rect_intersection_area(&rect, &work) >= MIN_VISIBLE_WINDOW_AREA {
        return (x, y);
    }

    let max_x = (work.right - width).max(work.left);
    let max_y = (work.bottom - height).max(work.top);
    (x.clamp(work.left, max_x), y.clamp(work.top, max_y))
}

fn ensure_window_visible_on_monitor(hwnd: HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
    };

    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return;
        }
        if rect_visible_on_some_work_area(&rect) {
            return;
        }
        let width = (rect.right - rect.left).max(1);
        let height = (rect.bottom - rect.top).max(1);
        let (x, y) = clamp_rect_origin_to_nearest_work_area(rect.left, rect.top, width, height);
        crate::logger::log(format!(
            "[VST3 GUI] moved off-screen host window back on-screen: ({}, {}) -> ({}, {}) size={}x{}",
            rect.left, rect.top, x, y, width, height
        ));
        let _ = SetWindowPos(
            hwnd,
            None,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn create_window(
    title: &str,
    width: u32,
    height: u32,
    resizable: bool,
    initial_pos: Option<(i32, i32)>,
    visible: bool,
) -> std::io::Result<(
    u64,
    Receiver<()>,
    Receiver<(u32, u32)>,
    Receiver<bool>,
    Receiver<bool>,
    u32,
    u32,
    u32,
)> {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    unsafe {
        let hinstance = GetModuleHandleW(PCWSTR::null())
            .map_err(|e| std::io::Error::other(format!("GetModuleHandleW: {e}")))?;
        let class_w = HSTRING::from(WINDOW_CLASS);

        // ── ウィンドウクラス登録 ──
        // ⚠️ プロセス全体で 1 回だけ登録する。Win32 のウィンドウクラスは
        //    プロセスグローバルなので、別スレッドで `GuiHost::spawn` を再度呼ぶと
        //    `RegisterClassExW` が `ERROR_CLASS_ALREADY_EXISTS (1410)` で失敗する
        //    (= 旧コードのバグ。スレッドローカル状態で「未登録」と判断していた)。
        //    既存登録はそのまま使えるので「既に登録済み」エラーは無視して継続する。
        let cursor = LoadCursorW(None, IDC_ARROW)
            .map_err(|e| std::io::Error::other(format!("LoadCursorW: {e}")))?;
        // hbrBackground = NULL: 背景の自動消去 (= WM_ERASEBKGND の DefWindowProc
        // による塗りつぶし) を無効化する。プラグインの子ウィンドウがクライアント
        // 領域を完全に占めているので、親側で消去する必要がない。Insight2 等の
        // リサイズ時に「親が一旦システム色で塗る → プラグインが描き直す」のチラつき
        // (= flicker フレーム) を抑える効果がある。
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: Default::default(),
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            hCursor: cursor,
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszClassName: PCWSTR(class_w.as_ptr()),
            ..Default::default()
        };
        // COLOR_WINDOW は将来 hbrBackground に戻す可能性のための保留 (現在未使用)。
        let _ = COLOR_WINDOW;
        if RegisterClassExW(&class) == 0 {
            let err = std::io::Error::last_os_error();
            // ERROR_CLASS_ALREADY_EXISTS = 1410: 別スレッド・別 GuiHost 由来で既に
            // 登録済みのケース。このエラーだけは無視して続行する。
            if err.raw_os_error() != Some(1410) {
                return Err(err);
            }
        }
        THREAD_STATE.with(|s| {
            if let Some(st) = s.borrow_mut().as_mut() {
                st.class_registered = true;
            }
        });

        // クライアント領域 width x height になるよう外枠サイズを調整。
        // Per-Monitor v2 DPI 環境では AdjustWindowRectEx (= 96 DPI 想定) だと
        // フレーム厚を過小評価し、結果クライアント領域が意図より狭くなる
        // (プラグイン GUI の上下が見切れる原因)。AdjustWindowRectExForDpi で
        // 実 DPI を渡して計算する。
        // ⚠️ プラグイン GUI は **常に最前面** で表示する (`WS_EX_TOPMOST`)。
        // 動画フルスクリーン再生中も裏に隠れないようにするため。ユーザーは
        // 動画を見ながら EQ カーブを調整したり LUFS を確認したりするので、
        // 隠れる挙動だと用途を満たせない (= 開いていながら確認できない状態)。
        let mut rect = windows::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        let dpi = GetDpiForSystem();
        // 旧版は `WS_EX_TOPMOST` を常時付けていたが、SSL Meter Pro 等のプラグインで
        // 右クリックメニューが即閉じる問題が発生 (TOPMOST + 非フォアグラウンドでは
        // ポップアップメニューがフォーカスを取れない)。
        // 既定は **TOPMOST 無し**で作成し、フルスクリーン動画再生時のみ動的に
        // SetWindowPos(HWND_TOPMOST) で持ち上げる (= `set_window_topmost` ヘルパー)。
        let ex_style = WINDOW_EX_STYLE(0);
        // ウィンドウスタイル:
        // - WS_OVERLAPPEDWINDOW = OVERLAPPED|CAPTION|SYSMENU|THICKFRAME|MIN/MAXBOX
        // - resizable=false の場合は WS_THICKFRAME (= リサイズ枠) を抜く。プラグインが
        //   canResize() で false を返した (= SSL Meter Pro 等の固定サイズ表示) なら
        //   ユーザーがドラッグしても外側ウィンドウが大きくならず、紛らわしい挙動を防ぐ。
        // - WS_CLIPCHILDREN を追加: プラグイン子ウィンドウの領域は親が描画しない
        //   設定。Insight2 のリサイズ時に親→子の上書きで起きる flicker フレーム
        //   をさらに抑える効果がある。
        let mut win_style = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;
        if !resizable {
            // WS_THICKFRAME (= ドラッグでサイズ変更) を抜く。
            // **WS_MAXIMIZEBOX も抜く** (= ユーザー報告 2026-05): タイトルバーの
            // ダブルクリックは WS_MAXIMIZEBOX が立っていれば最大化を発動するため、
            // SSL Meter Pro 等の固定サイズプラグインでも外枠だけが拡大して
            // 中身がそのサイズで止まる紛らわしい挙動になる。MAXIMIZEBOX を抜くと
            // ダブルクリックも無効化される。
            win_style &= !(WS_THICKFRAME | WS_MAXIMIZEBOX);
        }
        let _ = AdjustWindowRectExForDpi(&mut rect, win_style, false, ex_style, dpi);
        let outer_w = rect.right - rect.left;
        let outer_h = rect.bottom - rect.top;

        // 初期位置: ユーザーが前回終了時に動かしたウィンドウ位置を復元する
        // (= 2026-05 ユーザー要望)。Option::None の場合は CW_USEDEFAULT (= OS 既定の
        // カスケード配置) を使う。指定座標がモニター外でも Win32 が後段で
        // SetWindowPos してくれるので追加クランプは不要。
        let (init_x, init_y) = match initial_pos {
            Some((x, y)) => clamp_rect_origin_to_nearest_work_area(x, y, outer_w, outer_h),
            None => (CW_USEDEFAULT, CW_USEDEFAULT),
        };
        let title_w = HSTRING::from(title);
        let hwnd = CreateWindowExW(
            ex_style,
            PCWSTR(class_w.as_ptr()),
            PCWSTR(title_w.as_ptr()),
            win_style,
            init_x,
            init_y,
            outer_w,
            outer_h,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
        .map_err(|e| std::io::Error::other(format!("CreateWindowExW: {e}")))?;

        crate::dwm_transitions::disable_transitions_for_window(hwnd);
        if visible {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }

        // 実 client rect を確認 (デバッグ用)
        let mut actual = windows::Win32::Foundation::RECT::default();
        let _ = GetClientRect(hwnd, &mut actual);
        let actual_w = (actual.right - actual.left) as u32;
        let actual_h = (actual.bottom - actual.top) as u32;

        let (close_tx, close_rx) = channel::<()>();
        let (resize_tx, resize_rx) = channel::<(u32, u32)>();
        let (resize_session_tx, resize_session_rx) = channel::<bool>();
        let (app_active_tx, app_active_rx) = channel::<bool>();
        THREAD_STATE.with(|s| {
            if let Some(st) = s.borrow_mut().as_mut() {
                st.hwnd = Some(hwnd);
                st.close_tx = Some(close_tx);
                st.resize_tx = Some(resize_tx);
                st.resize_session_tx = Some(resize_session_tx);
                st.app_active_tx = Some(app_active_tx);
            }
        });
        Ok((
            hwnd.0 as u64,
            close_rx,
            resize_rx,
            resize_session_rx,
            app_active_rx,
            actual_w,
            actual_h,
            dpi,
        ))
    }
}

fn close_window() {
    THREAD_STATE.with(|s| {
        let mut state = s.borrow_mut();
        if let Some(st) = state.as_mut() {
            if let Some(hwnd) = st.hwnd.take() {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            st.close_tx = None;
            st.resize_tx = None;
            st.resize_session_tx = None;
            st.app_active_tx = None;
            st.bridge_container_hwnd = None;
        }
    });
}

/// 指定 HWND の現在のデスクトップ位置 + 外枠サイズを返す。
/// 戻り値: (x, y, width, height) (= screen coordinate、px)。HWND が無効なら None。
/// プラグイン GUI ウィンドウの位置永続化用 (= 2026-05 ユーザー要望)。
pub fn get_window_rect(hwnd_u64: u64) -> Option<(i32, i32, u32, u32)> {
    use windows::Win32::Foundation::RECT as Rect;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
    if hwnd_u64 == 0 {
        return None;
    }
    let mut rect = Rect::default();
    unsafe {
        let hwnd = HWND(hwnd_u64 as *mut _);
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return None;
        }
    }
    let w = (rect.right - rect.left).max(0) as u32;
    let h = (rect.bottom - rect.top).max(0) as u32;
    if w == 0 || h == 0 {
        return None;
    }
    if !rect_visible_on_some_work_area(&rect) {
        crate::logger::log(format!(
            "[VST3 GUI] skipped saving off-screen host window rect: ({}, {}) size={}x{}",
            rect.left, rect.top, w, h
        ));
        return None;
    }
    Some((rect.left, rect.top, w, h))
}

/// プラグイン GUI ホストウィンドウを最前面 (= topmost) に固定 + 表示する。
/// V キートグル時に既に開いているウィンドウを再度前に出すためのヘルパー。
///
/// `WS_EX_TOPMOST` 属性は CreateWindowExW で既に設定済みだが、
/// 一部の環境 (= Always On Top を解除する別アプリの介入等) で外れることがあるため、
/// 念のため SetWindowPos(HWND_TOPMOST) も毎回呼ぶ。
pub fn bring_to_front(hwnd_u64: u64) {
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_TOPMOST, SW_SHOW, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, ShowWindow,
    };
    if hwnd_u64 == 0 {
        return;
    }
    unsafe {
        let hwnd = HWND(hwnd_u64 as *mut _);
        crate::dwm_transitions::disable_transitions_for_window(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE,
        );
    }
}

/// 指定 HWND 群の現在の z-order を top-to-bottom 順 (= 最前面から背面) で返す。
///
/// `EnumWindows` でデスクトップの top-level window を前面順に走査し、
/// `targets` に含まれる HWND だけを拾って順序付きで返す。
/// `set_all_guis_topmost` で TOPMOST 切替前に snapshot し、切替後に bottom-to-top
/// で再適用するために使う (= 元の前後関係を保つ、Codex P1 対応)。
pub fn snapshot_z_order(targets: &[u64]) -> Vec<u64> {
    use windows::Win32::Foundation::{GetLastError, HWND as Hwnd2};
    use windows::Win32::UI::WindowsAndMessaging::{GW_HWNDNEXT, GetTopWindow, GetWindow};

    if targets.is_empty() {
        return Vec::new();
    }
    let target_set: std::collections::HashSet<u64> = targets.iter().copied().collect();
    let mut found: Vec<u64> = Vec::with_capacity(targets.len());

    // GetTopWindow(NULL) → desktop の最前面 top-level → GetWindow(GW_HWNDNEXT) で
    // z-order を top→bottom 順にたどる。target_set に含まれるものだけ拾う。
    unsafe {
        let mut h = GetTopWindow(None).unwrap_or(Hwnd2(std::ptr::null_mut()));
        let _ = GetLastError(); // 未使用警告抑制
        let mut safety_iter = 0u32;
        while !h.0.is_null() && safety_iter < 65536 {
            safety_iter += 1;
            let h_u = h.0 as u64;
            if target_set.contains(&h_u) {
                found.push(h_u);
                if found.len() == targets.len() {
                    break;
                }
            }
            h = match GetWindow(h, GW_HWNDNEXT) {
                Ok(next) => next,
                Err(_) => break,
            };
        }
    }
    // 取りこぼした HWND は targets の元順序で末尾に追加 (= 保険)。
    for h in targets {
        if !found.contains(h) {
            found.push(*h);
        }
    }
    found
}

/// 既存ウィンドウを TOPMOST にする / 解除する。
///
/// プラグイン GUI は通常時は regular な top-level window だが、mIV のフルスクリーン
/// 動画再生中は **動画ビューポート (= フルスクリーンサイズの普通のウィンドウ) の
/// 後ろに隠れる** ため、フルスクリーン中だけ動的に TOPMOST を付ける。
/// この方式なら通常時は `WS_EX_TOPMOST` 無しなので SSL Meter Pro 等の右クリック
/// メニューも問題なく動作する (TOPMOST + 非フォアグラウンドだとポップアップが
/// フォーカスを取れず即閉じる挙動を回避)。
pub fn set_window_topmost(hwnd_u64: u64, topmost: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    };
    if hwnd_u64 == 0 {
        return;
    }
    unsafe {
        let hwnd = HWND(hwnd_u64 as *mut _);
        let z = if topmost {
            HWND_TOPMOST
        } else {
            HWND_NOTOPMOST
        };
        let _ = SetWindowPos(
            hwnd,
            Some(z),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// 既存ウィンドウの可視状態をトグルする (= ShowWindow(SW_SHOW/SW_HIDE))。
///
/// **永続 GuiHost** デザインの中核ヘルパー。プラグイン GUI を
/// `show/hide_slot_gui` で頻繁にトグルする際、毎回 createView/removed を
/// 呼ぶと plugin の重い初期化が走り「重くなって固まる」「DAW より遅い」
/// 報告 (2026-04) の根本原因になっていた。窓は破棄せず可視状態のみ切替える。
pub fn set_window_visible(hwnd_u64: u64, visible: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNA, ShowWindow};
    if hwnd_u64 == 0 {
        return;
    }
    unsafe {
        let hwnd = HWND(hwnd_u64 as *mut _);
        crate::dwm_transitions::disable_transitions_for_window(hwnd);
        if visible {
            ensure_window_visible_on_monitor(hwnd);
        }
        let _ = ShowWindow(hwnd, if visible { SW_SHOWNA } else { SW_HIDE });
    }
}

/// `IPlugView::onSize` のような細かいプラグイン側通知は今回は扱わない (= 起動時に一回だけ)。
pub fn resize_window_client(hwnd_u64: u64, width: u32, height: u32) {
    if hwnd_u64 == 0 {
        return;
    }
    unsafe {
        let mut rect = windows::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        let dpi = GetDpiForSystem();
        let _ = AdjustWindowRectExForDpi(
            &mut rect,
            WS_OVERLAPPEDWINDOW,
            false,
            WINDOW_EX_STYLE(0),
            dpi,
        );
        let outer_w = rect.right - rect.left;
        let outer_h = rect.bottom - rect.top;
        let hwnd = HWND(hwnd_u64 as *mut _);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            outer_w,
            outer_h,
            SWP_NOMOVE | SWP_NOZORDER,
        );
    }
}

/// Show the given windows and restore their front-to-back order in one Win32 batch.
///
/// `ordered_top_to_bottom` must be the desired z-order snapshot. We submit it
/// bottom-to-top because each TOPMOST / NOTOPMOST insertion promotes the current
/// HWND within its group; the last HWND submitted becomes the front-most one.
pub fn show_windows_in_z_order(ordered_top_to_bottom: &[u64], topmost: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{
        BeginDeferWindowPos, DeferWindowPos, EndDeferWindowPos, HWND_NOTOPMOST, HWND_TOPMOST,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    };

    let hwnds: Vec<u64> = ordered_top_to_bottom
        .iter()
        .copied()
        .filter(|h| *h != 0)
        .collect();
    if hwnds.is_empty() {
        return;
    }

    unsafe {
        let mut batch = match BeginDeferWindowPos(hwnds.len() as i32) {
            Ok(batch) => batch,
            Err(e) => {
                crate::logger::log(format!(
                    "vst3 gui BeginDeferWindowPos failed: {e}; falling back"
                ));
                for hwnd in hwnds.iter().rev() {
                    set_window_visible(*hwnd, true);
                    set_window_topmost(*hwnd, topmost);
                }
                return;
            }
        };

        let insert_after = if topmost {
            HWND_TOPMOST
        } else {
            HWND_NOTOPMOST
        };
        for hwnd_u64 in hwnds.iter().rev() {
            let hwnd = HWND(*hwnd_u64 as *mut _);
            crate::dwm_transitions::disable_transitions_for_window(hwnd);
            match DeferWindowPos(
                batch,
                hwnd,
                Some(insert_after),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            ) {
                Ok(next_batch) => batch = next_batch,
                Err(e) => {
                    crate::logger::log(format!(
                        "vst3 gui DeferWindowPos failed: {e}; falling back"
                    ));
                    for hwnd in hwnds.iter().rev() {
                        set_window_visible(*hwnd, true);
                        set_window_topmost(*hwnd, topmost);
                    }
                    return;
                }
            }
        }

        if let Err(e) = EndDeferWindowPos(batch) {
            crate::logger::log(format!(
                "vst3 gui EndDeferWindowPos failed: {e}; falling back"
            ));
            for hwnd in hwnds.iter().rev() {
                set_window_visible(*hwnd, true);
                set_window_topmost(*hwnd, topmost);
            }
        }
    }
}
