use super::*;
use std::cell::RefCell;
use windows_sys::Win32::{
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SWP_NOSENDCHANGING,
        SendMessageW, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    },
};

// Smaller than the desktop's default tracking limit so that removing our subclass
// cannot accidentally pass just because DefWindowProc already capped the request.
const LIMIT: u32 = 1024;

thread_local! {
    static CAPTURED_WARNINGS: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

struct WarningCapture;
impl log::Log for WarningCapture {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target() == LOG_TARGET && metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            CAPTURED_WARNINGS.with(|warnings| {
                if let Some(warnings) = warnings.borrow_mut().as_mut() {
                    warnings.push(record.args().to_string());
                }
            });
        }
    }

    fn flush(&self) {}
}

fn capture_warnings(action: impl FnOnce()) -> Vec<String> {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        log::set_logger(&WarningCapture).unwrap();
        log::set_max_level(log::LevelFilter::Warn);
    });
    CAPTURED_WARNINGS.with(|warnings| *warnings.borrow_mut() = Some(Vec::new()));
    action();
    CAPTURED_WARNINGS.with(|warnings| warnings.borrow_mut().take().unwrap())
}

struct TestWindow(HWND);

impl TestWindow {
    fn new() -> Self {
        // Each test thread owns and destroys its hidden window; no visibility/focus/input calls.
        unsafe {
            let class: Vec<u16> = "egui_wgpu_size_limit_test\0".encode_utf16().collect();
            let instance = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..Default::default()
            };
            RegisterClassW(&wc); // Already registered is fine for concurrent tests.
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                20,
                30,
                640,
                480,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            assert!(
                !hwnd.is_null(),
                "CreateWindowExW: {}",
                io::Error::last_os_error()
            );
            Self(hwnd)
        }
    }

    fn register(&self) {
        register(self.0 as isize, LIMIT).unwrap();
    }

    fn resize(&self, x: i32, y: i32, width: i32, height: i32, flags: u32) {
        assert_ne!(
            unsafe {
                SetWindowPos(
                    self.0,
                    std::ptr::null_mut(),
                    x,
                    y,
                    width,
                    height,
                    flags | SWP_NOZORDER | SWP_NOACTIVATE,
                )
            },
            0
        );
    }

    fn rect(&self) -> RECT {
        outer_rect(self.0).unwrap()
    }

    fn client_size(&self) -> PhysicalSize<u32> {
        let mut rect = RECT::default();
        assert_ne!(unsafe { GetClientRect(self.0, &mut rect) }, 0);
        PhysicalSize::new(
            (rect.right - rect.left) as u32,
            (rect.bottom - rect.top) as u32,
        )
    }

    fn assert_bounded(&self) {
        let rect = self.rect();
        // DefWindowProc can impose a smaller desktop-dependent maximum before our proc.
        assert!(0 < rect.right - rect.left && rect.right - rect.left <= LIMIT as i32);
        assert!(0 < rect.bottom - rect.top && rect.bottom - rect.top <= LIMIT as i32);
        let client = self.client_size();
        assert!(client.width <= LIMIT && client.height <= LIMIT);
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        assert_ne!(unsafe { DestroyWindow(self.0) }, 0);
    }
}

#[test]
fn clamp_both_dimensions() {
    assert_eq!(limited_size(12000, 9000, 0, 8192), (8192, 8192));
}

#[test]
fn clamp_preserves_within_limit() {
    assert_eq!(limited_size(800, 600, 0, 8192), (800, 600));
    assert_eq!(limited_size(8192, 8192, 0, 8192), (8192, 8192));
}

#[test]
fn clamp_nosize_is_unchanged() {
    assert_eq!(limited_size(12000, 9000, SWP_NOSIZE, 8192), (12000, 9000));
}

#[test]
fn clamp_one_dimension() {
    assert_eq!(limited_size(12000, 600, 0, 8192), (8192, 600));
    assert_eq!(limited_size(800, 9000, 0, 8192), (800, 8192));
}

#[test]
fn hwnd_subclass_limits_explicit_size_preserving_position() {
    let window = TestWindow::new();
    window.register();
    window.register(); // Idempotent.
    window.resize(100, 110, 12000, 9000, 0);
    let rect = window.rect();
    assert_eq!((rect.left, rect.top), (100, 110));
    window.assert_bounded();
}

#[test]
fn hwnd_within_limit_and_nosize_move_are_unchanged() {
    let window = TestWindow::new();
    window.register();
    window.resize(120, 130, 900, 700, 0);
    let rect = window.rect();
    assert_eq!(
        (
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top
        ),
        (120, 130, 900, 700)
    );
    window.resize(210, 220, 12000, 9000, SWP_NOSIZE);
    let rect = window.rect();
    assert_eq!(
        (
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top
        ),
        (210, 220, 900, 700)
    );
}

// Simulate winit's downstream min/max handler, proving our caps run after the next proc.
unsafe extern "system" fn minmax_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    if message == WM_GETMINMAXINFO {
        let info = unsafe { &mut *(lparam as *mut MINMAXINFO) };
        info.ptMaxTrackSize.x = 12000;
        info.ptMaxTrackSize.y = 700;
        info.ptMaxSize.x = 600;
        info.ptMaxSize.y = 9000;
    } else if message == WM_WINDOWPOSCHANGING {
        let pos = unsafe { &mut *(lparam as *mut WINDOWPOS) };
        (pos.cx, pos.cy) = (12000, 9000);
    }
    result
}

#[test]
fn hwnd_minmax_caps_downstream_values_without_growing_them() {
    let window = TestWindow::new();
    assert_ne!(
        unsafe { SetWindowSubclass(window.0, Some(minmax_proc), 1, 0) },
        0
    );
    window.register();
    let mut info = MINMAXINFO::default();
    unsafe { SendMessageW(window.0, WM_GETMINMAXINFO, 0, &mut info as *mut _ as isize) };
    assert_eq!(
        (info.ptMaxTrackSize.x, info.ptMaxTrackSize.y),
        (LIMIT as i32, 700)
    );
    assert_eq!((info.ptMaxSize.x, info.ptMaxSize.y), (600, LIMIT as i32));
}

#[test]
fn hwnd_windowpos_caps_after_downstream_adjustment() {
    let window = TestWindow::new();
    assert_ne!(
        unsafe { SetWindowSubclass(window.0, Some(minmax_proc), 1, 0) },
        0
    );
    window.register();
    let mut pos = WINDOWPOS {
        hwnd: window.0,
        x: 100,
        y: 110,
        cx: 800,
        cy: 600,
        flags: SWP_NOZORDER | SWP_NOACTIVATE,
        ..Default::default()
    };
    unsafe {
        SendMessageW(
            window.0,
            WM_WINDOWPOSCHANGING,
            0,
            &mut pos as *mut _ as isize,
        )
    };
    assert_eq!(
        (pos.x, pos.y, pos.cx, pos.cy),
        (100, 110, LIMIT as i32, LIMIT as i32)
    );
}

#[test]
fn hwnd_destroy_does_not_affect_sibling_constraint() {
    let first = TestWindow::new();
    let second = TestWindow::new();
    first.register();
    second.register();
    drop(first);
    second.resize(150, 160, 12000, 9000, 0);
    second.assert_bounded();
}

#[test]
fn hwnd_enforce_corrects_bypass_and_returns_actual_client_size() {
    let window = TestWindow::new();
    window.register();
    window.resize(180, 190, 12000, 9000, SWP_NOSENDCHANGING);
    assert!(window.rect().right - window.rect().left > LIMIT as i32);
    let size = enforce(window.0 as isize, LIMIT).unwrap();
    let rect = window.rect();
    assert_eq!((rect.left, rect.top), (180, 190));
    window.assert_bounded();
    assert_eq!(size, window.client_size());
    assert!(size.width <= LIMIT && size.height <= LIMIT);
    assert_eq!(enforce(window.0 as isize, LIMIT).unwrap(), size);
}

#[test]
fn hwnd_registration_rejects_a_different_thread() {
    let window = TestWindow::new();
    let hwnd = window.0 as isize;
    std::thread::spawn(move || {
        assert!(register(hwnd, LIMIT).is_err());
        assert!(enforce(hwnd, LIMIT).is_err());
    })
    .join()
    .unwrap();
}

#[test]
fn hwnd_logs_each_oversized_request_but_not_within_limit_or_nosize() {
    let window = TestWindow::new();
    window.register();
    let warnings = capture_warnings(|| {
        window.resize(100, 110, 900, 700, 0);
        window.resize(100, 110, 12000, 9000, SWP_NOSIZE);
        window.resize(100, 110, 12000, 9000, 0);
        window.resize(100, 110, 12000, 9000, 0);
    });
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    for warning in warnings {
        assert!(warning.starts_with("Limiting HWND"));
        assert!(warning.contains("(12000, 9000)"));
        assert!(warning.contains("device limit 1024"));
    }
}

#[test]
fn hwnd_logs_post_chain_clamp_even_when_input_is_within_limit() {
    let window = TestWindow::new();
    assert_ne!(
        unsafe { SetWindowSubclass(window.0, Some(minmax_proc), 1, 0) },
        0
    );
    window.register();
    let mut pos = WINDOWPOS {
        hwnd: window.0,
        cx: 800,
        cy: 600,
        flags: SWP_NOZORDER | SWP_NOACTIVATE,
        ..Default::default()
    };
    let warnings = capture_warnings(|| {
        unsafe {
            SendMessageW(
                window.0,
                WM_WINDOWPOSCHANGING,
                0,
                &mut pos as *mut _ as isize,
            )
        };
    });
    assert_eq!((pos.cx, pos.cy), (LIMIT as i32, LIMIT as i32));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("(12000, 9000)"));
}

#[test]
fn hwnd_logs_each_enforce_correction() {
    let window = TestWindow::new();
    window.register();
    let warnings = capture_warnings(|| {
        for _ in 0..2 {
            window.resize(100, 110, 12000, 9000, SWP_NOSENDCHANGING);
            enforce(window.0 as isize, LIMIT).unwrap();
        }
    });
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings
            .iter()
            .all(|warning| warning.starts_with("Correcting HWND"))
    );
    window.assert_bounded();
}
