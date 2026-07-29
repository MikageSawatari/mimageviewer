//! Stage 3 disposable Windows harness for the native-video window-thread plan.
//!
//! This module is compiled only for Windows unit tests. It intentionally does not use the
//! production native-video window classes, settings/profile, decoder, or application runtime.

use std::cell::Cell;
use std::ffi::c_void;
use std::fmt;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11Texture2D,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter1, IDXGIDevice,
    IDXGIFactory2, IDXGIOutput, IDXGISwapChain1,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GWLP_USERDATA, GetClientRect, GetMessageW, GetWindowLongPtrW,
    GetWindowThreadProcessId, IsWindow, MSG, PM_REMOVE, PeekMessageW, PostMessageW,
    PostThreadMessageW, RegisterClassW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
    SetWindowLongPtrW, SetWindowPos, TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_CLOSE,
    WM_DESTROY, WM_NCCREATE, WM_NCDESTROY, WM_QUIT, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN,
    WS_CLIPSIBLINGS, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows::core::{Interface, w};
use winreg::RegKey;
use winreg::enums::HKEY_LOCAL_MACHINE;

use super::window_host_contract::{
    WindowBackendOperation, WindowHostChannel, WindowsHarnessError, WindowsHarnessInvariant,
    WindowsHarnessPhase, WindowsHarnessTimeout,
};

const TEST_NAME: &str = "video::native_window_thread_spike::cross_thread_dcomp_present_remains_pump_independent_when_render_stalls";
const CHILD_ENV: &str = "MIV_STAGE3_DCOMP_SPIKE_CHILD";
const CHILD_ENV_VALUE: &str = "stage3";
const PHASE_TIMEOUT: Duration = Duration::from_secs(3);
const GPU_TIMEOUT: Duration = Duration::from_secs(10);
const PROCESS_TIMEOUT: Duration = Duration::from_secs(30);
const RENDER_STALL_COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const PARENT_POLL_INTERVAL: Duration = Duration::from_millis(5);
const SURFACE_WIDTH: u32 = 320;
const SURFACE_HEIGHT: u32 = 180;
const WM_STAGE3_PING: u32 = WM_APP + 0x271;
const WM_STAGE3_RESIZE: u32 = WM_APP + 0x272;
const WM_STAGE3_THREAD_BARRIER: u32 = WM_APP + 0x273;

#[derive(Debug)]
struct HarnessFailure {
    error: WindowsHarnessError,
    detail: String,
}

impl HarnessFailure {
    fn timeout(phase: WindowsHarnessPhase, limit: Duration) -> Self {
        Self {
            error: WindowsHarnessError::Timeout {
                timeout: WindowsHarnessTimeout {
                    phase,
                    limit_millis: limit.as_millis() as u64,
                },
            },
            detail: String::new(),
        }
    }

    fn backend(
        operation: WindowBackendOperation,
        context: impl Into<String>,
        error: windows::core::Error,
    ) -> Self {
        let code = i64::from(error.code().0);
        Self {
            error: WindowsHarnessError::Backend { operation, code },
            detail: format!(
                "{}: {error:?} (HRESULT=0x{:08X})",
                context.into(),
                code as u32
            ),
        }
    }

    fn backend_code(
        operation: WindowBackendOperation,
        context: impl Into<String>,
        code: i64,
    ) -> Self {
        Self {
            error: WindowsHarnessError::Backend { operation, code },
            detail: context.into(),
        }
    }

    fn disconnected(channel: WindowHostChannel, detail: impl Into<String>) -> Self {
        Self {
            error: WindowsHarnessError::ChannelDisconnected { channel },
            detail: detail.into(),
        }
    }

    fn invariant(invariant: WindowsHarnessInvariant, detail: impl Into<String>) -> Self {
        Self {
            error: WindowsHarnessError::InvariantViolation { invariant },
            detail: detail.into(),
        }
    }

    fn unexpected(
        expected: WindowsHarnessPhase,
        actual: WindowsHarnessPhase,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            error: WindowsHarnessError::UnexpectedEvent { expected, actual },
            detail: detail.into(),
        }
    }
}

impl fmt::Display for HarnessFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.detail.is_empty() {
            self.error.fmt(f)
        } else {
            write!(f, "{}: {}", self.error, self.detail)
        }
    }
}

impl std::error::Error for HarnessFailure {}

#[derive(Clone, Debug)]
struct RenderEnvironment {
    thread_id: u32,
    adapter_name: String,
    vendor_id: u32,
    device_id: u32,
    dedicated_video_memory: usize,
    feature_level: u32,
    driver_version: Option<String>,
    driver_version_source: String,
    driver_version_raw: Option<u64>,
}

#[derive(Debug)]
enum ParentCommand {
    Destroy { id: u32 },
    Shutdown,
}

#[derive(Debug)]
enum ParentEvent {
    Ready {
        thread_id: u32,
        windows: [(u32, u64); 2],
    },
    Destroyed {
        id: u32,
        elapsed: Duration,
        result: Result<(), i64>,
    },
    Failed(HarnessFailure),
    Exited,
}

#[derive(Debug)]
enum PumpEvent {
    Ready {
        thread_id: u32,
        windows: [(u32, u64); 2],
    },
    Pong {
        id: u32,
    },
    Resized {
        id: u32,
        width: i32,
        height: i32,
        result: Result<(), i64>,
    },
    CloseCompleted {
        id: u32,
        result: Result<(), i64>,
    },
    Barrier {
        token: u32,
    },
    Destroyed {
        id: u32,
    },
    Failed(HarnessFailure),
    Exited,
}

#[derive(Debug)]
enum RenderCommand {
    AttachPresentStall {
        case_id: u32,
        hwnd: u64,
        color: [f32; 4],
    },
    Resume {
        case_id: u32,
    },
    Shutdown,
}

#[derive(Debug)]
enum RenderEvent {
    Ready(RenderEnvironment),
    Presented {
        case_id: u32,
        attach_elapsed: Duration,
        present_elapsed: Duration,
    },
    StallEntered {
        case_id: u32,
    },
    Detached {
        case_id: u32,
        stall_elapsed: Duration,
        detach_elapsed: Duration,
    },
    Failed(HarnessFailure),
    Exited,
}

fn log_line(message: impl AsRef<str>) {
    println!("[native-window-stage3] {}", message.as_ref());
    let _ = std::io::stdout().flush();
}

fn elapsed_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn recv_bounded<T>(
    receiver: &Receiver<T>,
    phase: WindowsHarnessPhase,
    limit: Duration,
    channel: WindowHostChannel,
) -> Result<T, HarnessFailure> {
    match receiver.recv_timeout(limit) {
        Ok(event) => Ok(event),
        Err(RecvTimeoutError::Timeout) => Err(HarnessFailure::timeout(phase, limit)),
        Err(RecvTimeoutError::Disconnected) => Err(HarnessFailure::disconnected(
            channel,
            format!("channel disconnected while waiting for {phase:?}"),
        )),
    }
}

fn send_bounded<T>(
    sender: &Sender<T>,
    value: T,
    channel: WindowHostChannel,
    detail: &'static str,
) -> Result<(), HarnessFailure> {
    sender
        .send(value)
        .map_err(|_| HarnessFailure::disconnected(channel, detail))
}

fn os_environment() -> String {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let Ok(key) = hklm.open_subkey("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion") else {
        return "Windows registry version unavailable".to_string();
    };
    let product = key
        .get_value::<String, _>("ProductName")
        .unwrap_or_else(|_| "Windows".to_string());
    let display = key
        .get_value::<String, _>("DisplayVersion")
        .or_else(|_| key.get_value::<String, _>("ReleaseId"))
        .unwrap_or_else(|_| "unknown".to_string());
    let build = key
        .get_value::<String, _>("CurrentBuildNumber")
        .unwrap_or_else(|_| "unknown".to_string());
    let ubr = key.get_value::<u32, _>("UBR").ok();
    let build_lab = key.get_value::<String, _>("BuildLabEx").ok();
    format!(
        "{product}, display={display}, build={build}{}, build_lab={}",
        ubr.map(|value| format!(".{value}")).unwrap_or_default(),
        build_lab.as_deref().unwrap_or("unknown")
    )
}

fn decode_driver_version(raw: u64) -> String {
    format!(
        "{}.{}.{}.{}",
        (raw >> 48) & 0xffff,
        (raw >> 32) & 0xffff,
        (raw >> 16) & 0xffff,
        raw & 0xffff
    )
}

fn registry_driver_version(adapter_name: &str, vendor_id: u32, device_id: u32) -> Option<String> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let video = hklm
        .open_subkey("SYSTEM\\CurrentControlSet\\Control\\Video")
        .ok()?;
    let pci_match = format!("ven_{vendor_id:04x}&dev_{device_id:04x}");
    for key_name in video.enum_keys().flatten() {
        let Ok(adapter) = video.open_subkey(format!("{key_name}\\0000")) else {
            continue;
        };
        let description = adapter
            .get_value::<String, _>("DriverDesc")
            .unwrap_or_default();
        let matching_device = adapter
            .get_value::<String, _>("MatchingDeviceId")
            .unwrap_or_default();
        if !description.eq_ignore_ascii_case(adapter_name)
            && !matching_device.to_ascii_lowercase().contains(&pci_match)
        {
            continue;
        }
        if let Ok(version) = adapter.get_value::<String, _>("DriverVersion") {
            return Some(version);
        }
    }
    None
}

fn hwnd(raw: u64) -> HWND {
    HWND(raw as *mut c_void)
}

fn raw_hwnd(value: HWND) -> u64 {
    value.0 as usize as u64
}

fn join_after_exit(handle: JoinHandle<()>, label: &str) -> Result<(), HarnessFailure> {
    handle.join().map_err(|_| {
        HarnessFailure::invariant(
            WindowsHarnessInvariant::TestProcessMustExitWithinDeadline,
            format!("{label} panicked after reporting exit"),
        )
    })
}

fn current_thread_id_for_window(raw: u64) -> u32 {
    unsafe { GetWindowThreadProcessId(hwnd(raw), None) }
}

fn window_exists(raw: u64) -> bool {
    unsafe { IsWindow(Some(hwnd(raw))).as_bool() }
}

fn drain_pipe<R: Read + Send + 'static>(mut pipe: R) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        bytes
    })
}

fn emit_child_output(stdout: &[u8], stderr: &[u8]) {
    if !stdout.is_empty() {
        print!("{}", String::from_utf8_lossy(stdout));
    }
    if !stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(stderr));
    }
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
}

fn run_in_watchdog_process() -> Result<(), HarnessFailure> {
    let executable = std::env::current_exe().map_err(|error| {
        HarnessFailure::invariant(
            WindowsHarnessInvariant::TestProcessMustExitWithinDeadline,
            format!("resolve current test executable: {error}"),
        )
    })?;
    let mut child = Command::new(executable)
        .arg("--exact")
        .arg(TEST_NAME)
        .arg("--ignored")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(CHILD_ENV, CHILD_ENV_VALUE)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            HarnessFailure::invariant(
                WindowsHarnessInvariant::TestProcessMustExitWithinDeadline,
                format!("spawn bounded child test process: {error}"),
            )
        })?;
    let stdout_reader = drain_pipe(child.stdout.take().expect("child stdout must be piped"));
    let stderr_reader = drain_pipe(child.stderr.take().expect("child stderr must be piped"));
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| {
            HarnessFailure::invariant(
                WindowsHarnessInvariant::TestProcessMustExitWithinDeadline,
                format!("poll child test process: {error}"),
            )
        })? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(POLL_INTERVAL);
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    emit_child_output(&stdout, &stderr);
    match status {
        Some(status) if status.success() => {
            log_line(format!(
                "watchdog: child process exited successfully within {} ms",
                PROCESS_TIMEOUT.as_millis()
            ));
            Ok(())
        }
        Some(status) => Err(HarnessFailure::invariant(
            WindowsHarnessInvariant::TestProcessMustExitWithinDeadline,
            format!("child test process exited with {status}"),
        )),
        None => Err(HarnessFailure::timeout(
            WindowsHarnessPhase::ThreadJoin,
            PROCESS_TIMEOUT,
        )),
    }
}

unsafe extern "system" fn parent_wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn module_instance() -> Result<HINSTANCE, HarnessFailure> {
    let module = unsafe { GetModuleHandleW(None) }.map_err(|error| {
        HarnessFailure::backend(
            WindowBackendOperation::CreateWindow,
            "GetModuleHandleW for disposable Stage 3 class",
            error,
        )
    })?;
    Ok(HINSTANCE(module.0))
}

fn register_parent_class(instance: HINSTANCE) -> Result<(), HarnessFailure> {
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(parent_wnd_proc),
        hInstance: instance,
        lpszClassName: w!("mIVStage3DisposableParent"),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(1410) {
            return Err(HarnessFailure::backend_code(
                WindowBackendOperation::CreateWindow,
                format!("RegisterClassW parent: {error:?}"),
                i64::from(error.raw_os_error().unwrap_or_default()),
            ));
        }
    }
    Ok(())
}

fn create_parent_window(instance: HINSTANCE) -> Result<u64, HarnessFailure> {
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("mIVStage3DisposableParent"),
            w!("mIV Stage 3 Disposable Parent"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN | WS_VISIBLE,
            -32_000,
            -32_000,
            SURFACE_WIDTH as i32,
            SURFACE_HEIGHT as i32,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| {
        HarnessFailure::backend(
            WindowBackendOperation::CreateWindow,
            "CreateWindowExW disposable parent",
            error,
        )
    })?;
    Ok(raw_hwnd(window))
}

fn destroy_window_on_owner_thread(raw: u64) -> Result<(), i64> {
    if !window_exists(raw) {
        return Ok(());
    }
    unsafe { DestroyWindow(hwnd(raw)) }.map_err(|error| i64::from(error.code().0))
}

fn pump_pending_messages() {
    unsafe {
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn run_parent_thread(
    command_rx: Receiver<ParentCommand>,
    event_tx: Sender<ParentEvent>,
) -> Result<(), HarnessFailure> {
    let instance = module_instance()?;
    register_parent_class(instance)?;
    let first = create_parent_window(instance)?;
    let second = match create_parent_window(instance) {
        Ok(window) => window,
        Err(error) => {
            let _ = destroy_window_on_owner_thread(first);
            return Err(error);
        }
    };
    let windows = [(1, first), (2, second)];
    event_tx
        .send(ParentEvent::Ready {
            thread_id: unsafe { GetCurrentThreadId() },
            windows,
        })
        .map_err(|_| {
            HarnessFailure::disconnected(
                WindowHostChannel::PumpToApp,
                "send disposable parent readiness",
            )
        })?;

    let mut shutdown = false;
    while !shutdown {
        pump_pending_messages();
        match command_rx.recv_timeout(PARENT_POLL_INTERVAL) {
            Ok(ParentCommand::Destroy { id }) => {
                let Some((_, raw)) = windows.iter().find(|(window_id, _)| *window_id == id) else {
                    return Err(HarnessFailure::invariant(
                        WindowsHarnessInvariant::ParentDestroyMustComplete,
                        format!("unknown parent id {id}"),
                    ));
                };
                let started = Instant::now();
                let result = destroy_window_on_owner_thread(*raw);
                let elapsed = started.elapsed();
                if event_tx
                    .send(ParentEvent::Destroyed {
                        id,
                        elapsed,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
            Ok(ParentCommand::Shutdown) => shutdown = true,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => shutdown = true,
        }
    }
    for (_, raw) in windows {
        let _ = destroy_window_on_owner_thread(raw);
    }
    let _ = event_tx.send(ParentEvent::Exited);
    Ok(())
}

fn spawn_parent_thread(
    command_rx: Receiver<ParentCommand>,
    event_tx: Sender<ParentEvent>,
) -> Result<JoinHandle<()>, HarnessFailure> {
    thread::Builder::new()
        .name("native-window-stage3-parent".to_string())
        .spawn(move || {
            if let Err(error) = run_parent_thread(command_rx, event_tx.clone()) {
                let _ = event_tx.send(ParentEvent::Failed(error));
            }
        })
        .map_err(|error| {
            HarnessFailure::invariant(
                WindowsHarnessInvariant::TestProcessMustExitWithinDeadline,
                format!("spawn disposable parent thread: {error}"),
            )
        })
}

struct PumpWindowContext {
    id: u32,
    event_tx: Sender<PumpEvent>,
    notify_destroyed_on_nc: Cell<bool>,
}

unsafe fn pump_window_context(hwnd: HWND) -> Option<&'static PumpWindowContext> {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const PumpWindowContext;
    unsafe { pointer.as_ref() }
}

unsafe extern "system" fn pump_wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCCREATE => {
            let create = lparam.0 as *const CREATESTRUCTW;
            if !create.is_null() {
                let context = unsafe { (*create).lpCreateParams } as *mut PumpWindowContext;
                if !context.is_null() {
                    unsafe {
                        let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, context as isize);
                    }
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_STAGE3_PING => {
            if let Some(context) = unsafe { pump_window_context(hwnd) } {
                let _ = context.event_tx.send(PumpEvent::Pong { id: context.id });
            }
            LRESULT(0)
        }
        WM_STAGE3_RESIZE => {
            if let Some(context) = unsafe { pump_window_context(hwnd) } {
                let requested_width = wparam.0.max(1) as i32;
                let requested_height = lparam.0.max(1) as i32;
                let result = unsafe {
                    SetWindowPos(
                        hwnd,
                        None,
                        0,
                        0,
                        requested_width,
                        requested_height,
                        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                };
                let mut rect = RECT::default();
                let rect_result = unsafe { GetClientRect(hwnd, &mut rect) };
                let result = result
                    .and(rect_result)
                    .map_err(|error| i64::from(error.code().0));
                let _ = context.event_tx.send(PumpEvent::Resized {
                    id: context.id,
                    width: rect.right - rect.left,
                    height: rect.bottom - rect.top,
                    result,
                });
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let completion = unsafe { pump_window_context(hwnd) }.map(|context| {
                context.notify_destroyed_on_nc.set(false);
                (context.id, context.event_tx.clone())
            });
            let result = unsafe { DestroyWindow(hwnd) }.map_err(|error| i64::from(error.code().0));
            if let Some((id, event_tx)) = completion {
                let _ = event_tx.send(PumpEvent::CloseCompleted { id, result });
            }
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        WM_NCDESTROY => {
            let pointer =
                unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut PumpWindowContext;
            let destroyed_event = unsafe { pointer.as_ref() }.and_then(|context| {
                context
                    .notify_destroyed_on_nc
                    .get()
                    .then(|| (context.id, context.event_tx.clone()))
            });
            unsafe {
                let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            let result = unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            if let Some((id, event_tx)) = destroyed_event {
                let _ = event_tx.send(PumpEvent::Destroyed { id });
            }
            if !pointer.is_null() {
                unsafe {
                    drop(Box::from_raw(pointer));
                }
            }
            result
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn register_pump_class(instance: HINSTANCE) -> Result<(), HarnessFailure> {
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(pump_wnd_proc),
        hInstance: instance,
        lpszClassName: w!("mIVStage3DisposablePumpChild"),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(1410) {
            return Err(HarnessFailure::backend_code(
                WindowBackendOperation::CreateWindow,
                format!("RegisterClassW pump child: {error:?}"),
                i64::from(error.raw_os_error().unwrap_or_default()),
            ));
        }
    }
    Ok(())
}

fn create_pump_child(
    instance: HINSTANCE,
    id: u32,
    parent: u64,
    event_tx: Sender<PumpEvent>,
) -> Result<u64, HarnessFailure> {
    let context = Box::new(PumpWindowContext {
        id,
        event_tx,
        notify_destroyed_on_nc: Cell::new(true),
    });
    let context_pointer = Box::into_raw(context);
    let result = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("mIVStage3DisposablePumpChild"),
            w!("mIV Stage 3 Disposable Pump Child"),
            WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN | WS_VISIBLE,
            0,
            0,
            SURFACE_WIDTH as i32,
            SURFACE_HEIGHT as i32,
            Some(hwnd(parent)),
            None,
            Some(instance),
            Some(context_pointer.cast()),
        )
    };
    match result {
        Ok(window) => Ok(raw_hwnd(window)),
        Err(error) => {
            unsafe {
                drop(Box::from_raw(context_pointer));
            }
            Err(HarnessFailure::backend(
                WindowBackendOperation::CreateWindow,
                format!("CreateWindowExW pump child id={id}"),
                error,
            ))
        }
    }
}

fn run_pump_thread(
    parents: [(u32, u64); 2],
    event_tx: Sender<PumpEvent>,
) -> Result<(), HarnessFailure> {
    let instance = module_instance()?;
    register_pump_class(instance)?;
    let first = create_pump_child(instance, parents[0].0, parents[0].1, event_tx.clone())?;
    let second = match create_pump_child(instance, parents[1].0, parents[1].1, event_tx.clone()) {
        Ok(window) => window,
        Err(error) => {
            let _ = destroy_window_on_owner_thread(first);
            return Err(error);
        }
    };
    let windows = [(parents[0].0, first), (parents[1].0, second)];
    event_tx
        .send(PumpEvent::Ready {
            thread_id: unsafe { GetCurrentThreadId() },
            windows,
        })
        .map_err(|_| {
            HarnessFailure::disconnected(
                WindowHostChannel::PumpToApp,
                "send disposable pump readiness",
            )
        })?;

    unsafe {
        let mut message = MSG::default();
        loop {
            let result = GetMessageW(&mut message, None, 0, 0);
            if result.0 == -1 {
                let error = windows::core::Error::from_win32();
                for (_, raw) in windows {
                    let _ = destroy_window_on_owner_thread(raw);
                }
                return Err(HarnessFailure::backend(
                    WindowBackendOperation::PumpMessage,
                    "GetMessageW disposable pump",
                    error,
                ));
            }
            if !result.as_bool() {
                break;
            }
            if message.hwnd.0.is_null() && message.message == WM_STAGE3_THREAD_BARRIER {
                let _ = event_tx.send(PumpEvent::Barrier {
                    token: message.wParam.0 as u32,
                });
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    for (_, raw) in windows {
        let _ = destroy_window_on_owner_thread(raw);
    }
    let _ = event_tx.send(PumpEvent::Exited);
    Ok(())
}

fn spawn_pump_thread(
    parents: [(u32, u64); 2],
    event_tx: Sender<PumpEvent>,
) -> Result<JoinHandle<()>, HarnessFailure> {
    thread::Builder::new()
        .name("native-window-stage3-pump".to_string())
        .spawn(move || {
            if let Err(error) = run_pump_thread(parents, event_tx.clone()) {
                let _ = event_tx.send(PumpEvent::Failed(error));
            }
        })
        .map_err(|error| {
            HarnessFailure::invariant(
                WindowsHarnessInvariant::TestProcessMustExitWithinDeadline,
                format!("spawn disposable pump thread: {error}"),
            )
        })
}

fn post_window_message(
    raw: u64,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    context: &'static str,
) -> Result<(), HarnessFailure> {
    unsafe { PostMessageW(Some(hwnd(raw)), message, wparam, lparam) }.map_err(|error| {
        HarnessFailure::backend(WindowBackendOperation::PumpMessage, context, error)
    })
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self, HarnessFailure> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|error| {
                HarnessFailure::backend(
                    WindowBackendOperation::AttachTarget,
                    "CoInitializeEx render thread",
                    error,
                )
            })?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct AttachedComposition {
    target: IDCompositionTarget,
    _root: IDCompositionVisual,
    _content: IDCompositionVisual,
    _swap_chain: IDXGISwapChain1,
    _backbuffer: ID3D11Texture2D,
    _render_target: ID3D11RenderTargetView,
}

struct RenderBackend {
    _apartment: ComApartment,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    factory: IDXGIFactory2,
    dcomp: IDCompositionDevice,
    environment: RenderEnvironment,
}

impl RenderBackend {
    fn new() -> Result<Self, HarnessFailure> {
        let apartment = ComApartment::initialize()?;
        let mut device = None;
        let mut context = None;
        let mut feature_level = D3D_FEATURE_LEVEL::default();
        let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(D3D11_CREATE_DEVICE_BGRA_SUPPORT.0),
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut feature_level),
                Some(&mut context),
            )
        }
        .map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::AttachTarget,
                "D3D11CreateDevice hardware adapter",
                error,
            )
        })?;
        let device = device.ok_or_else(|| {
            HarnessFailure::invariant(
                WindowsHarnessInvariant::TestProcessMustExitWithinDeadline,
                "D3D11CreateDevice returned a null device",
            )
        })?;
        let context = context.ok_or_else(|| {
            HarnessFailure::invariant(
                WindowsHarnessInvariant::TestProcessMustExitWithinDeadline,
                "D3D11CreateDevice returned a null immediate context",
            )
        })?;
        let dxgi_device: IDXGIDevice = device.cast().map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::AttachTarget,
                "cast ID3D11Device to IDXGIDevice",
                error,
            )
        })?;
        let adapter = unsafe { dxgi_device.GetAdapter() }.map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::AttachTarget,
                "IDXGIDevice::GetAdapter",
                error,
            )
        })?;
        let adapter1: IDXGIAdapter1 = adapter.cast().map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::AttachTarget,
                "cast IDXGIAdapter to IDXGIAdapter1",
                error,
            )
        })?;
        let description = unsafe { adapter1.GetDesc1() }.map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::AttachTarget,
                "IDXGIAdapter1::GetDesc1",
                error,
            )
        })?;
        let name_len = description
            .Description
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(description.Description.len());
        let adapter_name = String::from_utf16_lossy(&description.Description[..name_len]);
        let driver_version_raw = unsafe { adapter.CheckInterfaceSupport(&ID3D11Device::IID) }
            .ok()
            .map(|value| value as u64);
        let (driver_version, driver_version_source) = if let Some(raw) = driver_version_raw {
            (
                Some(decode_driver_version(raw)),
                "dxgi_check_interface_support".to_string(),
            )
        } else if let Some(version) =
            registry_driver_version(&adapter_name, description.VendorId, description.DeviceId)
        {
            (Some(version), "registry_control_video".to_string())
        } else {
            (None, "unavailable".to_string())
        };
        let factory: IDXGIFactory2 = unsafe { adapter.GetParent() }.map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::AttachTarget,
                "IDXGIAdapter::GetParent IDXGIFactory2",
                error,
            )
        })?;
        let dcomp: IDCompositionDevice = unsafe { DCompositionCreateDevice(&dxgi_device) }
            .map_err(|error| {
                HarnessFailure::backend(
                    WindowBackendOperation::AttachTarget,
                    "DCompositionCreateDevice",
                    error,
                )
            })?;
        let environment = RenderEnvironment {
            thread_id: unsafe { GetCurrentThreadId() },
            adapter_name,
            vendor_id: description.VendorId,
            device_id: description.DeviceId,
            dedicated_video_memory: description.DedicatedVideoMemory,
            feature_level: feature_level.0 as u32,
            driver_version,
            driver_version_source,
            driver_version_raw,
        };
        Ok(Self {
            _apartment: apartment,
            device,
            context,
            factory,
            dcomp,
            environment,
        })
    }

    fn attach_and_present(
        &self,
        raw: u64,
        color: [f32; 4],
    ) -> Result<(AttachedComposition, Duration, Duration), HarnessFailure> {
        let attach_started = Instant::now();
        let target =
            unsafe { self.dcomp.CreateTargetForHwnd(hwnd(raw), true) }.map_err(|error| {
                HarnessFailure::backend(
                    WindowBackendOperation::AttachTarget,
                    "IDCompositionDevice::CreateTargetForHwnd on pump-owned child",
                    error,
                )
            })?;
        let descriptor = DXGI_SWAP_CHAIN_DESC1 {
            Width: SURFACE_WIDTH,
            Height: SURFACE_HEIGHT,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: DXGI_SWAP_CHAIN_FLAG(0).0 as u32,
        };
        let swap_chain = unsafe {
            self.factory.CreateSwapChainForComposition(
                &self.device,
                &descriptor,
                None::<&IDXGIOutput>,
            )
        }
        .map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::AttachTarget,
                "IDXGIFactory2::CreateSwapChainForComposition",
                error,
            )
        })?;
        let root = unsafe { self.dcomp.CreateVisual() }.map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::AttachTarget,
                "IDCompositionDevice::CreateVisual root",
                error,
            )
        })?;
        let content = unsafe { self.dcomp.CreateVisual() }.map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::AttachTarget,
                "IDCompositionDevice::CreateVisual content",
                error,
            )
        })?;
        unsafe { content.SetContent(&swap_chain) }.map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::AttachTarget,
                "IDCompositionVisual::SetContent swap chain",
                error,
            )
        })?;
        unsafe { root.AddVisual(&content, false, None::<&IDCompositionVisual>) }.map_err(
            |error| {
                HarnessFailure::backend(
                    WindowBackendOperation::AttachTarget,
                    "IDCompositionVisual::AddVisual content",
                    error,
                )
            },
        )?;
        unsafe { target.SetRoot(&root) }.map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::AttachTarget,
                "IDCompositionTarget::SetRoot cross-thread HWND",
                error,
            )
        })?;
        unsafe { self.dcomp.Commit() }.map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::AttachTarget,
                "IDCompositionDevice::Commit attach",
                error,
            )
        })?;
        let attach_elapsed = attach_started.elapsed();

        let present_started = Instant::now();
        let backbuffer: ID3D11Texture2D = unsafe { swap_chain.GetBuffer(0) }.map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::PrimeTarget,
                "IDXGISwapChain::GetBuffer",
                error,
            )
        })?;
        let mut render_target = None;
        unsafe {
            self.device
                .CreateRenderTargetView(&backbuffer, None, Some(&mut render_target))
        }
        .map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::PrimeTarget,
                "ID3D11Device::CreateRenderTargetView",
                error,
            )
        })?;
        let render_target = render_target.ok_or_else(|| {
            HarnessFailure::invariant(
                WindowsHarnessInvariant::TestProcessMustExitWithinDeadline,
                "CreateRenderTargetView returned null",
            )
        })?;
        unsafe { self.context.ClearRenderTargetView(&render_target, &color) };
        unsafe { swap_chain.Present(0, DXGI_PRESENT(0)) }
            .ok()
            .map_err(|error| {
                HarnessFailure::backend(
                    WindowBackendOperation::PrimeTarget,
                    "IDXGISwapChain::Present",
                    error,
                )
            })?;
        unsafe { self.dcomp.WaitForCommitCompletion() }.map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::PrimeTarget,
                "IDCompositionDevice::WaitForCommitCompletion",
                error,
            )
        })?;
        unsafe { DwmFlush() }.map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::PrimeTarget,
                "DwmFlush after present",
                error,
            )
        })?;
        let present_elapsed = present_started.elapsed();
        Ok((
            AttachedComposition {
                target,
                _root: root,
                _content: content,
                _swap_chain: swap_chain,
                _backbuffer: backbuffer,
                _render_target: render_target,
            },
            attach_elapsed,
            present_elapsed,
        ))
    }

    fn detach(&self, attached: AttachedComposition) -> Result<Duration, HarnessFailure> {
        let started = Instant::now();
        unsafe { attached.target.SetRoot(None::<&IDCompositionVisual>) }.map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::DetachTarget,
                "IDCompositionTarget::SetRoot(None)",
                error,
            )
        })?;
        unsafe { self.dcomp.Commit() }.map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::DetachTarget,
                "IDCompositionDevice::Commit detach",
                error,
            )
        })?;
        unsafe { self.dcomp.WaitForCommitCompletion() }.map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::DetachTarget,
                "IDCompositionDevice::WaitForCommitCompletion detach",
                error,
            )
        })?;
        unsafe { DwmFlush() }.map_err(|error| {
            HarnessFailure::backend(
                WindowBackendOperation::DetachTarget,
                "DwmFlush after detach",
                error,
            )
        })?;
        drop(attached);
        Ok(started.elapsed())
    }
}

fn run_render_thread(
    command_rx: Receiver<RenderCommand>,
    event_tx: Sender<RenderEvent>,
) -> Result<(), HarnessFailure> {
    let backend = RenderBackend::new()?;
    event_tx
        .send(RenderEvent::Ready(backend.environment.clone()))
        .map_err(|_| {
            HarnessFailure::disconnected(WindowHostChannel::RenderToPump, "send render environment")
        })?;
    loop {
        let command = command_rx.recv().map_err(|_| {
            HarnessFailure::disconnected(
                WindowHostChannel::PumpToRender,
                "render command channel disconnected",
            )
        })?;
        match command {
            RenderCommand::AttachPresentStall {
                case_id,
                hwnd,
                color,
            } => {
                let (attached, attach_elapsed, present_elapsed) =
                    backend.attach_and_present(hwnd, color)?;
                event_tx
                    .send(RenderEvent::Presented {
                        case_id,
                        attach_elapsed,
                        present_elapsed,
                    })
                    .map_err(|_| {
                        HarnessFailure::disconnected(
                            WindowHostChannel::RenderToPump,
                            "send cross-thread present result",
                        )
                    })?;
                event_tx
                    .send(RenderEvent::StallEntered { case_id })
                    .map_err(|_| {
                        HarnessFailure::disconnected(
                            WindowHostChannel::RenderToPump,
                            "send render stall boundary",
                        )
                    })?;
                let stall_started = Instant::now();
                let resume = command_rx
                    .recv_timeout(RENDER_STALL_COMMAND_TIMEOUT)
                    .map_err(|error| match error {
                        RecvTimeoutError::Timeout => HarnessFailure::timeout(
                            WindowsHarnessPhase::RenderStall,
                            RENDER_STALL_COMMAND_TIMEOUT,
                        ),
                        RecvTimeoutError::Disconnected => HarnessFailure::disconnected(
                            WindowHostChannel::PumpToRender,
                            "render command channel disconnected during deliberate stall",
                        ),
                    })?;
                match resume {
                    RenderCommand::Resume {
                        case_id: resumed_case,
                    } if resumed_case == case_id => {}
                    RenderCommand::Resume {
                        case_id: resumed_case,
                    } => {
                        return Err(HarnessFailure::unexpected(
                            WindowsHarnessPhase::RenderStall,
                            WindowsHarnessPhase::RenderStall,
                            format!(
                                "resume case {resumed_case} did not match stalled case {case_id}"
                            ),
                        ));
                    }
                    RenderCommand::AttachPresentStall { .. } | RenderCommand::Shutdown => {
                        return Err(HarnessFailure::unexpected(
                            WindowsHarnessPhase::RenderStall,
                            WindowsHarnessPhase::RenderThreadStop,
                            "render received a non-resume command while stalled",
                        ));
                    }
                }
                let stall_elapsed = stall_started.elapsed();
                let detach_elapsed = backend.detach(attached)?;
                event_tx
                    .send(RenderEvent::Detached {
                        case_id,
                        stall_elapsed,
                        detach_elapsed,
                    })
                    .map_err(|_| {
                        HarnessFailure::disconnected(
                            WindowHostChannel::RenderToPump,
                            "send DComp detach result",
                        )
                    })?;
            }
            RenderCommand::Shutdown => {
                let _ = event_tx.send(RenderEvent::Exited);
                return Ok(());
            }
            RenderCommand::Resume { case_id } => {
                return Err(HarnessFailure::unexpected(
                    WindowsHarnessPhase::TargetAttach,
                    WindowsHarnessPhase::RenderStall,
                    format!("resume for non-stalled case {case_id}"),
                ));
            }
        }
    }
}

fn spawn_render_thread(
    command_rx: Receiver<RenderCommand>,
    event_tx: Sender<RenderEvent>,
) -> Result<JoinHandle<()>, HarnessFailure> {
    thread::Builder::new()
        .name("native-window-stage3-render".to_string())
        .spawn(move || {
            if let Err(error) = run_render_thread(command_rx, event_tx.clone()) {
                let _ = event_tx.send(RenderEvent::Failed(error));
            }
        })
        .map_err(|error| {
            HarnessFailure::invariant(
                WindowsHarnessInvariant::TestProcessMustExitWithinDeadline,
                format!("spawn disposable render thread: {error}"),
            )
        })
}

fn expect_parent_ready(
    receiver: &Receiver<ParentEvent>,
) -> Result<(u32, [(u32, u64); 2]), HarnessFailure> {
    match recv_bounded(
        receiver,
        WindowsHarnessPhase::WindowCreate,
        PHASE_TIMEOUT,
        WindowHostChannel::PumpToApp,
    )? {
        ParentEvent::Ready { thread_id, windows } => Ok((thread_id, windows)),
        ParentEvent::Failed(error) => Err(error),
        ParentEvent::Destroyed { .. } | ParentEvent::Exited => Err(HarnessFailure::unexpected(
            WindowsHarnessPhase::WindowCreate,
            WindowsHarnessPhase::ParentDestroy,
            "parent thread emitted lifecycle completion before readiness",
        )),
    }
}

fn expect_parent_destroyed(
    receiver: &Receiver<ParentEvent>,
    expected_id: u32,
) -> Result<Duration, HarnessFailure> {
    match recv_bounded(
        receiver,
        WindowsHarnessPhase::ParentDestroy,
        PHASE_TIMEOUT,
        WindowHostChannel::PumpToApp,
    )? {
        ParentEvent::Destroyed {
            id,
            elapsed,
            result,
        } if id == expected_id => {
            result.map_err(|code| {
                HarnessFailure::backend_code(
                    WindowBackendOperation::DestroyWindow,
                    format!("DestroyWindow parent id={id}"),
                    code,
                )
            })?;
            Ok(elapsed)
        }
        ParentEvent::Destroyed { id, .. } => Err(HarnessFailure::unexpected(
            WindowsHarnessPhase::ParentDestroy,
            WindowsHarnessPhase::ParentDestroy,
            format!("destroyed parent id {id}, expected {expected_id}"),
        )),
        ParentEvent::Failed(error) => Err(error),
        ParentEvent::Ready { .. } | ParentEvent::Exited => Err(HarnessFailure::unexpected(
            WindowsHarnessPhase::ParentDestroy,
            WindowsHarnessPhase::ThreadJoin,
            "unexpected parent event while waiting for destroy",
        )),
    }
}

fn expect_parent_exited(receiver: &Receiver<ParentEvent>) -> Result<(), HarnessFailure> {
    match recv_bounded(
        receiver,
        WindowsHarnessPhase::ThreadJoin,
        PHASE_TIMEOUT,
        WindowHostChannel::PumpToApp,
    )? {
        ParentEvent::Exited => Ok(()),
        ParentEvent::Failed(error) => Err(error),
        ParentEvent::Ready { .. } | ParentEvent::Destroyed { .. } => {
            Err(HarnessFailure::unexpected(
                WindowsHarnessPhase::ThreadJoin,
                WindowsHarnessPhase::ParentDestroy,
                "unexpected parent event while waiting for exit",
            ))
        }
    }
}

fn expect_pump_ready(
    receiver: &Receiver<PumpEvent>,
) -> Result<(u32, [(u32, u64); 2]), HarnessFailure> {
    match recv_bounded(
        receiver,
        WindowsHarnessPhase::WindowCreate,
        PHASE_TIMEOUT,
        WindowHostChannel::PumpToApp,
    )? {
        PumpEvent::Ready { thread_id, windows } => Ok((thread_id, windows)),
        PumpEvent::Failed(error) => Err(error),
        event => Err(HarnessFailure::unexpected(
            WindowsHarnessPhase::WindowCreate,
            pump_event_phase(&event),
            "pump emitted an operation event before readiness",
        )),
    }
}

fn pump_event_phase(event: &PumpEvent) -> WindowsHarnessPhase {
    match event {
        PumpEvent::Ready { .. } => WindowsHarnessPhase::WindowCreate,
        PumpEvent::Pong { .. } => WindowsHarnessPhase::PumpPing,
        PumpEvent::Resized { .. } => WindowsHarnessPhase::WindowResize,
        PumpEvent::CloseCompleted { .. } | PumpEvent::Destroyed { .. } => {
            WindowsHarnessPhase::WindowClose
        }
        PumpEvent::Barrier { .. } => WindowsHarnessPhase::ParentDestroy,
        PumpEvent::Failed(_) | PumpEvent::Exited => WindowsHarnessPhase::ThreadJoin,
    }
}

fn expect_pump_pong(
    receiver: &Receiver<PumpEvent>,
    expected_id: u32,
) -> Result<(), HarnessFailure> {
    match recv_bounded(
        receiver,
        WindowsHarnessPhase::PumpPing,
        PHASE_TIMEOUT,
        WindowHostChannel::PumpToApp,
    )? {
        PumpEvent::Pong { id } if id == expected_id => Ok(()),
        PumpEvent::Failed(error) => Err(error),
        event => Err(HarnessFailure::unexpected(
            WindowsHarnessPhase::PumpPing,
            pump_event_phase(&event),
            format!("expected pong for child {expected_id}, received {event:?}"),
        )),
    }
}

fn expect_pump_resized(
    receiver: &Receiver<PumpEvent>,
    expected_id: u32,
    expected_width: i32,
    expected_height: i32,
) -> Result<(), HarnessFailure> {
    match recv_bounded(
        receiver,
        WindowsHarnessPhase::WindowResize,
        PHASE_TIMEOUT,
        WindowHostChannel::PumpToApp,
    )? {
        PumpEvent::Resized {
            id,
            width,
            height,
            result,
        } if id == expected_id => {
            result.map_err(|code| {
                HarnessFailure::backend_code(
                    WindowBackendOperation::ResizeWindow,
                    format!("SetWindowPos pump child id={id}"),
                    code,
                )
            })?;
            if width != expected_width || height != expected_height {
                return Err(HarnessFailure::invariant(
                    WindowsHarnessInvariant::PumpMustProgressWhileRenderStalled,
                    format!(
                        "child {id} resized to {width}x{height}, expected {expected_width}x{expected_height}"
                    ),
                ));
            }
            Ok(())
        }
        PumpEvent::Failed(error) => Err(error),
        event => Err(HarnessFailure::unexpected(
            WindowsHarnessPhase::WindowResize,
            pump_event_phase(&event),
            format!("unexpected resize event: {event:?}"),
        )),
    }
}

fn expect_pump_close_completed(
    receiver: &Receiver<PumpEvent>,
    expected_id: u32,
) -> Result<(), HarnessFailure> {
    match recv_bounded(
        receiver,
        WindowsHarnessPhase::WindowClose,
        PHASE_TIMEOUT,
        WindowHostChannel::PumpToApp,
    )? {
        PumpEvent::CloseCompleted { id, result } if id == expected_id => {
            result.map_err(|code| {
                HarnessFailure::backend_code(
                    WindowBackendOperation::DestroyWindow,
                    format!("DestroyWindow from WM_CLOSE child id={id}"),
                    code,
                )
            })?;
            Ok(())
        }
        PumpEvent::Failed(error) => Err(error),
        event => Err(HarnessFailure::unexpected(
            WindowsHarnessPhase::WindowClose,
            pump_event_phase(&event),
            format!("expected close completion for child {expected_id}, received {event:?}"),
        )),
    }
}

fn expect_pump_destroyed(
    receiver: &Receiver<PumpEvent>,
    expected_id: u32,
    phase: WindowsHarnessPhase,
) -> Result<(), HarnessFailure> {
    match recv_bounded(receiver, phase, PHASE_TIMEOUT, WindowHostChannel::PumpToApp)? {
        PumpEvent::Destroyed { id } if id == expected_id => Ok(()),
        PumpEvent::Failed(error) => Err(error),
        event => Err(HarnessFailure::unexpected(
            phase,
            pump_event_phase(&event),
            format!("expected child {expected_id} destruction, received {event:?}"),
        )),
    }
}

fn expect_pump_barrier(
    receiver: &Receiver<PumpEvent>,
    expected_token: u32,
) -> Result<(), HarnessFailure> {
    match recv_bounded(
        receiver,
        WindowsHarnessPhase::ParentDestroy,
        PHASE_TIMEOUT,
        WindowHostChannel::PumpToApp,
    )? {
        PumpEvent::Barrier { token } if token == expected_token => Ok(()),
        PumpEvent::Failed(error) => Err(error),
        event => Err(HarnessFailure::unexpected(
            WindowsHarnessPhase::ParentDestroy,
            pump_event_phase(&event),
            format!("expected pump barrier {expected_token}, received {event:?}"),
        )),
    }
}

fn expect_pump_exited(receiver: &Receiver<PumpEvent>) -> Result<(), HarnessFailure> {
    match recv_bounded(
        receiver,
        WindowsHarnessPhase::ThreadJoin,
        PHASE_TIMEOUT,
        WindowHostChannel::PumpToApp,
    )? {
        PumpEvent::Exited => Ok(()),
        PumpEvent::Failed(error) => Err(error),
        event => Err(HarnessFailure::unexpected(
            WindowsHarnessPhase::ThreadJoin,
            pump_event_phase(&event),
            format!("unexpected pump event while waiting for exit: {event:?}"),
        )),
    }
}

fn expect_render_ready(
    receiver: &Receiver<RenderEvent>,
) -> Result<RenderEnvironment, HarnessFailure> {
    match recv_bounded(
        receiver,
        WindowsHarnessPhase::TargetAttach,
        GPU_TIMEOUT,
        WindowHostChannel::RenderToPump,
    )? {
        RenderEvent::Ready(environment) => Ok(environment),
        RenderEvent::Failed(error) => Err(error),
        event => Err(HarnessFailure::unexpected(
            WindowsHarnessPhase::TargetAttach,
            render_event_phase(&event),
            "render emitted an operation event before readiness",
        )),
    }
}

fn render_event_phase(event: &RenderEvent) -> WindowsHarnessPhase {
    match event {
        RenderEvent::Ready(_) => WindowsHarnessPhase::TargetAttach,
        RenderEvent::Presented { .. } => WindowsHarnessPhase::FirstPresent,
        RenderEvent::StallEntered { .. } => WindowsHarnessPhase::RenderStall,
        RenderEvent::Detached { .. } => WindowsHarnessPhase::TargetDetach,
        RenderEvent::Failed(_) | RenderEvent::Exited => WindowsHarnessPhase::ThreadJoin,
    }
}

fn expect_render_presented(
    receiver: &Receiver<RenderEvent>,
    expected_case: u32,
) -> Result<(Duration, Duration), HarnessFailure> {
    match recv_bounded(
        receiver,
        WindowsHarnessPhase::FirstPresent,
        GPU_TIMEOUT,
        WindowHostChannel::RenderToPump,
    )? {
        RenderEvent::Presented {
            case_id,
            attach_elapsed,
            present_elapsed,
        } if case_id == expected_case => Ok((attach_elapsed, present_elapsed)),
        RenderEvent::Failed(error) => Err(error),
        event => Err(HarnessFailure::unexpected(
            WindowsHarnessPhase::FirstPresent,
            render_event_phase(&event),
            format!("expected present for case {expected_case}, received {event:?}"),
        )),
    }
}

fn expect_render_stalled(
    receiver: &Receiver<RenderEvent>,
    expected_case: u32,
) -> Result<(), HarnessFailure> {
    match recv_bounded(
        receiver,
        WindowsHarnessPhase::RenderStall,
        PHASE_TIMEOUT,
        WindowHostChannel::RenderToPump,
    )? {
        RenderEvent::StallEntered { case_id } if case_id == expected_case => Ok(()),
        RenderEvent::Failed(error) => Err(error),
        event => Err(HarnessFailure::unexpected(
            WindowsHarnessPhase::RenderStall,
            render_event_phase(&event),
            format!("expected stall for case {expected_case}, received {event:?}"),
        )),
    }
}

fn expect_render_detached(
    receiver: &Receiver<RenderEvent>,
    expected_case: u32,
) -> Result<(Duration, Duration), HarnessFailure> {
    match recv_bounded(
        receiver,
        WindowsHarnessPhase::TargetDetach,
        GPU_TIMEOUT,
        WindowHostChannel::RenderToPump,
    )? {
        RenderEvent::Detached {
            case_id,
            stall_elapsed,
            detach_elapsed,
        } if case_id == expected_case => Ok((stall_elapsed, detach_elapsed)),
        RenderEvent::Failed(error) => Err(error),
        event => Err(HarnessFailure::unexpected(
            WindowsHarnessPhase::TargetDetach,
            render_event_phase(&event),
            format!("expected detach for case {expected_case}, received {event:?}"),
        )),
    }
}

fn expect_render_exited(receiver: &Receiver<RenderEvent>) -> Result<(), HarnessFailure> {
    match recv_bounded(
        receiver,
        WindowsHarnessPhase::RenderThreadStop,
        PHASE_TIMEOUT,
        WindowHostChannel::RenderToPump,
    )? {
        RenderEvent::Exited => Ok(()),
        RenderEvent::Failed(error) => Err(error),
        event => Err(HarnessFailure::unexpected(
            WindowsHarnessPhase::RenderThreadStop,
            render_event_phase(&event),
            format!("unexpected render event while waiting for exit: {event:?}"),
        )),
    }
}

fn find_window(windows: &[(u32, u64); 2], id: u32) -> Result<u64, HarnessFailure> {
    windows
        .iter()
        .find_map(|(window_id, raw)| (*window_id == id).then_some(*raw))
        .ok_or_else(|| {
            HarnessFailure::invariant(
                WindowsHarnessInvariant::WindowOwnerMustMatchPumpThread,
                format!("missing disposable window id {id}"),
            )
        })
}

fn run_stage3_harness() -> Result<(), HarnessFailure> {
    log_line(format!("OS: {}", os_environment()));
    log_line(format!(
        "deadlines: phase={}ms GPU={}ms process={}ms",
        PHASE_TIMEOUT.as_millis(),
        GPU_TIMEOUT.as_millis(),
        PROCESS_TIMEOUT.as_millis()
    ));

    let (parent_command_tx, parent_command_rx) = mpsc::channel();
    let (parent_event_tx, parent_event_rx) = mpsc::channel();
    let parent_handle = spawn_parent_thread(parent_command_rx, parent_event_tx)?;
    let (parent_thread_id, parent_windows) = expect_parent_ready(&parent_event_rx)?;

    let (pump_event_tx, pump_event_rx) = mpsc::channel();
    let pump_handle = spawn_pump_thread(parent_windows, pump_event_tx)?;
    let (pump_thread_id, pump_windows) = expect_pump_ready(&pump_event_rx)?;
    let parent_one = find_window(&parent_windows, 1)?;
    let parent_two = find_window(&parent_windows, 2)?;
    let child_one = find_window(&pump_windows, 1)?;
    let child_two = find_window(&pump_windows, 2)?;

    if parent_thread_id == pump_thread_id
        || current_thread_id_for_window(parent_one) != parent_thread_id
        || current_thread_id_for_window(parent_two) != parent_thread_id
        || current_thread_id_for_window(child_one) != pump_thread_id
        || current_thread_id_for_window(child_two) != pump_thread_id
    {
        return Err(HarnessFailure::invariant(
            WindowsHarnessInvariant::WindowOwnerMustMatchPumpThread,
            format!(
                "parent_thread={parent_thread_id}, pump_thread={pump_thread_id}, parent_tids=[{}, {}], child_tids=[{}, {}]",
                current_thread_id_for_window(parent_one),
                current_thread_id_for_window(parent_two),
                current_thread_id_for_window(child_one),
                current_thread_id_for_window(child_two)
            ),
        ));
    }
    log_line(format!(
        "HWND ownership: parent_thread={parent_thread_id} pump_thread={pump_thread_id} parents=[0x{parent_one:X},0x{parent_two:X}] pump_children=[0x{child_one:X},0x{child_two:X}]"
    ));

    let (render_command_tx, render_command_rx) = mpsc::channel();
    let (render_event_tx, render_event_rx) = mpsc::channel();
    let render_handle = spawn_render_thread(render_command_rx, render_event_tx)?;
    let render_environment = expect_render_ready(&render_event_rx)?;
    if render_environment.thread_id == pump_thread_id
        || render_environment.thread_id == parent_thread_id
    {
        return Err(HarnessFailure::invariant(
            WindowsHarnessInvariant::WindowOwnerMustMatchPumpThread,
            format!(
                "render thread {} was not distinct from parent/pump threads",
                render_environment.thread_id
            ),
        ));
    }
    log_line(format!(
        "DXGI adapter: name={} vendor=0x{:04X} device=0x{:04X} dedicated_vram={}MiB feature_level=0x{:X} driver={} driver_source={} driver_raw={} render_thread={}",
        render_environment.adapter_name,
        render_environment.vendor_id,
        render_environment.device_id,
        render_environment.dedicated_video_memory / (1024 * 1024),
        render_environment.feature_level,
        render_environment
            .driver_version
            .as_deref()
            .unwrap_or("unavailable"),
        render_environment.driver_version_source,
        render_environment
            .driver_version_raw
            .map(|value| format!("0x{value:016X}"))
            .unwrap_or_else(|| "unavailable".to_string()),
        render_environment.thread_id
    ));

    send_bounded(
        &render_command_tx,
        RenderCommand::AttachPresentStall {
            case_id: 1,
            hwnd: child_one,
            color: [0.10, 0.25, 0.85, 1.0],
        },
        WindowHostChannel::PumpToRender,
        "start close-case attach/present",
    )?;
    let (attach_one, present_one) = expect_render_presented(&render_event_rx, 1)?;
    expect_render_stalled(&render_event_rx, 1)?;
    log_line(format!(
        "case=close cross-thread attach={:.3}ms present+commit={:.3}ms; render deliberately stalled with DComp target live",
        elapsed_ms(attach_one),
        elapsed_ms(present_one)
    ));

    let ping_started = Instant::now();
    post_window_message(
        child_one,
        WM_STAGE3_PING,
        WPARAM(0),
        LPARAM(0),
        "PostMessage pump ping during render stall",
    )?;
    expect_pump_pong(&pump_event_rx, 1)?;
    let ping_elapsed = ping_started.elapsed();
    log_line(format!(
        "case=close pump_ping=PASS elapsed={:.3}ms (render stalled)",
        elapsed_ms(ping_elapsed)
    ));

    let resize_width = 384_i32;
    let resize_height = 216_i32;
    let resize_started = Instant::now();
    post_window_message(
        child_one,
        WM_STAGE3_RESIZE,
        WPARAM(resize_width as usize),
        LPARAM(resize_height as isize),
        "PostMessage pump resize during render stall",
    )?;
    expect_pump_resized(&pump_event_rx, 1, resize_width, resize_height)?;
    let resize_elapsed = resize_started.elapsed();
    log_line(format!(
        "case=close resize=PASS size={resize_width}x{resize_height} elapsed={:.3}ms (render stalled)",
        elapsed_ms(resize_elapsed)
    ));

    let close_started = Instant::now();
    post_window_message(
        child_one,
        WM_CLOSE,
        WPARAM(0),
        LPARAM(0),
        "PostMessage WM_CLOSE during render stall",
    )?;
    expect_pump_close_completed(&pump_event_rx, 1)?;
    let close_elapsed = close_started.elapsed();
    if window_exists(child_one) {
        return Err(HarnessFailure::invariant(
            WindowsHarnessInvariant::CloseMustNotWaitForRender,
            "pump reported WM_CLOSE completion but child HWND still exists",
        ));
    }
    log_line(format!(
        "case=close close=PASS elapsed={:.3}ms (render stalled)",
        elapsed_ms(close_elapsed)
    ));

    send_bounded(
        &render_command_tx,
        RenderCommand::Resume { case_id: 1 },
        WindowHostChannel::PumpToRender,
        "resume close-case render thread",
    )?;
    let (stall_one, detach_one) = expect_render_detached(&render_event_rx, 1)?;
    log_line(format!(
        "case=close detach=PASS stall_duration={:.3}ms detach+commit={:.3}ms",
        elapsed_ms(stall_one),
        elapsed_ms(detach_one)
    ));

    send_bounded(
        &render_command_tx,
        RenderCommand::AttachPresentStall {
            case_id: 2,
            hwnd: child_two,
            color: [0.15, 0.75, 0.25, 1.0],
        },
        WindowHostChannel::PumpToRender,
        "start parent-destroy-case attach/present",
    )?;
    let (attach_two, present_two) = expect_render_presented(&render_event_rx, 2)?;
    expect_render_stalled(&render_event_rx, 2)?;
    log_line(format!(
        "case=parent_destroy cross-thread attach={:.3}ms present+commit={:.3}ms; render deliberately stalled with DComp target live",
        elapsed_ms(attach_two),
        elapsed_ms(present_two)
    ));

    let second_ping_started = Instant::now();
    post_window_message(
        child_two,
        WM_STAGE3_PING,
        WPARAM(0),
        LPARAM(0),
        "PostMessage second pump ping during render stall",
    )?;
    expect_pump_pong(&pump_event_rx, 2)?;
    let second_ping_elapsed = second_ping_started.elapsed();
    log_line(format!(
        "case=parent_destroy pump_ping=PASS elapsed={:.3}ms (render stalled)",
        elapsed_ms(second_ping_elapsed)
    ));

    let parent_destroy_started = Instant::now();
    send_bounded(
        &parent_command_tx,
        ParentCommand::Destroy { id: 2 },
        WindowHostChannel::AppToPump,
        "request parent destroy while render is stalled",
    )?;
    let parent_destroy_call = expect_parent_destroyed(&parent_event_rx, 2)?;
    expect_pump_destroyed(&pump_event_rx, 2, WindowsHarnessPhase::ParentDestroy)?;
    unsafe {
        PostThreadMessageW(
            pump_thread_id,
            WM_STAGE3_THREAD_BARRIER,
            WPARAM(2),
            LPARAM(0),
        )
    }
    .map_err(|error| {
        HarnessFailure::backend(
            WindowBackendOperation::PumpMessage,
            "PostThreadMessageW parent-destroy completion barrier",
            error,
        )
    })?;
    expect_pump_barrier(&pump_event_rx, 2)?;
    let parent_destroy_end_to_end = parent_destroy_started.elapsed();
    if window_exists(parent_two) || window_exists(child_two) {
        return Err(HarnessFailure::invariant(
            WindowsHarnessInvariant::ParentDestroyMustComplete,
            format!(
                "parent/child survived destroy: parent_exists={} child_exists={}",
                window_exists(parent_two),
                window_exists(child_two)
            ),
        ));
    }
    log_line(format!(
        "case=parent_destroy parent_destroy=PASS DestroyWindow_call={:.3}ms child_destroy_notification_total={:.3}ms (render stalled)",
        elapsed_ms(parent_destroy_call),
        elapsed_ms(parent_destroy_end_to_end)
    ));

    send_bounded(
        &render_command_tx,
        RenderCommand::Resume { case_id: 2 },
        WindowHostChannel::PumpToRender,
        "resume parent-destroy-case render thread",
    )?;
    let (stall_two, detach_two) = expect_render_detached(&render_event_rx, 2)?;
    log_line(format!(
        "case=parent_destroy detach=PASS stall_duration={:.3}ms detach+commit={:.3}ms",
        elapsed_ms(stall_two),
        elapsed_ms(detach_two)
    ));

    send_bounded(
        &parent_command_tx,
        ParentCommand::Destroy { id: 1 },
        WindowHostChannel::AppToPump,
        "destroy first disposable parent during cleanup",
    )?;
    let cleanup_parent_elapsed = expect_parent_destroyed(&parent_event_rx, 1)?;
    log_line(format!(
        "cleanup parent_destroy=PASS elapsed={:.3}ms",
        elapsed_ms(cleanup_parent_elapsed)
    ));

    send_bounded(
        &render_command_tx,
        RenderCommand::Shutdown,
        WindowHostChannel::PumpToRender,
        "stop render thread",
    )?;
    expect_render_exited(&render_event_rx)?;
    join_after_exit(render_handle, "render thread")?;

    unsafe { PostThreadMessageW(pump_thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }.map_err(
        |error| {
            HarnessFailure::backend(
                WindowBackendOperation::PumpMessage,
                "PostThreadMessageW WM_QUIT to pump",
                error,
            )
        },
    )?;
    expect_pump_exited(&pump_event_rx)?;
    join_after_exit(pump_handle, "pump thread")?;

    send_bounded(
        &parent_command_tx,
        ParentCommand::Shutdown,
        WindowHostChannel::AppToPump,
        "stop disposable parent owner thread",
    )?;
    expect_parent_exited(&parent_event_rx)?;
    join_after_exit(parent_handle, "parent owner thread")?;

    for (label, raw) in [
        ("parent_one", parent_one),
        ("parent_two", parent_two),
        ("child_one", child_one),
        ("child_two", child_two),
    ] {
        if window_exists(raw) {
            return Err(HarnessFailure::invariant(
                WindowsHarnessInvariant::TestProcessMustExitWithinDeadline,
                format!("{label} HWND 0x{raw:X} remained after harness cleanup"),
            ));
        }
    }
    log_line(
        "visual pixel probe: NOT PERFORMED (Present, DComp commit completion, and DwmFlush were verified at API level)",
    );
    log_line(
        "RESULT: PASS cross-thread DirectComposition attach/present/detach succeeded; pump ping/resize/close/parent destroy all completed while render was stalled; all HWNDs destroyed",
    );
    Ok(())
}

#[test]
#[ignore = "Stage 3 hardware/driver gate; run explicitly with --ignored --exact --nocapture"]
fn cross_thread_dcomp_present_remains_pump_independent_when_render_stalls() {
    let result = if std::env::var(CHILD_ENV).as_deref() == Ok(CHILD_ENV_VALUE) {
        run_stage3_harness()
    } else {
        run_in_watchdog_process()
    };
    if let Err(error) = result {
        panic!("native video window-thread Stage 3 harness failed: {error}");
    }
}
