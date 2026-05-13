use std::ffi::c_void;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
// `GetCursorPos` / `GetClientRect` 等は WindowsAndMessaging から (上の `use` を参照)。
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::Ime::{
    GCS_COMPSTR, GCS_RESULTSTR, IME_COMPOSITION_STRING, ISC_SHOWUICOMPOSITIONWINDOW,
    ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetActiveWindow, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT,
    TrackMouseEvent, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CREATESTRUCTW, CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetClientRect,
    GetCursorPos, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId,
    HTCLIENT, HWND_TOP, IDC_ARROW, IsWindow, IsWindowVisible, LoadCursorW, MA_ACTIVATE,
    MA_ACTIVATEANDEAT, MSG, PM_REMOVE, PeekMessageW, PostQuitMessage, RegisterClassW, SW_SHOW,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_SHOWWINDOW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOWPOS,
    WM_APPCOMMAND, WM_CHAR, WM_CLOSE, WM_DESTROY, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION,
    WM_IME_SETCONTEXT, WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEACTIVATE,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_WINDOWPOSCHANGED, WM_XBUTTONDBLCLK, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW,
    WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW, WS_POPUP,
    WS_VISIBLE,
};
use windows::core::w;

#[derive(Clone, Copy, Debug)]
pub struct NativeVideoKeyEvent {
    pub virtual_key: u32,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub repeat: bool,
}

#[derive(Clone, Debug)]
pub enum NativeVideoImeEvent {
    Enabled,
    Preedit(String),
    Commit(String),
    Disabled,
}

#[derive(Clone, Debug)]
pub enum NativeVideoWindowEvent {
    KeyDown(NativeVideoKeyEvent),
    KeyUp(NativeVideoKeyEvent),
    Text(char),
    Ime(NativeVideoImeEvent),
    MouseMove(NativeVideoMouseEvent),
    MouseButton(NativeVideoMouseButtonEvent),
    MouseWheel(NativeVideoMouseWheelEvent),
    MouseLeave,
    /// presenter HWND の `WM_WINDOWPOSCHANGED` で発火。HUD overlay HWND を
    /// presenter のジオメトリに追従させるために presenter thread が消費する。
    /// UI 側には転送しない。
    GeometryChanged {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    },
    /// HUD overlay HWND の `WM_DPICHANGED` で発火。`suggested_rect` は
    /// `WM_DPICHANGED` の lparam で渡される新 DPI 用 RECT。
    /// presenter thread 内で pixels_per_point 更新 + resize + 次フレーム region 再計算に使う。
    DpiChanged {
        dpi: u32,
        suggested_rect: RECT,
    },
    /// HUD overlay HWND の `WM_WINDOWPOSCHANGING` で「自分より前に別 window が
    /// 割り込みそう」を検知したときに送る raise 要求。presenter thread が
    /// `RaiseHudToTop` に内部変換する。これは best-effort safety net。
    RequestRaiseHud,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeVideoMouseButton {
    Left,
    Right,
    Middle,
    Extra1,
    Extra2,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeVideoMouseEvent {
    pub x: i32,
    pub y: i32,
    pub shift: bool,
    pub ctrl: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeVideoMouseButtonEvent {
    pub button: NativeVideoMouseButton,
    pub down: bool,
    pub double_click: bool,
    pub x: i32,
    pub y: i32,
    pub shift: bool,
    pub ctrl: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeVideoMouseWheelEvent {
    pub delta: i16,
    pub x: i32,
    pub y: i32,
    pub shift: bool,
    pub ctrl: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum NativeVideoWindowMode {
    Windowed { width: u32, height: u32 },
    Borderless { rect: RECT },
}

pub struct NativeVideoWindowConfig {
    pub mode: NativeVideoWindowMode,
    pub owner_hwnd: u64,
    /// `false` の場合、HWND は hidden で作成し、呼び出し側が DComp 初期化後に
    /// `show_and_raise` で表示する。native fullscreen presenter の透明期間を避けるため。
    pub initially_visible: bool,
    pub close_on_escape: bool,
    pub post_quit_on_destroy: bool,
    pub event_tx: Option<std::sync::mpsc::Sender<NativeVideoWindowEvent>>,
}

impl NativeVideoWindowConfig {
    pub fn test_windowed(width: u32, height: u32) -> Self {
        Self {
            mode: NativeVideoWindowMode::Windowed { width, height },
            owner_hwnd: 0,
            initially_visible: true,
            close_on_escape: true,
            post_quit_on_destroy: true,
            event_tx: None,
        }
    }
}

pub struct NativeVideoWindow {
    hwnd: HWND,
}

struct WindowState {
    close_on_escape: bool,
    post_quit_on_destroy: bool,
    event_tx: Option<std::sync::mpsc::Sender<NativeVideoWindowEvent>>,
    ime_preediting: bool,
}

impl NativeVideoWindow {
    pub fn create(config: NativeVideoWindowConfig) -> Result<Self, String> {
        unsafe {
            let hmodule = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e:?}"))?;
            let hinstance = HINSTANCE(hmodule.0);
            let cursor = LoadCursorW(None, IDC_ARROW).ok();
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinstance,
                hCursor: cursor.unwrap_or_default(),
                lpszClassName: w!("mIVNativeVideoWindow"),
                ..Default::default()
            };
            RegisterClassW(&wc);

            let (ex_style, style, x, y, width, height, raise_on_show) = match config.mode {
                NativeVideoWindowMode::Windowed { width, height } => {
                    let mut style = WS_OVERLAPPEDWINDOW;
                    if config.initially_visible {
                        style |= WS_VISIBLE;
                    }
                    let ex_style = WINDOW_EX_STYLE::default();
                    let mut rect = RECT {
                        left: 0,
                        top: 0,
                        right: width as i32,
                        bottom: height as i32,
                    };
                    AdjustWindowRectEx(&mut rect, style, false, ex_style)
                        .map_err(|e| format!("AdjustWindowRectEx: {e:?}"))?;
                    (
                        ex_style,
                        style,
                        CW_USEDEFAULT,
                        CW_USEDEFAULT,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        false,
                    )
                }
                NativeVideoWindowMode::Borderless { rect } => {
                    let mut style = WS_POPUP | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;
                    if config.initially_visible {
                        style |= WS_VISIBLE;
                    }
                    let ex_style = WS_EX_NOREDIRECTIONBITMAP;
                    (
                        ex_style,
                        style,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        true,
                    )
                }
            };

            let state = Box::new(WindowState {
                close_on_escape: config.close_on_escape,
                post_quit_on_destroy: config.post_quit_on_destroy,
                event_tx: config.event_tx,
                ime_preediting: false,
            });
            let state_ptr = Box::into_raw(state);
            let owner_hwnd = if config.owner_hwnd != 0 {
                Some(HWND(config.owner_hwnd as *mut _))
            } else {
                None
            };
            let hwnd = match CreateWindowExW(
                ex_style,
                w!("mIVNativeVideoWindow"),
                w!("mIV Native Video Window"),
                style,
                x,
                y,
                width,
                height,
                owner_hwnd,
                None,
                Some(hinstance),
                Some(state_ptr.cast()),
            ) {
                Ok(hwnd) => hwnd,
                Err(err) => {
                    let _ = Box::from_raw(state_ptr);
                    return Err(format!("CreateWindowExW: {err:?}"));
                }
            };
            crate::dwm_transitions::disable_transitions_for_window(hwnd);
            if config.initially_visible {
                let _ = ShowWindow(hwnd, SW_SHOW);
            }
            if config.initially_visible && raise_on_show {
                bring_hwnd_to_front(hwnd);
                log_window_state("created", hwnd);
            } else if !config.initially_visible {
                log_window_state("created-hidden", hwnd);
            }
            Ok(Self { hwnd })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn show_and_raise(&self) -> bool {
        if self.hwnd.0.is_null() {
            return false;
        }
        unsafe {
            if !IsWindow(Some(self.hwnd)).as_bool() {
                return false;
            }
            crate::dwm_transitions::disable_transitions_for_window(self.hwnd);
            let _ = ShowWindow(self.hwnd, SW_SHOW);
        }
        let raised = bring_hwnd_to_front(self.hwnd);
        log_window_state("shown", self.hwnd);
        raised
    }

    pub fn destroy(&mut self) {
        if self.hwnd.0.is_null() {
            return;
        }
        unsafe {
            if IsWindow(Some(self.hwnd)).as_bool() {
                let _ = DestroyWindow(self.hwnd);
            }
        }
        self.hwnd = HWND::default();
    }
}

pub fn bring_to_front(hwnd_raw: u64) -> bool {
    if hwnd_raw == 0 {
        return false;
    }
    unsafe {
        let hwnd = HWND(hwnd_raw as *mut _);
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        bring_hwnd_to_front(hwnd)
    }
}

pub fn foreground_belongs_to_current_process() -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.0.is_null() {
            return true;
        }
        let mut foreground_pid = 0_u32;
        let _ = GetWindowThreadProcessId(foreground, Some(&mut foreground_pid));
        foreground_pid == 0 || foreground_pid == GetCurrentProcessId()
    }
}

/// `foreground_belongs_to_current_process` の保守的版。
/// foreground=null / pid=0 の不確定ケースは false を返す
/// (= 「mIV が前面と確信できない」場合は奪還しない)。
pub fn foreground_belongs_to_current_process_strict() -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.0.is_null() {
            return false;
        }
        let mut foreground_pid = 0_u32;
        let _ = GetWindowThreadProcessId(foreground, Some(&mut foreground_pid));
        foreground_pid != 0 && foreground_pid == GetCurrentProcessId()
    }
}

pub fn is_window_alive(hwnd_raw: u64) -> bool {
    if hwnd_raw == 0 {
        return false;
    }
    unsafe { IsWindow(Some(HWND(hwnd_raw as *mut _))).as_bool() }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ForegroundClaimReport {
    pub foreground_hwnd: u64,
    pub post_foreground_hwnd: u64,
    pub target_hwnd: u64,
    pub attach_thread_input_ok: bool,
    pub set_foreground_ok: bool,
    pub set_active_ok: bool,
    pub set_focus_ok: bool,
}

/// `SetForegroundWindow` cooperative ルールを掻い潜って HWND を最前面に上げる。
/// Alt+Tab で他アプリが foreground を持っている状態でも安定して動く best-effort。
/// 詳細ログ用に各 API 結果を返す。
///
/// `target_hwnd_raw == 0` または `!IsWindow(target)` の場合は何もせず、
/// 全 ok=false の report を返す (共通 utility なので呼び出し側で stale HWND を
/// 渡しても安全に no-op になる)。
pub fn claim_foreground(target_hwnd_raw: u64) -> ForegroundClaimReport {
    if target_hwnd_raw == 0 || !is_window_alive(target_hwnd_raw) {
        return ForegroundClaimReport {
            target_hwnd: target_hwnd_raw,
            ..ForegroundClaimReport::default()
        };
    }
    unsafe {
        let target = HWND(target_hwnd_raw as *mut c_void);
        let foreground = GetForegroundWindow();
        let this_tid = GetCurrentThreadId();
        let foreground_tid = if !foreground.0.is_null() {
            GetWindowThreadProcessId(foreground, None)
        } else {
            0
        };
        let attached = foreground_tid != 0
            && foreground_tid != this_tid
            && AttachThreadInput(this_tid, foreground_tid, true).as_bool();
        let set_foreground_ok = SetForegroundWindow(target).as_bool();
        let set_active_ok = SetActiveWindow(target).is_ok();
        let set_focus_ok = SetFocus(Some(target)).is_ok();
        let post_foreground = GetForegroundWindow();
        if attached {
            let _ = AttachThreadInput(this_tid, foreground_tid, false);
        }
        ForegroundClaimReport {
            foreground_hwnd: foreground.0 as u64,
            post_foreground_hwnd: post_foreground.0 as u64,
            target_hwnd: target.0 as u64,
            attach_thread_input_ok: attached,
            set_foreground_ok,
            set_active_ok,
            set_focus_ok,
        }
    }
}

fn bring_hwnd_to_front(hwnd: HWND) -> bool {
    unsafe {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW;
        SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, flags).is_ok()
    }
}

pub fn log_state(hwnd_raw: u64, label: &str) {
    if hwnd_raw == 0 {
        return;
    }
    unsafe {
        let hwnd = HWND(hwnd_raw as *mut _);
        if IsWindow(Some(hwnd)).as_bool() {
            log_window_state(label, hwnd);
        }
    }
}

fn log_window_state(label: &str, hwnd: HWND) {
    unsafe {
        let mut rect = RECT::default();
        let rect_ok = GetWindowRect(hwnd, &mut rect).is_ok();
        let visible = IsWindowVisible(hwnd).as_bool();
        let foreground = GetForegroundWindow();
        crate::logger::log(format!(
            "[native-video] window {label}: hwnd=0x{:x} visible={} rect_ok={} rect=({},{} {}x{}) foreground=0x{:x}",
            hwnd.0 as usize,
            visible,
            rect_ok,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            foreground.0 as usize
        ));
        crate::perf::event(
            "native_presenter",
            "window_state",
            None,
            0,
            &[
                ("label", serde_json::Value::from(label)),
                ("hwnd", serde_json::Value::from(hwnd.0 as usize as u64)),
                ("visible", serde_json::Value::from(visible)),
                ("rect_ok", serde_json::Value::from(rect_ok)),
                ("left", serde_json::Value::from(rect.left)),
                ("top", serde_json::Value::from(rect.top)),
                ("width", serde_json::Value::from(rect.right - rect.left)),
                ("height", serde_json::Value::from(rect.bottom - rect.top)),
                (
                    "foreground",
                    serde_json::Value::from(foreground.0 as usize as u64),
                ),
            ],
        );
    }
}

impl Drop for NativeVideoWindow {
    fn drop(&mut self) {
        self.destroy();
    }
}

pub fn pump_thread_messages() -> bool {
    let mut quit = false;
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_QUIT {
                quit = true;
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    quit
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let createstruct = lparam.0 as *const CREATESTRUCTW;
            if !createstruct.is_null() {
                let state = unsafe { (*createstruct).lpCreateParams } as *mut WindowState;
                if !state.is_null() {
                    unsafe {
                        let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
                    }
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_MOUSEACTIVATE => {
            // 他アプリから戻ってきたときの左クリックは「フォーカスを戻すためのクリック」
            // とみなし、再生 toggle に作用させない。MA_ACTIVATEANDEAT はアクティブ化を
            // 引き起こしたマウス down メッセージ (= WM_LBUTTONDOWN) を Windows が破棄
            // する動作で、結果として App 経路 (handle_native_video_mouse_button) と
            // egui overlay 経路 (overlay_draw.rs primary_clicked) のどちらの click
            // 判定も成立しなくなる。WM_LBUTTONUP 単独は届く可能性があるが、App 側は
            // 対応する down 記録が無ければ無視するし、egui も down 抜きの up を
            // click 扱いしないので副作用は出ない。
            //
            // 画像フルスクリーン (ui_fullscreen.rs の fs_suppress_primary_until_release)
            // と同じく左クリックのみ抑制し、右/中ボタンによるアクティブ化は通常通り
            // 通す (右クリック = フルスクリーン終了がそのまま走るのは画像側挙動と
            // 整合する)。LOWORD(lparam) == HTCLIENT で「クライアント領域上のクリック」
            // だけを対象にし、test_windowed (debug 用ウィンドウモード) で title bar や
            // resize 枠をクリックして戻るときの操作まで食べないようにする。
            // WM_LBUTTONDBLCLK は通常 WM_LBUTTONDOWN の後に来るので trigger になる
            // ことは稀だが、念のため同等扱い。
            let hit_test = (lparam.0 & 0xFFFF) as u32;
            let trigger_msg = ((lparam.0 >> 16) & 0xFFFF) as u32;
            let is_left = trigger_msg == WM_LBUTTONDOWN || trigger_msg == WM_LBUTTONDBLCLK;
            if hit_test == HTCLIENT as u32 && is_left {
                LRESULT(MA_ACTIVATEANDEAT as isize)
            } else {
                LRESULT(MA_ACTIVATE as isize)
            }
        }
        WM_KEYDOWN => {
            if let Some(tx) = window_state(hwnd).and_then(|s| s.event_tx.as_ref()) {
                let _ = tx.send(NativeVideoWindowEvent::KeyDown(native_key_event(
                    wparam, lparam,
                )));
            }
            if wparam.0 as u32 == 0x1B && window_state(hwnd).is_some_and(|s| s.close_on_escape) {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_KEYUP => {
            if let Some(tx) = window_state(hwnd).and_then(|s| s.event_tx.as_ref()) {
                let _ = tx.send(NativeVideoWindowEvent::KeyUp(native_key_event(
                    wparam, lparam,
                )));
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_APPCOMMAND => {
            // Mouse driver が進む/戻るボタンを APPCOMMAND_BROWSER_BACKWARD/FORWARD で
            // 送ってくる経路 (Chrome / Explorer 等の標準ナビ経路) を受ける。
            // HIWORD(lparam) の下 12 bit が AppCommand コード。
            //  1 = APPCOMMAND_BROWSER_BACKWARD (= 戻る = Ctrl+↑ 相当)
            //  2 = APPCOMMAND_BROWSER_FORWARD  (= 進む = Ctrl+↓ 相当)
            let cmd_word = ((lparam.0 >> 16) & 0xFFFF) as u32;
            let app_command = cmd_word & 0xFFF;
            let synth_vk = match app_command {
                1 => Some(0xA6_u32), // VK_BROWSER_BACK
                2 => Some(0xA7_u32), // VK_BROWSER_FORWARD
                _ => None,
            };
            if let Some(vk) = synth_vk
                && let Some(tx) = window_state(hwnd).and_then(|s| s.event_tx.as_ref())
            {
                let _ = tx.send(NativeVideoWindowEvent::KeyDown(NativeVideoKeyEvent {
                    virtual_key: vk,
                    shift: false,
                    ctrl: false,
                    alt: false,
                    repeat: false,
                }));
                // WM_APPCOMMAND の規約: 処理した場合 TRUE を返す。
                return LRESULT(1);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CHAR => {
            if let Some(ch) = char::from_u32(wparam.0 as u32)
                && !ch.is_control()
                && let Some(tx) = window_state(hwnd).and_then(|s| s.event_tx.as_ref())
            {
                let _ = tx.send(NativeVideoWindowEvent::Text(ch));
            }
            LRESULT(0)
        }
        WM_IME_STARTCOMPOSITION => {
            if let Some(state) = window_state_mut(hwnd) {
                state.ime_preediting = false;
                send_ime_event(state, NativeVideoImeEvent::Enabled);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_IME_COMPOSITION => {
            if let Some(state) = window_state_mut(hwnd) {
                let flags = lparam.0 as u32;
                if flags == 0 {
                    state.ime_preediting = false;
                    send_ime_event(state, NativeVideoImeEvent::Preedit(String::new()));
                }
                if (flags & GCS_RESULTSTR.0) != 0
                    && let Some(text) = ime_composition_string(hwnd, GCS_RESULTSTR)
                {
                    state.ime_preediting = false;
                    send_ime_event(state, NativeVideoImeEvent::Preedit(String::new()));
                    if !text.is_empty() {
                        send_ime_event(state, NativeVideoImeEvent::Commit(text));
                    }
                }
                if (flags & GCS_COMPSTR.0) != 0
                    && let Some(text) = ime_composition_string(hwnd, GCS_COMPSTR)
                {
                    state.ime_preediting = true;
                    send_ime_event(state, NativeVideoImeEvent::Preedit(text));
                }
                // egui draws the preedit text itself; suppress the default IME
                // composition window that otherwise appears at the top-left.
                LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
        }
        WM_IME_ENDCOMPOSITION => {
            if let Some(state) = window_state_mut(hwnd) {
                if state.ime_preediting
                    && let Some(text) = ime_composition_string(hwnd, GCS_RESULTSTR)
                {
                    send_ime_event(state, NativeVideoImeEvent::Preedit(String::new()));
                    if !text.is_empty() {
                        send_ime_event(state, NativeVideoImeEvent::Commit(text));
                    }
                }
                state.ime_preediting = false;
                send_ime_event(state, NativeVideoImeEvent::Disabled);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_IME_SETCONTEXT => {
            let lparam = LPARAM(lparam.0 & !(ISC_SHOWUICOMPOSITIONWINDOW as isize));
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_MOUSEMOVE => {
            track_mouse_leave(hwnd);
            if let Some(tx) = window_state(hwnd).and_then(|s| s.event_tx.as_ref()) {
                // CP9 実機 debug: presenter wndproc 経由の mouse は HUD region 外 (= 穴)
                // のときに来るので、これが頻発しているなら HUD region に問題がある。
                // 100ms 周期 rate limit で log。
                if std::env::var_os("MIV_HUD_DEBUG").is_some() {
                    static LAST_LOG_MS: std::sync::atomic::AtomicI64 =
                        std::sync::atomic::AtomicI64::new(0);
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    let last = LAST_LOG_MS.load(std::sync::atomic::Ordering::Relaxed);
                    if now_ms - last >= 100 {
                        LAST_LOG_MS.store(now_ms, std::sync::atomic::Ordering::Relaxed);
                        let evt = native_mouse_event(wparam, lparam);
                        crate::logger::log(format!(
                            "[HUD-DEBUG] presenter WM_MOUSEMOVE x={} y={}",
                            evt.x, evt.y
                        ));
                    }
                }
                let _ = tx.send(NativeVideoWindowEvent::MouseMove(native_mouse_event(
                    wparam, lparam,
                )));
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
        | WM_MBUTTONUP | WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK | WM_MBUTTONDBLCLK
        | WM_XBUTTONDOWN | WM_XBUTTONUP | WM_XBUTTONDBLCLK => {
            if mouse_message_is_down(msg) {
                unsafe {
                    let _ = SetCapture(hwnd);
                }
            } else {
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            if let Some(tx) = window_state(hwnd).and_then(|s| s.event_tx.as_ref()) {
                let _ = tx.send(NativeVideoWindowEvent::MouseButton(
                    native_mouse_button_event(msg, wparam, lparam),
                ));
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_MOUSEWHEEL => {
            if let Some(tx) = window_state(hwnd).and_then(|s| s.event_tx.as_ref()) {
                let _ = tx.send(NativeVideoWindowEvent::MouseWheel(
                    native_mouse_wheel_event(hwnd, wparam, lparam),
                ));
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_MOUSELEAVE => {
            // CP9 実機修正: cursor が presenter client rect 内なら MouseLeave を流さない。
            //
            // 背景: presenter HWND が `WM_MOUSELEAVE` を受けるのは「cursor が presenter から
            // 出た」通知。ところが HUD overlay HWND が前面にあると、cursor が HUD region 内に
            // 入った瞬間に OS から見て「presenter から離脱」になり、WM_MOUSELEAVE が来る。
            // これを overlay の pointer_pos=None として流すと top_bar_visible=false → region
            // 縮小 → cursor が region 外に → presenter が再度 mouse 受ける → 振動ループ。
            //
            // 真の「cursor が presenter から完全に出た」は cursor polling の client rect 範囲外
            // 検出で十分カバーされる (CP6 `cursor_polling_tick`)。なので presenter wndproc では
            // 「実際に画面外/他 window に行ったか」をその場で確認し、内なら流さない。
            //
            // HUD HWND がないフォールバック経路では polling 自体が動かないので、その場合は
            // 従来通り MouseLeave を流す (= cursor が画面外に出たことを overlay に伝える)。
            // 判定は `GetCursorPos` + `ScreenToClient` で current cursor が presenter client
            // rect 内かを見る。
            let cursor_in_client = unsafe {
                let mut pt = POINT::default();
                if GetCursorPos(&mut pt).is_ok() && ScreenToClient(hwnd, &mut pt).as_bool() {
                    let mut rc = windows::Win32::Foundation::RECT::default();
                    if GetClientRect(hwnd, &mut rc).is_ok() {
                        pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom
                    } else {
                        false
                    }
                } else {
                    false
                }
            };
            if !cursor_in_client {
                // 真に外に出た → overlay に MouseLeave 流す。
                if let Some(tx) = window_state(hwnd).and_then(|s| s.event_tx.as_ref()) {
                    let _ = tx.send(NativeVideoWindowEvent::MouseLeave);
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_WINDOWPOSCHANGED => {
            // CP8: presenter HWND の位置 / サイズが変わったら HUD HWND に mirror する。
            // `lparam` は `WINDOWPOS*`。
            //
            // **Codex CP8 P1 反映**: `SWP_NOMOVE` / `SWP_NOSIZE` が立っているとき、その field
            // (x/y or cx/cy) は無視される値で、bogus。z-order 変更だけの `SetWindowPos(...,
            // SWP_NOMOVE | SWP_NOSIZE, ...)` でも `WM_WINDOWPOSCHANGED` が発火するため、
            // フラグを見ずに値を信じると HUD geometry / presenter.resize が bogus で走る。
            //
            // 対策:
            //   - `SWP_NOMOVE && SWP_NOSIZE` 両方なら event 発火せず skip。
            //   - どちらかだけ立っているなら `GetWindowRect(hwnd)` で現在値を取り直す。
            //
            // borderless `WS_POPUP` なので window cx/cy = client サイズ前提。
            if lparam.0 != 0 {
                let wp = lparam.0 as *const WINDOWPOS;
                if !wp.is_null() {
                    let (flags_value, mut x, mut y, mut w, mut h) = unsafe {
                        let p = &*wp;
                        (p.flags.0, p.x, p.y, p.cx, p.cy)
                    };
                    let no_move = (flags_value & SWP_NOMOVE.0) != 0;
                    let no_size = (flags_value & SWP_NOSIZE.0) != 0;
                    if !(no_move && no_size) {
                        if no_move || no_size {
                            // 片方だけ bogus なので `GetWindowRect` で現在値を取り直す。
                            let mut rc = windows::Win32::Foundation::RECT::default();
                            if unsafe { GetWindowRect(hwnd, &mut rc) }.is_ok() {
                                if no_move {
                                    x = rc.left;
                                    y = rc.top;
                                }
                                if no_size {
                                    w = rc.right - rc.left;
                                    h = rc.bottom - rc.top;
                                }
                            }
                        }
                        let w_u32 = w.max(1) as u32;
                        let h_u32 = h.max(1) as u32;
                        if let Some(tx) = window_state(hwnd).and_then(|s| s.event_tx.as_ref()) {
                            let _ = tx.send(NativeVideoWindowEvent::GeometryChanged {
                                x,
                                y,
                                w: w_u32,
                                h: h_u32,
                            });
                        }
                    }
                    // 両方 NoMove + NoSize は z-order だけの変更 → event 発火しない。
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            if window_state(hwnd).is_some_and(|s| s.post_quit_on_destroy) {
                unsafe {
                    PostQuitMessage(0);
                }
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                unsafe {
                    let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let _ = Box::from_raw(ptr);
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn window_state(hwnd: HWND) -> Option<&'static WindowState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const WindowState;
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &*ptr })
    }
}

fn window_state_mut(hwnd: HWND) -> Option<&'static mut WindowState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &mut *ptr })
    }
}

fn send_ime_event(state: &WindowState, event: NativeVideoImeEvent) {
    if let Some(tx) = state.event_tx.as_ref() {
        let _ = tx.send(NativeVideoWindowEvent::Ime(event));
    }
}

fn ime_composition_string(hwnd: HWND, mode: IME_COMPOSITION_STRING) -> Option<String> {
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.0.is_null() {
            return None;
        }
        let byte_len = ImmGetCompositionStringW(himc, mode, None, 0);
        let result = if byte_len < 0 {
            None
        } else if byte_len == 0 {
            Some(String::new())
        } else {
            let mut buf = vec![0_u16; byte_len as usize / 2];
            let read = ImmGetCompositionStringW(
                himc,
                mode,
                Some(buf.as_mut_ptr().cast::<c_void>()),
                byte_len as u32,
            );
            if read < 0 {
                None
            } else {
                buf.truncate(read as usize / 2);
                Some(String::from_utf16_lossy(&buf))
            }
        };
        let _ = ImmReleaseContext(hwnd, himc);
        result
    }
}

fn native_key_event(wparam: WPARAM, lparam: LPARAM) -> NativeVideoKeyEvent {
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let alt = unsafe { GetKeyState(VK_MENU.0 as i32) } < 0;
    NativeVideoKeyEvent {
        virtual_key: wparam.0 as u32,
        shift,
        ctrl,
        alt,
        repeat: ((lparam.0 as u64) & (1 << 30)) != 0,
    }
}

fn native_mouse_event(wparam: WPARAM, lparam: LPARAM) -> NativeVideoMouseEvent {
    NativeVideoMouseEvent {
        x: signed_low_word(lparam.0),
        y: signed_high_word(lparam.0),
        shift: mouse_shift(wparam),
        ctrl: mouse_ctrl(wparam),
    }
}

fn native_mouse_button_event(
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> NativeVideoMouseButtonEvent {
    let button = match msg {
        WM_RBUTTONDOWN | WM_RBUTTONUP | WM_RBUTTONDBLCLK => NativeVideoMouseButton::Right,
        WM_MBUTTONDOWN | WM_MBUTTONUP | WM_MBUTTONDBLCLK => NativeVideoMouseButton::Middle,
        WM_XBUTTONDOWN | WM_XBUTTONUP | WM_XBUTTONDBLCLK => {
            // HIWORD(wparam): XBUTTON1=1 (back), XBUTTON2=2 (forward)
            match ((wparam.0 >> 16) & 0xFFFF) as u16 {
                2 => NativeVideoMouseButton::Extra2,
                _ => NativeVideoMouseButton::Extra1,
            }
        }
        _ => NativeVideoMouseButton::Left,
    };
    NativeVideoMouseButtonEvent {
        button,
        down: mouse_message_is_down(msg),
        double_click: matches!(
            msg,
            WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK | WM_MBUTTONDBLCLK | WM_XBUTTONDBLCLK
        ),
        x: signed_low_word(lparam.0),
        y: signed_high_word(lparam.0),
        shift: mouse_shift(wparam),
        ctrl: mouse_ctrl(wparam),
    }
}

fn native_mouse_wheel_event(
    hwnd: HWND,
    wparam: WPARAM,
    lparam: LPARAM,
) -> NativeVideoMouseWheelEvent {
    let mut point = POINT {
        x: signed_low_word(lparam.0),
        y: signed_high_word(lparam.0),
    };
    unsafe {
        let _ = ScreenToClient(hwnd, &mut point);
    }
    NativeVideoMouseWheelEvent {
        delta: signed_high_word(wparam.0 as isize) as i16,
        x: point.x,
        y: point.y,
        shift: mouse_shift(wparam),
        ctrl: mouse_ctrl(wparam),
    }
}

fn mouse_message_is_down(msg: u32) -> bool {
    matches!(
        msg,
        WM_LBUTTONDOWN
            | WM_RBUTTONDOWN
            | WM_MBUTTONDOWN
            | WM_XBUTTONDOWN
            | WM_LBUTTONDBLCLK
            | WM_RBUTTONDBLCLK
            | WM_MBUTTONDBLCLK
            | WM_XBUTTONDBLCLK
    )
}

fn mouse_shift(wparam: WPARAM) -> bool {
    (wparam.0 & 0x0004) != 0
}

fn mouse_ctrl(wparam: WPARAM) -> bool {
    (wparam.0 & 0x0008) != 0
}

fn track_mouse_leave(hwnd: HWND) {
    let mut track = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe {
        let _ = TrackMouseEvent(&mut track);
    }
}

fn signed_low_word(value: isize) -> i32 {
    ((value as u32 & 0xffff) as i16) as i32
}

fn signed_high_word(value: isize) -> i32 {
    (((value as u32 >> 16) & 0xffff) as i16) as i32
}
