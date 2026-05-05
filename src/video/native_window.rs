use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CREATESTRUCTW, CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId,
    HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, IDC_ARROW, IsWindow, IsWindowVisible, LoadCursorW, MSG,
    PM_REMOVE, PeekMessageW, PostQuitMessage, RegisterClassW, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WNDCLASSW, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_NOREDIRECTIONBITMAP,
    WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
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

#[derive(Clone, Copy, Debug)]
pub enum NativeVideoWindowEvent {
    KeyDown(NativeVideoKeyEvent),
    KeyUp(NativeVideoKeyEvent),
    MouseMove(NativeVideoMouseEvent),
    MouseButton(NativeVideoMouseButtonEvent),
    MouseWheel(NativeVideoMouseWheelEvent),
    MouseLeave,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeVideoMouseButton {
    Left,
    Right,
    Middle,
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
    pub close_on_escape: bool,
    pub post_quit_on_destroy: bool,
    pub event_tx: Option<std::sync::mpsc::Sender<NativeVideoWindowEvent>>,
}

impl NativeVideoWindowConfig {
    pub fn test_windowed(width: u32, height: u32) -> Self {
        Self {
            mode: NativeVideoWindowMode::Windowed { width, height },
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
                    let style = WS_OVERLAPPEDWINDOW | WS_VISIBLE;
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
                    let style = WS_POPUP | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;
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
            });
            let state_ptr = Box::into_raw(state);
            let hwnd = match CreateWindowExW(
                ex_style,
                w!("mIVNativeVideoWindow"),
                w!("mIV Native Video Window"),
                style,
                x,
                y,
                width,
                height,
                None,
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
            let _ = ShowWindow(hwnd, SW_SHOW);
            if raise_on_show {
                bring_hwnd_to_front(hwnd);
                log_window_state("created", hwnd);
            }
            Ok(Self { hwnd })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
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

fn bring_hwnd_to_front(hwnd: HWND) -> bool {
    unsafe {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW;
        let top_ok = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, flags).is_ok();
        // A plain HWND_TOP raise can lose a same-process activation race to the thumbnail grid
        // during double-click fullscreen startup. Pulse through the TOPMOST band and immediately
        // demote back to normal so the native video stays above mIV windows without becoming an
        // always-on-top window over other applications.
        let pulse_ok = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, flags).is_ok()
            && SetWindowPos(hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags).is_ok();
        top_ok || pulse_ok
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
        WM_MOUSEMOVE => {
            track_mouse_leave(hwnd);
            if let Some(tx) = window_state(hwnd).and_then(|s| s.event_tx.as_ref()) {
                let _ = tx.send(NativeVideoWindowEvent::MouseMove(native_mouse_event(
                    wparam, lparam,
                )));
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
        | WM_MBUTTONUP | WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK | WM_MBUTTONDBLCLK => {
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
            if let Some(tx) = window_state(hwnd).and_then(|s| s.event_tx.as_ref()) {
                let _ = tx.send(NativeVideoWindowEvent::MouseLeave);
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
        _ => NativeVideoMouseButton::Left,
    };
    NativeVideoMouseButtonEvent {
        button,
        down: mouse_message_is_down(msg),
        double_click: matches!(msg, WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK | WM_MBUTTONDBLCLK),
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
            | WM_LBUTTONDBLCLK
            | WM_RBUTTONDBLCLK
            | WM_MBUTTONDBLCLK
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
