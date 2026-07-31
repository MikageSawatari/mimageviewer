//! タスクトレイ常駐サポート (v0.9)。
//!
//! 目的: ウィンドウ [×] ボタンでプロセス終了する代わりにタスクトレイに収め、
//! notify-rs によるファイル監視を継続することで次回起動時の再スキャン負荷を避ける。
//!
//! ## 設計上の要点 (eframe 0.33 + tray-icon 0.20 の制約)
//!
//! eframe/winit は **ウィンドウが非表示の間 `App::update` を呼ばない**。
//! `ViewportCommand::Visible(false)` でも Win32 `SW_HIDE` でも同じ。`request_repaint`
//! は最終的に hidden HWND への `RedrawWindow(..., RDW_INTERNALPAINT)` となるが、Windows は
//! hidden window へ `WM_PAINT` を配送しない。したがって「トレイメニューをクリック →
//! App::update で処理」という素直な流れは**成立しない**。
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
//! - **常駐中の再生**: App が既存 media owner から「再生 intent または連続 EOF 遷移中」を
//!   projection している間だけ、トレイスレッドの既存 50ms pump が hidden main HWND へ
//!   `WM_PAINT` を 1 件 post する。これは winit の `RedrawRequested` を明示的に起こすための
//!   Windows bridge であり、paused / EOF 停止 / still / 可視 window では post しない。
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

use crossbeam_channel::{Receiver, Sender, bounded};

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
        // `[u8; N]` に書き写して Send+Sync な値として保持する (WINDOWPLACEMENT 自体は
        // 内部に Point/Rect を持つだけで opaque ハンドル等は含まない)。
        let mut out = [0u8; std::mem::size_of::<WINDOWPLACEMENT>()];
        std::ptr::copy_nonoverlapping(
            (&wp as *const WINDOWPLACEMENT) as *const u8,
            out.as_mut_ptr(),
            std::mem::size_of::<WINDOWPLACEMENT>(),
        );
        Some(SavedWindowPlacement { bytes: out })
    }
}

#[cfg(windows)]
pub(crate) fn restore_window_placement(hwnd_raw: isize, saved: &SavedWindowPlacement) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPlacement, WINDOWPLACEMENT};
    unsafe {
        // `[u8; N]` は alignment=1 なので `*const WINDOWPLACEMENT` への dereference は UB。
        // `read_unaligned` で安全に読む。
        let wp = std::ptr::read_unaligned(saved.bytes.as_ptr() as *const WINDOWPLACEMENT);
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
    /// 一時停止トグル (muda の CheckMenuItem が自動トグルした新状態を同梱)。
    /// `pause_indexer_while_minimized` 設定とトレイ checkmark は App 側で反映する。
    /// activity_gate はトレイスレッドが既に反転済みだが、「ウィンドウ表示中は強制 false」等
    /// の統合判断は App 側で行うため、ここでは hint に留める。
    TogglePauseRequested { new_checked: bool },
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
    /// CheckMenuItem (pause) の現在状態。トレイスレッドがユーザークリックで反転し、
    /// `SetPausedCheck` コマンドでも同期される (= 常に表示中の checkmark と一致)。
    /// App 側は `TogglePauseRequested` イベントの取りこぼし対策としてこれを reconcile
    /// ソースに使う (bounded(16) で drop されても設定が stale にならない)。
    pause_checked: Arc<AtomicBool>,
    /// hidden residency 中に UI-owned media state machine を進める必要があるか。
    /// App の既存 player / EOF transition owner から導出した projection であり、
    /// tray residency 自体や detached window の有無を wake 条件にはしない。
    resident_media_wake_enabled: Arc<AtomicBool>,
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
        let pause_checked = Arc::new(AtomicBool::new(false));
        let pause_checked_th = Arc::clone(&pause_checked);
        let resident_media_wake_enabled = Arc::new(AtomicBool::new(false));
        let resident_media_wake_enabled_th = Arc::clone(&resident_media_wake_enabled);

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
                    pause_checked_th,
                    resident_media_wake_enabled_th,
                );
            })
            .ok()?;

        Some(Self {
            event_rx,
            cmd_tx,
            shutdown,
            thread: Some(thread),
            quit_flag,
            pause_checked,
            resident_media_wake_enabled,
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

    /// トレイメニュー pause の現在 checkmark を読む (Codex P3 の reconcile 用)。
    /// `TogglePauseRequested` イベントが bounded channel overflow でドロップされても、
    /// App はこれを見て `settings.pause_indexer_while_minimized` に反映できる。
    pub fn pause_checked_snapshot(&self) -> bool {
        self.pause_checked.load(Ordering::Relaxed)
    }

    /// ノンブロッキングでイベントを受信。無ければ None。毎フレーム呼ぶ想定。
    pub fn try_recv(&self) -> Option<TrayEvent> {
        self.event_rx.try_recv().ok()
    }

    /// 「一時停止」メニュー項目のチェック状態を更新。
    /// App 側から設定ページなど経由で paused が変化したときの同期用。
    ///
    /// 共有 atomic を**即時**更新してから command を送る点に注意 (Codex P2)。
    /// `cmd_tx` は bounded channel + tray thread の 50ms ポーリングで遅延があるため、
    /// command 処理前に `poll_tray_events` の `reconcile_pause_state` が走ると、
    /// 古い atomic 値で `settings.pause_indexer_while_minimized` を巻き戻す race が
    /// 起きる。atomic を先に書き換えれば次フレームの reconcile は no-op になり、
    /// tray thread 側の `item_pause.set_checked` だけ遅延適用される (見た目だけ
    /// 数十ミリ秒遅れる) 形に収まる。同 atomic への二重 store は冪等。
    pub fn set_paused_check(&self, paused: bool) {
        self.pause_checked.store(paused, Ordering::Relaxed);
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

    /// UI-owned media state machine needs explicit hidden-window ticks only while playback or
    /// its continuous-EOF handoff is active. This is a latest-value projection; the tray thread
    /// samples it from its existing 50ms pump without adding another timer or worker.
    pub(crate) fn set_resident_media_wake_enabled(&self, enabled: bool) -> bool {
        self.resident_media_wake_enabled
            .swap(enabled, Ordering::AcqRel)
            != enabled
    }
}

fn should_post_resident_media_wake(
    main_window_visible: bool,
    resident_media_wake_enabled: bool,
) -> bool {
    !main_window_visible && resident_media_wake_enabled
}

#[cfg(test)]
impl TrayController {
    /// テスト専用: 実トレイスレッドを起動せずに、atomic + channel だけ持つコントローラを
    /// 組み立てる。`set_paused_check` の atomic 即時更新セマンティクス検証用。
    /// 返り値の `Sender<TrayEvent>` は test 側で `_event_tx` として束縛しておくこと
    /// (drop すると `event_rx` 側が disconnected になるが、テスト本体はチャネル受信を
    /// しないので影響は無い。leak 回避目的で明示的に持つ)。
    fn new_for_test() -> (Self, Receiver<TrayCommand>, Sender<TrayEvent>) {
        let (event_tx, event_rx) = bounded::<TrayEvent>(16);
        let (cmd_tx, cmd_rx) = bounded::<TrayCommand>(16);
        let ctrl = Self {
            event_rx,
            cmd_tx,
            shutdown: Arc::new(AtomicBool::new(false)),
            thread: None,
            quit_flag: Arc::new(AtomicBool::new(false)),
            pause_checked: Arc::new(AtomicBool::new(false)),
            resident_media_wake_enabled: Arc::new(AtomicBool::new(false)),
        };
        (ctrl, cmd_rx, event_tx)
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
    pause_checked: Arc<AtomicBool>,
    resident_media_wake_enabled: Arc<AtomicBool>,
) {
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, IsWindowVisible, MSG, PM_REMOVE, PeekMessageW, PostMessageW, SW_SHOW,
        SW_SHOWNOACTIVATE, SetForegroundWindow, ShowWindow, TranslateMessage, WM_CLOSE, WM_PAINT,
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

    // CheckMenuItem (Rc<...> を含み !Send) は MenuEvent 用の 'static + Send closure に
    // 持ち込めないので、checkmark の現状態を Send+Sync な AtomicBool (TrayController から
    // 共有) で並行追跡する。App 側はこれを snapshot して設定値と reconcile できる
    // (bounded channel overflow で TogglePauseRequested が drop された場合の保険)。

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
            // ⚠ ロックを握ったまま `SetWindowPlacement` を呼ばないこと。あの API は
            // カーネルコールバックでこのスレッドのウィンドウプロシージャを呼び戻し、
            // トレイイベントのハンドラ (= このクロージャ) に再入する。`std::sync::Mutex`
            // は再入不可なので、そこで自己デッドロックし、以降トレイのクリックも
            // 右クリックメニューも一切反応しなくなる (2026-07-30 実害。メインウィンドウは
            // トレイへ格納済みなので復帰手段が無くなる)。
            // 取り出しだけロック内で行い、guard を落としてから Win32 を呼ぶ。
            let saved = placement_slot.lock().unwrap().take();
            let used_placement = if let Some(p) = saved {
                restore_window_placement(hwnd_raw, &p);
                true
            } else {
                false
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
            // try_send: チャネルが埋まっていても(トレイ常駐中に update が drain できない等)
            // ドロップしてよい。状態変更はクロージャ内で既に適用済み。
            let _ = event_tx.try_send(TrayEvent::OpenRequested);
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
        let pause_checked = Arc::clone(&pause_checked);
        MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
            crate::logger::log(format!("tray: MenuEvent id={:?}", ev.id));
            if ev.id == open_id {
                do_show_window();
            } else if ev.id == pause_id {
                // muda が CheckMenuItem を既にトグル済み。item_pause は !Send なので
                // 直接参照できないが、AtomicBool で反転を追跡する (SetPausedCheck
                // コマンドとも同期される)。`fetch_xor` で原子的に反転し、反転前の値の
                // 否定が新値。これで高速連打や SetPausedCheck との競合でも状態が drift
                // しない。
                let prev = pause_checked.fetch_xor(true, Ordering::Relaxed);
                let new_checked = !prev;
                activity_gate.set_paused(new_checked);
                // try_send ドロップに備え、App は `pause_checked_snapshot` で reconcile する。
                let _ = event_tx.try_send(TrayEvent::TogglePauseRequested { new_checked });
                ctx.request_repaint();
                crate::logger::log(format!("tray: Pause toggled → new_checked = {new_checked}"));
            } else if ev.id == quit_id {
                quit_flag.store(true, Ordering::SeqCst);
                let hwnd = make_hwnd(hwnd_raw);
                // 常に eframe の通常 close 経路を通って on_exit / Drop を走らせる。
                // 非表示状態では winit が hidden window に対して `update` を回さないため
                // close_requested が処理されない。そこで以下の手順で「winit は可視と見るが
                // 画面には出ない」状態に持っていく:
                //   1. DWM の DWMWA_CLOAK を有効化 → コンポジットされないので画面に出ない
                //   2. ShowWindow(SW_SHOWNOACTIVATE) → Win32 的に可視に (winit が update を回す)
                //   3. PostMessage(WM_CLOSE) → 通常の close フロー
                // 結果として黒フラッシュ / 復元アニメーションを完全に抑制しつつ、インデクサや
                // tag writer 等の graceful shutdown を保証できる。
                unsafe {
                    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
                    if !IsWindowVisible(hwnd).as_bool() {
                        // BOOL は 4 バイト整数 (TRUE=1)。DWM 側は *const c_void で受ける
                        // のでサイズを厳密に指定するだけでよい (型は BOOL 相当)。
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
                let _ = event_tx.try_send(TrayEvent::QuitRequested);
                ctx.request_repaint();
                crate::logger::log("tray: Quit → DWM cloak + SW_SHOWNOACTIVATE + WM_CLOSE");
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
            // set_event_handler で登録した global handler が生き残ったままになるので
            // 明示的にクリアしてから return する (無効な HWND への send を避ける)。
            MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
            TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
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
                    pause_checked.store(p, Ordering::Relaxed);
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

        // winit turns WM_PAINT into RedrawRequested, which is the only normal entry to
        // eframe::App::update. RedrawWindow/request_repaint cannot create WM_PAINT for a hidden
        // HWND, so active resident playback needs this explicit bridge. The App publishes the
        // latest projection from the existing media owners; checking IsWindowVisible here also
        // closes the hide/show race without another App flag.
        let main_window_visible = unsafe { IsWindowVisible(make_hwnd(main_hwnd)).as_bool() };
        if should_post_resident_media_wake(
            main_window_visible,
            resident_media_wake_enabled.load(Ordering::Acquire),
        ) {
            unsafe {
                let _ = PostMessageW(Some(make_hwnd(main_hwnd)), WM_PAINT, WPARAM(0), LPARAM(0));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `set_paused_check` は **atomic を即時更新してから** command を送信すること。
    /// 順序が逆転すると、command 送信〜tray スレッドの 50ms ポーリング待機の間に
    /// `App::reconcile_pause_state` が古い atomic を読んで `settings` を巻き戻す
    /// race が再発する (Codex P2 修正)。
    ///
    /// 本テストはコマンド送信先 receiver を test 側で保持し、`set_paused_check`
    /// 直後に `pause_checked_snapshot()` が新しい値を返していることを assert する
    /// (= atomic store が同期的に終わっている)。
    #[test]
    fn set_paused_check_updates_atomic_synchronously_before_send() {
        let (ctrl, cmd_rx, _event_tx) = TrayController::new_for_test();
        assert!(!ctrl.pause_checked_snapshot(), "初期は false");

        ctrl.set_paused_check(true);
        // 同スレッドからの load: store が return より前に実行されているはず。
        assert!(
            ctrl.pause_checked_snapshot(),
            "set_paused_check return 直後に atomic は新しい値を反映していること"
        );
        // command も channel に積まれている (= 後段の tray thread が読み取り可能)。
        match cmd_rx.try_recv() {
            Ok(TrayCommand::SetPausedCheck(true)) => {}
            other => panic!("SetPausedCheck(true) コマンドが届かない: {other:?}"),
        }

        // 反転も同様
        ctrl.set_paused_check(false);
        assert!(!ctrl.pause_checked_snapshot());
        match cmd_rx.try_recv() {
            Ok(TrayCommand::SetPausedCheck(false)) => {}
            other => panic!("SetPausedCheck(false) が届かない: {other:?}"),
        }
    }

    #[test]
    fn resident_media_wake_is_bounded_to_hidden_active_playback() {
        assert!(should_post_resident_media_wake(false, true));
        assert!(!should_post_resident_media_wake(true, true));
        assert!(!should_post_resident_media_wake(false, false));
        assert!(!should_post_resident_media_wake(true, false));
    }

    /// 同じ値で 2 回叩いても冪等 (atomic への二重 store は無害、command 2 通は届く)。
    #[test]
    fn set_paused_check_is_idempotent_for_same_value() {
        let (ctrl, cmd_rx, _event_tx) = TrayController::new_for_test();
        ctrl.set_paused_check(true);
        ctrl.set_paused_check(true);
        assert!(ctrl.pause_checked_snapshot());
        // command channel には 2 個積まれている
        let mut count = 0;
        while cmd_rx.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 2, "同値でも command 2 通は積まれる");
    }
}
