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

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, HBRUSH};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
    IDC_ARROW, LoadCursorW, MSG, PostQuitMessage, RegisterClassExW, SetWindowPos, ShowWindow,
    SW_SHOW, SWP_NOMOVE, SWP_NOZORDER, TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY,
    WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{HSTRING, PCWSTR};

const WINDOW_CLASS: &str = "MivVst3HostTesterPluginWindow";

#[derive(Debug)]
pub enum Cmd {
    /// 新規ウィンドウを作って HWND を返す。返り値: (hwnd_u64, close_signal_rx)。
    /// close_signal_rx に値が来たらユーザーが × を押した = メインスレッドが
    /// bridge に `hide_gui` 送信 → このスレッドに `Close` を投げてウィンドウ破棄、
    /// の流れにする。
    Show {
        title: String,
        width: u32,
        height: u32,
        reply: Sender<ShowReply>,
    },
    /// 既存ウィンドウを閉じる (= プラグイン側 `removed()` 完了後に呼ぶ)。
    Close,
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
}

/// 専用スレッドで Win32 メッセージループを回す GUI ホスト。
pub struct GuiHost {
    cmd_tx: Sender<Cmd>,
    thread: Option<std::thread::JoinHandle<()>>,
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
            thread: Some(thread),
        }
    }

    pub fn show(&self, title: &str, width: u32, height: u32) -> std::io::Result<ShowReply> {
        let (tx, rx) = channel::<ShowReply>();
        self.cmd_tx
            .send(Cmd::Show {
                title: title.to_string(),
                width,
                height,
                reply: tx,
            })
            .map_err(|_| std::io::Error::other("gui thread terminated"))?;
        rx.recv()
            .map_err(|_| std::io::Error::other("gui thread did not reply"))
    }

    pub fn close(&self) {
        let _ = self.cmd_tx.send(Cmd::Close);
    }
}

impl Drop for GuiHost {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Cmd::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ── スレッド実装 ──

struct ThreadState {
    hwnd: Option<HWND>,
    /// ユーザーが × を押した瞬間、メインスレッドに通知するための sender。
    close_tx: Option<Sender<()>>,
    class_registered: bool,
}

unsafe impl Send for ThreadState {}

extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_CLOSE => {
                // ユーザーが × を押した。メインスレッドに通知し、
                // メインスレッドからの Cmd::Close を待つ (= ここでは破棄しない)。
                let tx_opt: Option<Sender<()>> = THREAD_STATE.with(|s| {
                    s.borrow()
                        .as_ref()
                        .and_then(|st| st.close_tx.clone())
                });
                if let Some(tx) = tx_opt {
                    let _ = tx.send(());
                }
                LRESULT(0)
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
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    let co_hr = unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)
    };

    // GUI スレッドの per-thread state を初期化
    THREAD_STATE.with(|s| {
        *s.borrow_mut() = Some(Box::new(ThreadState {
            hwnd: None,
            close_tx: None,
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
            Cmd::Show {
                title,
                width,
                height,
                reply,
            } => {
                let result = create_window(&title, width, height);
                match result {
                    Ok((hwnd_u64, close_rx, actual_w, actual_h, used_dpi)) => {
                        let _ = reply.send(ShowReply {
                            hwnd_u64,
                            actual_client_w: actual_w,
                            actual_client_h: actual_h,
                            used_dpi,
                            close_signal: Arc::new(Mutex::new(Some(close_rx))),
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
    use windows::Win32::UI::WindowsAndMessaging::PeekMessageW;
    use windows::Win32::UI::WindowsAndMessaging::PM_REMOVE;
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

fn create_window(
    title: &str,
    width: u32,
    height: u32,
) -> std::io::Result<(u64, Receiver<()>, u32, u32, u32)> {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    unsafe {
        let hinstance = GetModuleHandleW(PCWSTR::null())
            .map_err(|e| std::io::Error::other(format!("GetModuleHandleW: {e}")))?;
        let class_w = HSTRING::from(WINDOW_CLASS);

        // 1 度だけクラス登録 (スレッドローカル状態に記録)
        let need_register = THREAD_STATE.with(|s| {
            !s.borrow().as_ref().map(|st| st.class_registered).unwrap_or(true)
        });
        if need_register {
            let cursor = LoadCursorW(None, IDC_ARROW)
                .map_err(|e| std::io::Error::other(format!("LoadCursorW: {e}")))?;
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: Default::default(),
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance.into(),
                hCursor: cursor,
                hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as *mut _),
                lpszClassName: PCWSTR(class_w.as_ptr()),
                ..Default::default()
            };
            if RegisterClassExW(&class) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            THREAD_STATE.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    st.class_registered = true;
                }
            });
        }

        // クライアント領域 width x height になるよう外枠サイズを調整。
        // Per-Monitor v2 DPI 環境では AdjustWindowRectEx (= 96 DPI 想定) だと
        // フレーム厚を過小評価し、結果クライアント領域が意図より狭くなる
        // (プラグイン GUI の上下が見切れる原因)。AdjustWindowRectExForDpi で
        // 実 DPI を渡して計算する。
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

        let title_w = HSTRING::from(title);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_w.as_ptr()),
            PCWSTR(title_w.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            outer_w,
            outer_h,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
        .map_err(|e| std::io::Error::other(format!("CreateWindowExW: {e}")))?;

        let _ = ShowWindow(hwnd, SW_SHOW);

        // 実 client rect を確認 (デバッグ用)
        let mut actual = windows::Win32::Foundation::RECT::default();
        let _ = GetClientRect(hwnd, &mut actual);
        let actual_w = (actual.right - actual.left) as u32;
        let actual_h = (actual.bottom - actual.top) as u32;

        let (tx, rx) = channel::<()>();
        THREAD_STATE.with(|s| {
            if let Some(st) = s.borrow_mut().as_mut() {
                st.hwnd = Some(hwnd);
                st.close_tx = Some(tx);
            }
        });
        Ok((hwnd.0 as u64, rx, actual_w, actual_h, dpi))
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
        }
    });
}

/// プラグインの推奨サイズに合わせてホストウィンドウのクライアント領域をリサイズする。
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
        let _ = SetWindowPos(hwnd, None, 0, 0, outer_w, outer_h, SWP_NOMOVE | SWP_NOZORDER);
    }
}
