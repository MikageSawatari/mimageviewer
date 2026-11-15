//! Standalone Win32 host for the actual detached native-input subclass.
//! No App, eframe backend, renderer, runtime DLL setup, or settings are opened.
//! Manual mode never injects input. Only an explicit `--auto` clicks the native
//! maximize button twice, with an OS-confirmed size transition between clicks.
//! The OS hit is checked on the actual button-down, never by a preparatory hit test.

use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
struct Options {
    auto: bool,
    log: PathBuf,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Self, String> {
        let mut result = Self {
            auto: false,
            log: "native-detached-probe.log".into(),
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--auto" {
                result.auto = true;
            } else if arg == "--log" {
                result.log = args.next().ok_or("--log requires a path")?.into();
            } else {
                return Err(format!("unknown argument: {}", arg.to_string_lossy()));
            }
        }
        Ok(result)
    }
}

#[cfg(windows)]
mod host {
    use super::Options;
    use mimageviewer::key_input::native_detached_probe::{ProbeSession, observe_removed_message};
    use std::{
        cell::RefCell,
        fs::File,
        io::Write,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::Instant,
    };
    use windows::{
        Win32::{
            Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
            Graphics::Gdi::{COLOR_WINDOW, GetSysColorBrush},
            System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
            UI::{
                HiDpi::{
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
                },
                Input::KeyboardAndMouse::{
                    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
                    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT,
                    SendInput,
                },
                WindowsAndMessaging::*,
            },
        },
        core::w,
    };

    const WAKE_AUTO: u32 = WM_APP + 1;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Phase {
        Manual,
        ReadyMaximize,
        AwaitMaximizePress,
        Maximizing,
        ReadyRestore,
        AwaitRestorePress,
        Restoring,
        Done,
        Failed { message: u32, wparam: usize },
    }

    impl Phase {
        fn pressed(self, message: u32, wparam: usize) -> Self {
            let next = match self {
                Self::AwaitMaximizePress => Self::Maximizing,
                Self::AwaitRestorePress => Self::Restoring,
                _ => return self,
            };
            match message {
                WM_NCLBUTTONDOWN if wparam == HTMAXBUTTON as usize => next,
                WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK | WM_LBUTTONDOWN | WM_LBUTTONDBLCLK
                | WM_NCLBUTTONUP | WM_LBUTTONUP => Self::Failed { message, wparam },
                _ => self,
            }
        }

        fn sized(self, size: usize) -> Self {
            match (self, size) {
                (Self::Maximizing, n) if n == SIZE_MAXIMIZED as usize => Self::ReadyRestore,
                (Self::Restoring, n) if n == SIZE_RESTORED as usize => Self::Done,
                _ => self,
            }
        }
    }

    struct Trace {
        start: Instant,
        tx: mpsc::Sender<String>,
        failed: Arc<AtomicBool>,
    }

    impl Trace {
        fn record(&self, detail: impl AsRef<str>) {
            let line = format!(
                "t_us={} {}",
                self.start.elapsed().as_micros(),
                detail.as_ref()
            );
            if self.tx.send(line).is_err() {
                self.failed.store(true, Ordering::Relaxed);
            }
        }
    }

    struct UiState {
        trace: Arc<Trace>,
        phase: Phase,
    }

    thread_local! {
        static UI: RefCell<Option<UiState>> = const { RefCell::new(None) };
    }

    fn record(detail: impl AsRef<str>) {
        UI.with(|ui| {
            if let Some(ui) = ui.borrow().as_ref() {
                ui.trace
                    .record(format!("phase={:?} {}", ui.phase, detail.as_ref()));
            }
        });
    }

    fn phase() -> Phase {
        UI.with(|ui| ui.borrow().as_ref().expect("UI initialized").phase)
    }

    fn set_phase(next: Phase) {
        UI.with(|ui| ui.borrow_mut().as_mut().expect("UI initialized").phase = next);
        record("source=auto phase_transition");
    }

    fn message_hit(message: u32, wp: usize) -> Option<isize> {
        match message {
            WM_NCMOUSEMOVE | WM_NCLBUTTONDOWN | WM_NCLBUTTONUP | WM_NCLBUTTONDBLCLK
            | WM_NCRBUTTONDOWN | WM_NCRBUTTONUP | WM_NCMBUTTONDOWN | WM_NCMBUTTONUP => {
                Some(wp as isize)
            }
            // Captured client messages can be outside the client area. They
            // carry no hit payload; button selection is witnessed by matching SC.
            _ => None,
        }
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        record(format!(
            "source=wndproc hwnd=0x{:x} msg=0x{msg:04x} wparam=0x{:x} lparam=0x{:x} hit={:?}",
            hwnd.0 as usize,
            wp.0,
            lp.0,
            message_hit(msg, wp.0)
        ));
        let current = phase();
        let next = current.pressed(msg, wp.0);
        if current != next {
            set_phase(next);
            record(format!(
                "source=auto actual_press msg=0x{msg:04x} wparam=0x{:x} verified={}",
                wp.0,
                matches!(next, Phase::Maximizing | Phase::Restoring)
            ));
            if matches!(next, Phase::Failed { .. }) {
                // Do not enter an unrelated OS caption loop on a bad target.
                // The outer dispatch returns the error without another click.
                return LRESULT(0);
            }
        }
        if msg == WM_SIZE {
            let current = phase();
            let next = current.sized(wp.0);
            if current != next {
                set_phase(next);
            }
        }
        if msg == WM_KEYDOWN && wp.0 == 0x1b {
            // Tool-local Escape exit; this is not an application/keymap action.
            let _ = unsafe { DestroyWindow(hwnd) };
            return LRESULT(0);
        }
        if msg == WM_DESTROY {
            unsafe { PostQuitMessage(0) };
        }
        let result = unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
        if msg == WM_NCHITTEST {
            record(format!(
                "source=wndproc_result msg=0x{msg:04x} hit={}",
                result.0
            ));
        }
        result
    }

    unsafe extern "system" fn getmessage_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 && wp.0 == PM_REMOVE.0 as usize && lp.0 != 0 {
            let msg = unsafe { &*(lp.0 as *const MSG) };
            record(format!(
                "source=hook_removed hwnd=0x{:x} msg=0x{:04x} wparam=0x{:x} lparam=0x{:x} message_time={} hit={:?}",
                msg.hwnd.0 as usize,
                msg.message,
                msg.wParam.0,
                msg.lParam.0,
                msg.time,
                message_hit(msg.message, msg.wParam.0)
            ));
            observe_removed_message(
                msg.hwnd.0 as usize as u64,
                msg.message,
                msg.wParam.0,
                msg.lParam.0,
            );
        }
        unsafe { CallNextHookEx(None, code, wp, lp) }
    }

    struct Window(HWND);
    impl Drop for Window {
        fn drop(&mut self) {
            if unsafe { IsWindow(Some(self.0)) }.as_bool() {
                let _ = unsafe { DestroyWindow(self.0) };
            }
        }
    }

    struct Hook(HHOOK);
    impl Drop for Hook {
        fn drop(&mut self) {
            let _ = unsafe { UnhookWindowsHookEx(self.0) };
        }
    }

    fn absolute_axis(value: i32, origin: i32, extent: i32) -> Result<i32, String> {
        if extent <= 1
            || value < origin
            || i64::from(value) >= i64::from(origin) + i64::from(extent)
        {
            return Err("caption button is outside the virtual desktop".into());
        }
        // Pick the middle of the target pixel in SendInput's 16-bit desktop map.
        Ok(
            ((i64::from(value) - i64::from(origin)) * 65_536 / i64::from(extent)
                + 32_768 / i64::from(extent))
            .min(65_535) as i32,
        )
    }

    fn click_maximize_button(hwnd: HWND) -> Result<(), String> {
        unsafe {
            let mut info = TITLEBARINFOEX {
                cbSize: std::mem::size_of::<TITLEBARINFOEX>() as u32,
                ..Default::default()
            };
            SendMessageW(
                hwnd,
                WM_GETTITLEBARINFOEX,
                Some(WPARAM(0)),
                Some(LPARAM((&mut info as *mut TITLEBARINFOEX) as isize)),
            );
            let rect = info.rgrect[3]; // OS accessibility rectangle for maximize/restore.
            if rect.right <= rect.left || rect.bottom <= rect.top {
                return Err("OS returned no maximize/restore button rectangle".into());
            }
            let point = POINT {
                x: ((i64::from(rect.left) + i64::from(rect.right)) / 2) as i32,
                y: ((i64::from(rect.top) + i64::from(rect.bottom)) / 2) as i32,
            };
            let x = absolute_axis(
                point.x,
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
            )?;
            let y = absolute_axis(
                point.y,
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )?;
            // Check immediately before input. Never click through another window.
            if WindowFromPoint(point) != hwnd {
                return Err(
                    "maximize/restore point is covered by another window; no input sent".into(),
                );
            }
            record(format!(
                "source=auto send_click x={} y={} hwnd=0x{:x} hit=await_actual_press",
                point.x, point.y, hwnd.0 as usize
            ));
            let mouse = |flags| INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: x,
                        dy: y,
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            };
            let inputs = [
                mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK),
                mouse(MOUSEEVENTF_LEFTDOWN),
                mouse(MOUSEEVENTF_LEFTUP),
            ];
            let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
            if sent != inputs.len() as u32 {
                return Err(format!(
                    "SendInput inserted {sent}/{} events; auto aborted",
                    inputs.len()
                ));
            }
        }
        Ok(())
    }

    fn drive_auto(hwnd: HWND) -> Result<(), String> {
        // Called only after the outer DispatchMessage returns. A WM_SIZE observed
        // inside DefWindowProc's caption loop never injects the next click there.
        match phase() {
            Phase::ReadyMaximize => {
                set_phase(Phase::AwaitMaximizePress);
                click_maximize_button(hwnd)?;
            }
            Phase::ReadyRestore => {
                set_phase(Phase::AwaitRestorePress);
                click_maximize_button(hwnd)?;
            }
            Phase::Done => unsafe {
                PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0))
                    .map_err(|e| e.to_string())?;
            },
            Phase::Failed { message, wparam } => {
                return Err(format!(
                    "actual click was not WM_NCLBUTTONDOWN/HTMAXBUTTON: msg=0x{message:04x} wparam=0x{wparam:x}; auto aborted"
                ));
            }
            _ => {}
        }
        Ok(())
    }

    fn run_window(auto: bool, trace: Arc<Trace>) -> Result<(), String> {
        UI.with(|ui| {
            *ui.borrow_mut() = Some(UiState {
                trace: trace.clone(),
                phase: if auto {
                    Phase::ReadyMaximize
                } else {
                    Phase::Manual
                },
            })
        });
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
                .map_err(|e| format!("DPI awareness: {e}"))?;
            let instance = HINSTANCE(GetModuleHandleW(None).map_err(|e| e.to_string())?.0);
            let class = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                hCursor: LoadCursorW(None, IDC_ARROW).map_err(|e| e.to_string())?,
                hbrBackground: GetSysColorBrush(COLOR_WINDOW),
                lpszClassName: w!("MivNativeDetachedProbe"),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err(format!(
                    "RegisterClassW: {}",
                    windows::core::Error::from_win32()
                ));
            }
            let window = Window(
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    class.lpszClassName,
                    w!("mIV native detached probe - Escape or Close exits"),
                    WS_OVERLAPPEDWINDOW,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    800,
                    500,
                    None,
                    None,
                    Some(instance),
                    None,
                )
                .map_err(|e| e.to_string())?,
            );
            let hwnd = window.0;
            let _hook = Hook(
                SetWindowsHookExW(
                    WH_GETMESSAGE,
                    Some(getmessage_hook),
                    None,
                    GetCurrentThreadId(),
                )
                .map_err(|e| format!("WH_GETMESSAGE: {e}"))?,
            );
            let sink_trace = trace.clone();
            let _session = ProbeSession::attach(
                hwnd.0 as usize as u64,
                Arc::new(move |line| {
                    sink_trace.record(format!("source=shared {line}"));
                }),
            )?;
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            if auto {
                PostMessageW(Some(hwnd), WAKE_AUTO, WPARAM(0), LPARAM(0))
                    .map_err(|e| e.to_string())?;
            }
            let mut msg = MSG::default();
            loop {
                let status = GetMessageW(&mut msg, None, 0, 0).0;
                if status == -1 {
                    return Err(format!(
                        "GetMessageW: {}",
                        windows::core::Error::from_win32()
                    ));
                }
                if status == 0 {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                if IsWindow(Some(hwnd)).as_bool() {
                    drive_auto(hwnd)?;
                }
            }
            if auto && phase() != Phase::Done {
                return Err(format!(
                    "auto ended before maximize/restore completed: {:?}",
                    phase()
                ));
            }
        }
        Ok(())
    }

    pub fn run(options: Result<Options, String>) -> Result<(), String> {
        let log_path = options
            .as_ref()
            .map(|o| o.log.clone())
            .unwrap_or_else(|_| "native-detached-probe.log".into());
        let mut file =
            File::create(&log_path).map_err(|e| format!("{}: {e}", log_path.display()))?;
        let (tx, rx) = mpsc::channel::<String>();
        let failed = Arc::new(AtomicBool::new(false));
        let writer_failed = failed.clone();
        let writer = std::thread::spawn(move || {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            for line in rx {
                let stdout_result = writeln!(out, "{line}").and_then(|_| out.flush());
                let file_result = writeln!(file, "{line}").and_then(|_| file.flush());
                if stdout_result.is_err() || file_result.is_err() {
                    writer_failed.store(true, Ordering::Relaxed);
                    eprintln!("native_detached_probe: trace output failed");
                    break;
                }
            }
        });
        let trace = Arc::new(Trace {
            start: Instant::now(),
            tx,
            failed: failed.clone(),
        });
        trace.record(format!(
            "source=tool start log={} options={options:?}",
            log_path.display()
        ));
        let result = options.and_then(|options| run_window(options.auto, trace.clone()));
        match &result {
            Ok(()) => trace.record("source=tool completed"),
            Err(error) => trace.record(format!("source=tool error={error}")),
        }
        UI.with(|ui| *ui.borrow_mut() = None);
        drop(trace);
        let joined = writer.join();
        result?;
        if joined.is_err() || failed.load(Ordering::Relaxed) {
            return Err("trace writer failed".into());
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn auto_requires_confirmed_size_transitions() {
            assert_eq!(
                Phase::ReadyMaximize.sized(SIZE_MAXIMIZED as usize),
                Phase::ReadyMaximize
            );
            assert_eq!(
                Phase::Maximizing.sized(SIZE_RESTORED as usize),
                Phase::Maximizing
            );
            assert_eq!(
                Phase::Maximizing.sized(SIZE_MAXIMIZED as usize),
                Phase::ReadyRestore
            );
            assert_eq!(
                Phase::Restoring.sized(SIZE_MAXIMIZED as usize),
                Phase::Restoring
            );
            assert_eq!(Phase::Restoring.sized(SIZE_RESTORED as usize), Phase::Done);
            for phase in [Phase::AwaitMaximizePress, Phase::AwaitRestorePress] {
                assert_eq!(phase.sized(SIZE_MAXIMIZED as usize), phase);
                assert_eq!(phase.sized(SIZE_RESTORED as usize), phase);
            }
        }

        #[test]
        fn auto_requires_actual_max_button_press_before_each_size_transition() {
            let maximizing =
                Phase::AwaitMaximizePress.pressed(WM_NCLBUTTONDOWN, HTMAXBUTTON as usize);
            assert_eq!(maximizing, Phase::Maximizing);
            assert_eq!(
                maximizing.sized(SIZE_MAXIMIZED as usize),
                Phase::ReadyRestore
            );
            let restoring =
                Phase::AwaitRestorePress.pressed(WM_NCLBUTTONDOWN, HTMAXBUTTON as usize);
            assert_eq!(restoring, Phase::Restoring);
            assert_eq!(restoring.sized(SIZE_RESTORED as usize), Phase::Done);
            // Size changes and later input cannot restart either completed click.
            assert_eq!(
                Phase::Done.pressed(WM_NCLBUTTONDOWN, HTMAXBUTTON as usize),
                Phase::Done
            );
            assert_eq!(Phase::Done.sized(SIZE_MAXIMIZED as usize), Phase::Done);
        }

        #[test]
        fn auto_rejects_wrong_actual_hit_and_never_retries() {
            for awaiting in [Phase::AwaitMaximizePress, Phase::AwaitRestorePress] {
                for (message, wparam) in [
                    (WM_NCLBUTTONDOWN, HTCAPTION as usize),
                    (WM_NCLBUTTONDOWN, HTMINBUTTON as usize),
                    (WM_NCLBUTTONDOWN, HTCLOSE as usize),
                    (WM_NCLBUTTONDBLCLK, HTMAXBUTTON as usize),
                    (WM_LBUTTONDOWN, 0),
                    (WM_LBUTTONDBLCLK, 0),
                    (WM_LBUTTONUP, 0),
                    (WM_NCLBUTTONUP, HTMAXBUTTON as usize),
                ] {
                    let failed = awaiting.pressed(message, wparam);
                    assert_eq!(failed, Phase::Failed { message, wparam });
                    assert_eq!(failed.sized(SIZE_MAXIMIZED as usize), failed);
                    assert_eq!(failed.sized(SIZE_RESTORED as usize), failed);
                    assert_eq!(
                        failed.pressed(WM_NCLBUTTONDOWN, HTMAXBUTTON as usize),
                        failed
                    );
                }
                for message in [WM_NCHITTEST, WM_NCMOUSEMOVE, WM_MOUSEMOVE] {
                    assert_eq!(awaiting.pressed(message, HTMAXBUTTON as usize), awaiting);
                }
            }
        }

        #[test]
        fn native_hit_payload_and_virtual_desktop_coordinates() {
            assert_eq!(message_hit(WM_LBUTTONUP, 0), None);
            assert_eq!(
                message_hit(WM_NCLBUTTONUP, HTMAXBUTTON as usize),
                Some(HTMAXBUTTON as isize)
            );
            assert!(absolute_axis(0, 0, 0).is_err());
            assert!(absolute_axis(-101, -100, 200).is_err());
            assert!(absolute_axis(100, -100, 200).is_err());
            assert_eq!(absolute_axis(0, -100, 200).unwrap(), 32931);
        }
    }
}

fn main() {
    let options = Options::parse(std::env::args_os().skip(1));
    #[cfg(windows)]
    let result = host::run(options);
    #[cfg(not(windows))]
    let result =
        options.and_then(|_| Err::<(), _>("native_detached_probe requires Windows".into()));
    if let Err(error) = result {
        eprintln!("native_detached_probe: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_is_default_and_auto_is_explicit() {
        assert!(!Options::parse([]).unwrap().auto);
        assert!(Options::parse(["--auto".into()]).unwrap().auto);
        assert!(Options::parse(["--log".into()]).is_err());
        assert!(Options::parse(["--automatic".into()]).is_err());
        assert_eq!(
            Options::parse(["--log".into(), "custom.log".into()])
                .unwrap()
                .log,
            PathBuf::from("custom.log")
        );
    }
}
