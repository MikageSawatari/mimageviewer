use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW,
    DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetWindowLongPtrW, IDC_ARROW,
    IsWindow, LoadCursorW, MSG, PM_REMOVE, PeekMessageW, PostQuitMessage, RegisterClassW, SW_SHOW,
    SetWindowLongPtrW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY,
    WM_KEYDOWN, WM_NCCREATE, WM_NCDESTROY, WNDCLASSW, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
    WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
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
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinstance,
                hCursor: cursor.unwrap_or_default(),
                lpszClassName: w!("mIVNativeVideoWindow"),
                ..Default::default()
            };
            RegisterClassW(&wc);

            let (ex_style, style, x, y, width, height) = match config.mode {
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
            if let Some(state) = window_state(hwnd) {
                if let Some(tx) = &state.event_tx {
                    let _ = tx.send(NativeVideoWindowEvent::KeyDown(native_key_event(
                        wparam, lparam,
                    )));
                }
            }
            if wparam.0 as u32 == 0x1B && window_state(hwnd).is_some_and(|s| s.close_on_escape) {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return LRESULT(0);
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
