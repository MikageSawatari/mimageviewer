//! タスクトレイ常駐サポート (v0.9)。
//!
//! 目的: ウィンドウ [×] ボタンでプロセス終了する代わりにタスクトレイに収め、
//! notify-rs によるファイル監視を継続することで次回起動時の再スキャン負荷を避ける。
//!
//! ## 設計上の要点 (eframe 0.33 + tray-icon 0.20 の制約)
//!
//! eframe/winit は **ウィンドウが非表示の間 `App::update` を呼ばない**。
//! `ViewportCommand::Visible(false)` でも Win32 `SW_HIDE` でも同じ。したがって
//! 「トレイメニューをクリック → App::update で処理」という素直な流れは**成立しない**。
//!
//! 解決策: トレイスレッド自身が Win32 を直接叩く。
//!
//! - **開く (左クリック / メニュー)**: `ShowWindow(hwnd, SW_SHOW)` + `SetForegroundWindow`
//!   をトレイスレッドから直接呼ぶ。ウィンドウが可視になれば winit/eframe は `update` を
//!   再開する。同時に App 側の事後処理 (throttle/pause 解除、ログ、ツールチップ) のために
//!   イベントも送信する。
//! - **一時停止**: `Arc<ActivityGate>` と `Arc<GlobalIoSemaphore>` をトレイスレッドに
//!   共有してあり、クリック即座に状態反転する。App 側は設定保存 + ツールチップ更新用に
//!   イベントも受信する (遅延反映でも OK)。
//! - **終了**: `quit_flag` を立てた後 `ShowWindow(SW_SHOW)` (update 再開保証) +
//!   `PostMessageW(hwnd, WM_CLOSE)`。winit が `CloseRequested` を発火 → `maybe_intercept_close`
//!   が `quit_flag=true` を見て interception をスキップ → 通常の close flow で `on_exit` 実行。
//!
//! ## スレッドモデル
//!
//! - `mimv-tray` 専用スレッドで TrayIcon の隠し HWND を作成
//! - `PeekMessageW` + `DispatchMessageW` で Win32 メッセージをポンプ
//! - `MenuEvent::set_event_handler` / `TrayIconEvent::set_event_handler` でイベント受信
//! - コマンドは `cmd_rx` 経由で受信 (チェック状態更新、ツールチップ更新、shutdown)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender};

use crate::activity_gate::ActivityGate;
use crate::io_semaphore::GlobalIoSemaphore;

/// Win32 `WINDOWPLACEMENT` の保存スナップショット。hide 直前にキャプチャし、
/// restore 時に `SetWindowPlacement` に戻す。マルチモニタ DPI 環境で eframe/winit が
/// ウィンドウサイズを丸める問題を回避するため (docs/dpi-multimonitor-issue.md)。
///
/// トレイスレッドと UI スレッド間で共有する必要があるので Send+Sync 前提の [u8; N] 表現。
/// 非 Windows ではフィールドは空で機能しない。
#[derive(Clone, Copy, Debug)]
pub struct SavedWindowPlacement {
    #[cfg(windows)]
    bytes: [u8; std::mem::size_of::<windows::Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT>()],
}

/// 共有プレースメントスロット。hide 側がセット、show 側が take する。
pub type PlacementSlot = Arc<Mutex<Option<SavedWindowPlacement>>>;

pub fn new_placement_slot() -> PlacementSlot {
    Arc::new(Mutex::new(None))
}

#[cfg(windows)]
pub fn capture_window_placement(hwnd_raw: isize) -> Option<SavedWindowPlacement> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowPlacement, WINDOWPLACEMENT};
    unsafe {
        let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
        wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(HWND(hwnd_raw as *mut _), &mut wp).is_err() {
            return None;
        }
        let bytes_src = std::slice::from_raw_parts(
            (&wp as *const WINDOWPLACEMENT) as *const u8,
            std::mem::size_of::<WINDOWPLACEMENT>(),
        );
        let mut out = [0u8; std::mem::size_of::<WINDOWPLACEMENT>()];
        out.copy_from_slice(bytes_src);
        Some(SavedWindowPlacement { bytes: out })
    }
}

#[cfg(windows)]
fn restore_window_placement(hwnd_raw: isize, saved: &SavedWindowPlacement) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPlacement, WINDOWPLACEMENT};
    unsafe {
        let wp = *(saved.bytes.as_ptr() as *const WINDOWPLACEMENT);
        if let Err(e) = SetWindowPlacement(HWND(hwnd_raw as *mut _), &wp) {
            crate::logger::log(format!("tray: SetWindowPlacement failed: {e:?}"));
        }
    }
}

/// UI スレッドが受け取るトレイイベント (状態同期用、重要な副作用は既にトレイスレッドで完了済)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    /// 左クリック or メニュー「開く」。ShowWindow はトレイスレッドが既に実行済み。
    /// App は window_visible フラグ更新 + throttle 解除ログ + ツールチップ更新のみ。
    OpenRequested,
    /// 一時停止トグル。ActivityGate はトレイスレッドが既に反転済み。
    /// App は設定保存 + ツールチップ更新のみ。
    TogglePauseRequested,
    /// メニュー「終了」。quit_flag は既に true、WM_CLOSE も post 済み。
    /// App は close 経路でそのまま抜けるだけ。
    QuitRequested,
}

/// UI スレッドからトレイスレッドへの制御コマンド。
#[derive(Clone, Debug)]
enum TrayCommand {
    /// 「一時停止」メニュー項目のチェック表示を更新 (App 側から状態同期したいとき用)
    SetPausedCheck(bool),
    /// ツールチップを更新 ("mImageViewer — インデックス一時停止中" 等)
    SetTooltip(String),
    /// スレッド終了
    Shutdown,
}

/// タスクトレイ制御ハンドル。App から保持し、Drop でトレイスレッドを join する。
pub struct TrayController {
    event_rx: Receiver<TrayEvent>,
    cmd_tx: Sender<TrayCommand>,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// トレイ「終了」が押されたら true。`App::maybe_intercept_close` が読み、
    /// true なら close インターセプトをスキップして通常終了経路に抜ける。
    quit_flag: Arc<AtomicBool>,
}

impl TrayController {
    /// トレイスレッドを起動してコントローラを返す。Windows 以外では `None`。
    ///
    /// - `icon_rgba` / `icon_w` / `icon_h`: 埋め込みアイコンの RGBA ピクセル列
    /// - `egui_ctx`: UI スレッドを起こすための Context (request_repaint 用)
    /// - `main_hwnd`: メインウィンドウの HWND (raw isize)。
    ///   `ShowWindow` / `SetForegroundWindow` / `PostMessageW(WM_CLOSE)` で直接操作する。
    /// - `activity_gate`: 一時停止トグルを即座に適用するため共有
    /// - `io_sem`: 任意。トレイ常駐中の I/O throttle 制御用
    #[cfg(windows)]
    pub fn start(
        icon_rgba: Vec<u8>,
        icon_w: u32,
        icon_h: u32,
        egui_ctx: eframe::egui::Context,
        main_hwnd: isize,
        activity_gate: Arc<ActivityGate>,
        io_sem: Option<Arc<GlobalIoSemaphore>>,
        placement_slot: PlacementSlot,
    ) -> Option<Self> {
        let (event_tx, event_rx) = bounded::<TrayEvent>(16);
        let (cmd_tx, cmd_rx) = bounded::<TrayCommand>(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_th = Arc::clone(&shutdown);
        let quit_flag = Arc::new(AtomicBool::new(false));
        let quit_flag_th = Arc::clone(&quit_flag);

        let thread = std::thread::Builder::new()
            .name("mimv-tray".into())
            .spawn(move || {
                run_tray_thread(
                    icon_rgba,
                    icon_w,
                    icon_h,
                    event_tx,
                    cmd_rx,
                    shutdown_th,
                    egui_ctx,
                    main_hwnd,
                    activity_gate,
                    io_sem,
                    quit_flag_th,
                    placement_slot,
                );
            })
            .ok()?;

        Some(Self {
            event_rx,
            cmd_tx,
            shutdown,
            thread: Some(thread),
            quit_flag,
        })
    }

    #[cfg(not(windows))]
    pub fn start(
        _: Vec<u8>,
        _: u32,
        _: u32,
        _: eframe::egui::Context,
        _: isize,
        _: Arc<ActivityGate>,
        _: Option<Arc<GlobalIoSemaphore>>,
        _: PlacementSlot,
    ) -> Option<Self> {
        None
    }

    /// ノンブロッキングでイベントを受信。無ければ None。毎フレーム呼ぶ想定。
    pub fn try_recv(&self) -> Option<TrayEvent> {
        self.event_rx.try_recv().ok()
    }

    /// 「一時停止」メニュー項目のチェック状態を更新。
    /// App 側から設定ページなど経由で paused が変化したときの同期用。
    pub fn set_paused_check(&self, paused: bool) {
        let _ = self.cmd_tx.send(TrayCommand::SetPausedCheck(paused));
    }

    /// ツールチップを更新。「mImageViewer — インデックス一時停止中」等の表示に使う。
    pub fn set_tooltip(&self, text: String) {
        let _ = self.cmd_tx.send(TrayCommand::SetTooltip(text));
    }

    /// トレイ「終了」メニューが押されたか。`App::maybe_intercept_close` が判定用に読む。
    pub fn is_quit_requested(&self) -> bool {
        self.quit_flag.load(Ordering::SeqCst)
    }
}

/// 埋め込みアイコン (`assets/icon.png`) を RGBA ピクセル列にデコードして返す。
/// 失敗時 (画像ライブラリがデコードできない等) は `None`。
pub fn load_embedded_icon_rgba() -> Option<(Vec<u8>, u32, u32)> {
    let bytes = include_bytes!("../assets/icon.png");
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some((img.into_raw(), w, h))
}

impl Drop for TrayController {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = self.cmd_tx.send(TrayCommand::Shutdown);
        if let Some(th) = self.thread.take() {
            let _ = th.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Windows 実装
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn run_tray_thread(
    icon_rgba: Vec<u8>,
    icon_w: u32,
    icon_h: u32,
    event_tx: Sender<TrayEvent>,
    cmd_rx: Receiver<TrayCommand>,
    shutdown: Arc<AtomicBool>,
    egui_ctx: eframe::egui::Context,
    main_hwnd: isize,
    activity_gate: Arc<ActivityGate>,
    io_sem: Option<Arc<GlobalIoSemaphore>>,
    quit_flag: Arc<AtomicBool>,
    placement_slot: PlacementSlot,
) {
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetWindowThreadProcessId, PeekMessageW, PostThreadMessageW,
        SetForegroundWindow, ShowWindow, TranslateMessage, MSG, PM_REMOVE, SW_SHOW, WM_QUIT,
    };

    // HWND (`*mut c_void`) は Send/Sync ではないので、クロージャでキャプチャするときは
    // isize のままキャプチャして各呼び出し点で HWND を再構成する。
    fn make_hwnd(raw: isize) -> HWND {
        HWND(raw as *mut _)
    }

    let icon = match Icon::from_rgba(icon_rgba, icon_w, icon_h) {
        Ok(i) => i,
        Err(e) => {
            crate::logger::log(format!("tray: icon load failed: {e}"));
            return;
        }
    };

    // メニュー構造: 開く / ✓ 常駐時のスキャンを一時停止 / ── / 終了
    let menu = Menu::new();
    let item_open = MenuItem::new("開く", true, None);
    let item_pause = CheckMenuItem::new("常駐時のスキャンを一時停止", true, false, None);
    let item_sep = PredefinedMenuItem::separator();
    let item_quit = MenuItem::new("終了", true, None);
    for (r, name) in [
        (menu.append(&item_open), "open"),
        (menu.append(&item_pause), "pause"),
        (menu.append(&item_sep), "sep"),
        (menu.append(&item_quit), "quit"),
    ] {
        if let Err(e) = r {
            crate::logger::log(format!("tray: menu.append({name}) failed: {e}"));
        }
    }

    let open_id: MenuId = item_open.id().clone();
    let pause_id: MenuId = item_pause.id().clone();
    let quit_id: MenuId = item_quit.id().clone();

    // --- Win32 アクションをクロージャ化してイベントハンドラから呼ぶ ---
    // HWND は Send/Sync ではないので、`main_hwnd: isize` をキャプチャして呼び出し時に
    // `make_hwnd` で再構成する。Arc<...> は Send+Sync なのでそのままキャプチャして良い。
    //
    // 黒い矩形フラッシュの対策:
    // 1. `ShowWindow(SW_RESTORE)` を呼ばない (SW_RESTORE は minimize-from-restore の
    //    アニメーションをトリガして、元フレームがない状態で一瞬黒枠が見える)。
    //    保存していた WINDOWPLACEMENT で showCmd も適切に復元されるので不要。
    // 2. `SetWindowPlacement` は `ShowWindow(SW_SHOW)` より**前**に呼ぶ。後から呼ぶと
    //    表示済みウィンドウが移動/リサイズされて視覚的なジャンプになる。
    //    `SetWindowPlacement` の wp.showCmd が SW_SHOWNORMAL なら、この呼び出しで
    //    ウィンドウは同時に可視化されるため、別途 `ShowWindow` 呼出は不要。
    let hwnd_raw = main_hwnd;
    let do_show_window = {
        let ctx = egui_ctx.clone();
        let activity_gate = Arc::clone(&activity_gate);
        let io_sem = io_sem.clone();
        let event_tx = event_tx.clone();
        let placement_slot = Arc::clone(&placement_slot);
        move || {
            let hwnd = make_hwnd(hwnd_raw);

            // 保存していた配置があれば先に復元 (位置・サイズ + showCmd)。
            // このパスで SetWindowPlacement が showCmd=SW_SHOWNORMAL を含むので
            // 追加の ShowWindow は不要。
            let used_placement = {
                let mut slot = placement_slot.lock().unwrap();
                if let Some(p) = slot.take() {
                    restore_window_placement(hwnd_raw, &p);
                    true
                } else {
                    false
                }
            };
            if !used_placement {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                }
            }
            unsafe {
                let _ = SetForegroundWindow(hwnd);
            }
            if let Some(sem) = &io_sem {
                sem.set_throttled(false);
            }
            activity_gate.set_paused(false);
            let _ = event_tx.send(TrayEvent::OpenRequested);
            ctx.request_repaint();
            crate::logger::log(format!(
                "tray: Open → placement_restored={used_placement} + SetForegroundWindow"
            ));
        }
    };

    // メニューイベントを `set_event_handler` で受信 (receiver の race を回避)。
    {
        let event_tx = event_tx.clone();
        let ctx = egui_ctx.clone();
        let activity_gate = Arc::clone(&activity_gate);
        let quit_flag = Arc::clone(&quit_flag);
        let do_show_window = do_show_window.clone();
        let open_id = open_id.clone();
        let pause_id = pause_id.clone();
        let quit_id = quit_id.clone();
        MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
            crate::logger::log(format!("tray: MenuEvent id={:?}", ev.id));
            if ev.id == open_id {
                do_show_window();
            } else if ev.id == pause_id {
                // muda が CheckMenuItem を既にトグル済み。こちらも activity_gate を反転。
                let new_state = !activity_gate.is_paused();
                activity_gate.set_paused(new_state);
                let _ = event_tx.send(TrayEvent::TogglePauseRequested);
                ctx.request_repaint();
                crate::logger::log(format!(
                    "tray: Pause toggled → activity_gate.paused = {new_state}"
                ));
            } else if ev.id == quit_id {
                quit_flag.store(true, Ordering::SeqCst);
                let hwnd = make_hwnd(hwnd_raw);
                // WM_QUIT をメインスレッドに直接 post する。
                //
                // 以前は `PostMessageW(WM_CLOSE)` を使っていたが、winit がその close を
                // 処理するために hidden なウィンドウを一瞬可視化する副作用があり、
                // 終了直前にウィンドウが光って不自然だった。
                //
                // WM_QUIT は WndProc を経由せず、winit のイベントループの
                // `GetMessage`/`PeekMessage` 段階で検出されてそのままループ終了に
                // つながる。可視化が挟まらないので静かに終わる。
                // eframe の on_exit もこのルートで呼ばれる (hide_to_tray で既に
                // settings.save() + sidecar flush 済みなのでどちらでも安全)。
                unsafe {
                    let main_tid = GetWindowThreadProcessId(hwnd, None);
                    if main_tid != 0 {
                        let _ = PostThreadMessageW(
                            main_tid,
                            WM_QUIT,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    } else {
                        crate::logger::log(
                            "tray: GetWindowThreadProcessId returned 0 (cannot quit cleanly)",
                        );
                    }
                }
                let _ = event_tx.send(TrayEvent::QuitRequested);
                ctx.request_repaint();
                crate::logger::log("tray: Quit → quit_flag + PostThreadMessage(WM_QUIT)");
            }
        }));
    }

    // 左クリック (Up) でのみウィンドウ復帰 (ダブルクリックも同義)
    {
        let do_show_window = do_show_window.clone();
        TrayIconEvent::set_event_handler(Some(move |ev: TrayIconEvent| {
            let should_open = matches!(
                &ev,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );
            if should_open {
                crate::logger::log("tray: TrayIconEvent Left-click → ShowWindow");
                do_show_window();
            }
        }));
    }

    // 左クリックでメニューは出さない (直接ウィンドウ復帰)。右クリックだけメニュー表示。
    let tray = match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("mImageViewer")
        .with_icon(icon)
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            crate::logger::log(format!("tray: build failed: {e}"));
            return;
        }
    };

    crate::logger::log(format!(
        "tray: controller thread started (hwnd={main_hwnd:#x})"
    ));

    let mut iter: u64 = 0;
    while !shutdown.load(Ordering::Relaxed) {
        iter = iter.wrapping_add(1);

        // Win32 メッセージポンプ (メニュー表示・クリック処理のために必須)。
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // UI からのコマンド処理
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                TrayCommand::SetPausedCheck(p) => {
                    item_pause.set_checked(p);
                }
                TrayCommand::SetTooltip(t) => {
                    if let Err(e) = tray.set_tooltip(Some(t)) {
                        crate::logger::log(format!("tray: set_tooltip failed: {e}"));
                    }
                }
                TrayCommand::Shutdown => {
                    MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
                    TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
                    drop(tray);
                    crate::logger::log("tray: controller thread exiting");
                    return;
                }
            }
        }

        // 30 秒に 1 回「生きている」ログ (iter * 50ms = 600 で 30 秒)
        if iter.is_multiple_of(600) {
            crate::logger::log(format!("tray: thread alive (iter={iter})"));
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
    TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
    drop(tray);
    crate::logger::log("tray: controller thread exiting (shutdown flag)");
}
