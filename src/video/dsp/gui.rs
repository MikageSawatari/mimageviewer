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
    IDC_ARROW, LoadCursorW, MSG, PostQuitMessage, RegisterClassExW, SetForegroundWindow,
    SetWindowPos, ShowWindow, SW_SHOW, SWP_NOMOVE, SWP_NOZORDER, TranslateMessage, WINDOW_EX_STYLE,
    WM_CLOSE, WM_DESTROY, WM_LBUTTONDOWN, WM_PARENTNOTIFY, WM_RBUTTONDOWN, WM_SIZE, WNDCLASSEXW,
    WS_CLIPCHILDREN, WS_OVERLAPPEDWINDOW, WS_THICKFRAME,
};
use windows::core::{HSTRING, PCWSTR};

const WINDOW_CLASS: &str = "MivVst3PluginHostWindow";

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
    /// ユーザーがホストウィンドウをリサイズしたときに新クライアント領域サイズが届く。
    /// メインスレッドはこれを polling して、bridge に notify_host_resize を送る。
    pub resize_signal: Arc<Mutex<Option<Receiver<(u32, u32)>>>>,
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
    ) -> std::io::Result<ShowReply> {
        let (tx, rx) = channel::<ShowReply>();
        self.cmd_tx
            .send(Cmd::Show {
                title: title.to_string(),
                width,
                height,
                resizable,
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
            WM_PARENTNOTIFY => {
                // 子ウィンドウ (= プラグインの GUI 本体) でクリックが起きたら
                // ホストウィンドウを **明示的に foreground** にする。
                // これがないと SSL Meter Pro 等の `TrackPopupMenu` で開く
                // 右クリックメニューがフォーカスを取れず即閉じる事象が出る
                // (= owner top-level window が foreground でないと popup は不安定)。
                let event_msg = (wparam.0 & 0xFFFF) as u32;
                if event_msg == WM_LBUTTONDOWN || event_msg == WM_RBUTTONDOWN {
                    let _ = SetForegroundWindow(hwnd);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
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
            WM_SIZE => {
                // ユーザーがホストウィンドウをドラッグでリサイズした。
                // 新しいクライアント領域サイズを取得してメインスレッドに通知。
                let lparam_v = lparam.0 as u32;
                let w = (lparam_v & 0xFFFF) as u32;
                let h = ((lparam_v >> 16) & 0xFFFF) as u32;
                if w > 0 && h > 0 {
                    let tx_opt: Option<Sender<(u32, u32)>> = THREAD_STATE.with(|s| {
                        s.borrow()
                            .as_ref()
                            .and_then(|st| st.resize_tx.clone())
                    });
                    if let Some(tx) = tx_opt {
                        let _ = tx.send((w, h));
                    }
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
            resize_tx: None,
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
                resizable,
                reply,
            } => {
                let result = create_window(&title, width, height, resizable);
                match result {
                    Ok((hwnd_u64, close_rx, resize_rx, actual_w, actual_h, used_dpi)) => {
                        let _ = reply.send(ShowReply {
                            hwnd_u64,
                            actual_client_w: actual_w,
                            actual_client_h: actual_h,
                            used_dpi,
                            close_signal: Arc::new(Mutex::new(Some(close_rx))),
                            resize_signal: Arc::new(Mutex::new(Some(resize_rx))),
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
                        resize_signal: Arc::new(Mutex::new(None)),
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
    resizable: bool,
) -> std::io::Result<(u64, Receiver<()>, Receiver<(u32, u32)>, u32, u32, u32)> {
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
            win_style &= !WS_THICKFRAME;
        }
        let _ = AdjustWindowRectExForDpi(
            &mut rect,
            win_style,
            false,
            ex_style,
            dpi,
        );
        let outer_w = rect.right - rect.left;
        let outer_h = rect.bottom - rect.top;

        let title_w = HSTRING::from(title);
        let hwnd = CreateWindowExW(
            ex_style,
            PCWSTR(class_w.as_ptr()),
            PCWSTR(title_w.as_ptr()),
            win_style,
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

        let (close_tx, close_rx) = channel::<()>();
        let (resize_tx, resize_rx) = channel::<(u32, u32)>();
        THREAD_STATE.with(|s| {
            if let Some(st) = s.borrow_mut().as_mut() {
                st.hwnd = Some(hwnd);
                st.close_tx = Some(close_tx);
                st.resize_tx = Some(resize_tx);
            }
        });
        Ok((hwnd.0 as u64, close_rx, resize_rx, actual_w, actual_h, dpi))
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
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
    }
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
        let z = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
        let _ = SetWindowPos(
            hwnd,
            Some(z),
            0, 0, 0, 0,
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
        let _ = SetWindowPos(hwnd, None, 0, 0, outer_w, outer_h, SWP_NOMOVE | SWP_NOZORDER);
    }
}
