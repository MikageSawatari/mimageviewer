//! Win32 ownership adapter. HWNDs, sends and callbacks stay on one IPC worker;
//! its caller can stop waiting through cancellation without joining a native send.
//! Microsoft API contracts: WM_COPYDATA, SendMessageTimeoutW, and
//! MsgWaitForMultipleObjectsEx; callback buffers are copied before returning.

use super::*;
use std::cell::{Cell, RefCell};
use std::os::windows::io::AsRawHandle;
use std::sync::OnceLock;
use std::time::Instant;
use windows::Win32::Foundation::{
    ERROR_TIMEOUT, HANDLE, HWND, LPARAM, LRESULT, WAIT_FAILED, WAIT_TIMEOUT, WPARAM,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

const GET_MAJOR: usize = 0;
const GET_MINOR: usize = 1;
const GET_REVISION: usize = 2;
const IS_DB_LOADED: usize = 401;

/// Classify the failed send using Windows' observation at that failure, not
/// elapsed time alone. A timeout can also occur while the target is responsive.
pub(super) fn classify_send_failure(error: u32, hung: bool) -> EverythingError {
    if hung {
        EverythingError::NotResponding
    } else if error == ERROR_TIMEOUT.0 {
        EverythingError::Busy
    } else {
        EverythingError::SendFailed
    }
}

/// The search/prepare worker waits on two events, not the synchronous native send.
/// The detached IPC run owns its entire packet, HWND and callback lifetime until
/// its bounded send returns. Its caller thread stays alive through reply parsing
/// (Everything may watch that thread). No HWND or borrowed packet crosses threads.
pub(super) fn run_cancellable<T: Send + 'static>(
    cancel: &CancelHandle,
    run: impl FnOnce() -> Result<T, EverythingError> + Send + 'static,
) -> Result<T, EverythingError> {
    use std::os::windows::io::FromRawHandle;
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};

    cancelled(cancel)?;
    let completion = unsafe { CreateEventW(None, true, false, None) }
        .map_err(|e| EverythingError::Native(e.code().0 as u32))?;
    // SAFETY: sole ownership of the fresh event transfers to OwnedHandle. Both
    // wait and signal hold an Arc, even when cancellation drops the receiver.
    let completion =
        Arc::new(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(completion.0) });
    let signal = Arc::clone(&completion);
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let _ipc_run = std::thread::Builder::new()
        .name("everything-ipc".into())
        .spawn(move || {
            let result = run();
            let _ = tx.send(result);
            if let Err(error) = unsafe { SetEvent(HANDLE(signal.as_raw_handle())) } {
                crate::logger::log(format!("everything IPC completion signal failed: {error}"));
            }
        })
        .map_err(|e| EverythingError::Native(e.raw_os_error().unwrap_or(0) as u32))?;
    let handles = [
        HANDLE(cancel.inner.event.as_raw_handle()),
        HANDLE(completion.as_raw_handle()),
    ];
    // Four status sends (<=1s each), one 60s send+reply budget, 100ms cleanup.
    // Also bounds a rare IPC-worker panic/signal failure; no recovery wait/retry.
    let wait = unsafe { WaitForMultipleObjects(&handles, false, 65_000) };
    let wait_error = unsafe { windows::Win32::Foundation::GetLastError().0 };
    cancelled(cancel)?;
    if wait.0 == WAIT_OBJECT_0.0 + 1 {
        rx.try_recv().map_err(|_| EverythingError::Disconnected)?
    } else if wait == WAIT_TIMEOUT {
        Err(EverythingError::Timeout)
    } else {
        Err(EverythingError::Native(wait_error))
    }
}

// Opt-in, ignored test executable only; ordinary tests never discover Everything.
#[cfg(test)]
thread_local! { static PROBE_TRACE: Cell<bool> = const { Cell::new(false) }; }
macro_rules! probe_trace {
    ($($arg:tt)*) => {
        #[cfg(test)]
        PROBE_TRACE.with(|enabled| {
            if enabled.get() { eprintln!($($arg)*); }
        });
    };
}

struct CallbackState {
    active_id: Cell<Option<u32>>,
    packet: RefCell<Option<Result<EndpointReply, EverythingError>>>,
}

impl CallbackState {
    fn new() -> Self {
        Self {
            active_id: Cell::new(None),
            packet: RefCell::new(None),
        }
    }
}

unsafe extern "system" fn reply_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if message == WM_COPYDATA {
        // SAFETY: this HWND stores a stable Box pointer from creation until after
        // DestroyWindow returns. No reference to the state is held across a send/pump.
        let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const CallbackState;
        if state.is_null() || lp.0 == 0 {
            return LRESULT(0);
        }
        let state = unsafe { &*state };
        let cds = unsafe { &*(lp.0 as *const COPYDATASTRUCT) };
        probe_trace!(
            "reply WM_COPYDATA: hwnd={hwnd:?}, sender={wp:?}, id={}, bytes={}",
            cds.dwData,
            cds.cbData
        );
        let Some(active_id) = state.active_id.get() else {
            return LRESULT(0);
        };
        if cds.dwData != active_id as usize || state.packet.borrow().is_some() {
            return LRESULT(0);
        }
        let size = cds.cbData as usize;
        let packet = if size > MAX_REPLY_BYTES {
            Err(EverythingError::ReplyTooLarge)
        } else if size < REPLY_HEADER_BYTES || size % 2 != 0 || cds.lpData.is_null() {
            Err(EverythingError::Malformed("callback payload size"))
        } else {
            // SAFETY: WM_COPYDATA is system-marshalled and valid during this callback
            // only. Upper/lower bounds were checked before allocating or reading.
            let copy_started = crate::perf::is_enabled().then(Instant::now);
            let bytes =
                unsafe { std::slice::from_raw_parts(cds.lpData.cast::<u8>(), size) }.to_vec();
            if let Some(started) = copy_started {
                // The existing perf sink uses bounded nonblocking publication;
                // this callback performs no file I/O and holds no RefCell borrow.
                crate::perf::event(
                    "everything",
                    "reply_copy",
                    None,
                    u64::from(active_id),
                    &[
                        ("source", "everything".into()),
                        ("ms", (started.elapsed().as_secs_f64() * 1000.0).into()),
                        ("reply_bytes", size.into()),
                    ],
                );
            }
            Ok(EndpointReply {
                reply_id: active_id,
                bytes,
            })
        };
        *state.packet.borrow_mut() = Some(packet);
        return LRESULT(1);
    }
    unsafe { DefWindowProcW(hwnd, message, wp, lp) }
}

fn register_reply_class() -> Result<(), EverythingError> {
    static REGISTERED: OnceLock<Result<(), EverythingError>> = OnceLock::new();
    REGISTERED
        .get_or_init(|| {
            let module = unsafe { GetModuleHandleW(None) }
                .map_err(|e| EverythingError::Native(e.code().0 as u32))?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(reply_proc),
                hInstance: module.into(),
                lpszClassName: w!("mIV.Everything.Query2.Reply"),
                ..Default::default()
            };
            if unsafe { RegisterClassW(&class) } == 0 {
                return Err(EverythingError::Native(
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32,
                ));
            }
            Ok(())
        })
        .clone()
}

pub(super) struct WindowsEndpoint {
    target: HWND,
    reply: HWND,
    // Separate allocation: no alias to this state or RefCell borrow is held during
    // SendMessageTimeout/DispatchMessage, which may reenter reply_proc immediately.
    callback: Box<CallbackState>,
    started: Instant,
    // HWND ownership is thread-affine, so the endpoint must not be Send/Sync.
    _thread_affine: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl WindowsEndpoint {
    pub(super) fn discover() -> Result<Self, EverythingError> {
        probe_trace!("FindWindowW: class=EVERYTHING_TASKBAR_NOTIFICATION, title=NULL");
        let target = unsafe { FindWindowW(w!("EVERYTHING_TASKBAR_NOTIFICATION"), None) }
            .map_err(|_| EverythingError::Unavailable)?;
        probe_trace!("FindWindowW: target={target:?}");
        Self::for_window(target)
    }

    /// Injection for fake HWND tests. Never discovers the Everything class.
    fn for_window(target: HWND) -> Result<Self, EverythingError> {
        if target.is_invalid() || !unsafe { IsWindow(Some(target)) }.as_bool() {
            return Err(EverythingError::Unavailable);
        }
        if unsafe { GetWindowThreadProcessId(target, None) } == unsafe { GetCurrentThreadId() } {
            // SendMessageTimeout ignores the timeout for the same message queue.
            return Err(EverythingError::SendFailed);
        }
        register_reply_class()?;
        let module = unsafe { GetModuleHandleW(None) }
            .map_err(|e| EverythingError::Native(e.code().0 as u32))?;
        let callback = Box::new(CallbackState::new());
        let reply = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("mIV.Everything.Query2.Reply"),
                w!(""),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(module.into()),
                None,
            )
        }
        .map_err(|e| EverythingError::Native(e.code().0 as u32))?;
        unsafe {
            SetWindowLongPtrW(
                reply,
                GWLP_USERDATA,
                (&*callback as *const CallbackState) as isize,
            )
        };
        probe_trace!(
            "reply HWND created: {reply:?}, parent=HWND_MESSAGE, worker thread={}",
            unsafe { GetCurrentThreadId() }
        );
        Ok(Self {
            target,
            reply,
            callback,
            started: Instant::now(),
            _thread_affine: std::marker::PhantomData,
        })
    }

    fn send_message(
        &self,
        message: u32,
        wp: WPARAM,
        lp: LPARAM,
        timeout: Duration,
    ) -> Result<usize, EverythingError> {
        let flags = if message == WM_COPYDATA && timeout > STATUS_TIMEOUT {
            // A healthy 1.4 handler can stay busy without pumping for >5s.
            // Cancellation is handled by run_cancellable, not by declaring that
            // thread hung. Still bounded: never use NOTIMEOUTIFNOTHUNG or BLOCK.
            SMTO_ERRORONEXIT
        } else {
            SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT
        };
        self.send_message_with_flags(message, wp, lp, timeout, flags)
    }

    fn send_message_with_flags(
        &self,
        message: u32,
        wp: WPARAM,
        lp: LPARAM,
        timeout: Duration,
        flags: SEND_MESSAGE_TIMEOUT_FLAGS,
    ) -> Result<usize, EverythingError> {
        let mut response = 0usize;
        unsafe {
            windows::Win32::Foundation::SetLastError(windows::Win32::Foundation::WIN32_ERROR(0))
        };
        // No SMTO_BLOCK: sent-message callbacks must remain dispatchable during send.
        // No NOTIMEOUTIFNOTHUNG: each send remains bounded even when the peer pumps.
        let send_started = Instant::now();
        let sent = unsafe {
            SendMessageTimeoutW(
                self.target,
                message,
                wp,
                lp,
                flags,
                timeout.as_millis().clamp(1, u32::MAX as u128) as u32,
                Some(&mut response),
            )
        };
        let last_error = unsafe { windows::Win32::Foundation::GetLastError() };
        probe_trace!(
            "SendMessageTimeoutW: message={message:#x}, wp={}, sent={}, response={response}, last_error={}, timeout_ms={}",
            wp.0,
            sent.0,
            last_error.0,
            timeout.as_millis()
        );
        if sent.0 == 0 {
            // GetLastError was captured before this observation. No extra send,
            // wait or polling: status and query share this failure boundary.
            let hung = unsafe { IsHungAppWindow(self.target) }.as_bool();
            let failure = classify_send_failure(last_error.0, hung);
            crate::logger::log(format!(
                "everything IPC send failed: classification={failure:?}, message={message:#x}, status_code={}, win32_error={}, timeout_ms={}, elapsed_ms={:.3}",
                if message == WM_USER { wp.0 } else { 0 },
                last_error.0,
                timeout.as_millis(),
                send_started.elapsed().as_secs_f64() * 1000.0
            ));
            Err(failure)
        } else {
            Ok(response)
        }
    }

    fn status(&self, code: usize) -> Result<u32, EverythingError> {
        let response = self.send_message(WM_USER, WPARAM(code), LPARAM(0), STATUS_TIMEOUT)?;
        u32::try_from(response).map_err(|_| EverythingError::UnsupportedVersion)
    }
}

impl Drop for WindowsEndpoint {
    fn drop(&mut self) {
        self.callback.active_id.set(None);
        // Destroy synchronously on this worker; callback Box outlives the entire call.
        let _ = unsafe { DestroyWindow(self.reply) };
    }
}

impl EverythingEndpoint for WindowsEndpoint {
    fn version(&mut self, cancel: &CancelHandle) -> Result<EverythingVersion, EverythingError> {
        let major = self.status(GET_MAJOR)?;
        cancelled(cancel)?;
        let minor = self.status(GET_MINOR)?;
        cancelled(cancel)?;
        let revision = self.status(GET_REVISION)?;
        Ok(EverythingVersion {
            major,
            minor,
            revision,
        })
    }
    fn database_loaded(&mut self) -> Result<bool, EverythingError> {
        self.status(IS_DB_LOADED).map(|r| r != 0)
    }
    fn reply_hwnd(&self) -> u32 {
        // Microsoft explicitly permits truncating HWND to the significant low32 bits.
        self.reply.0 as usize as u32
    }
    fn activate(&mut self, reply_id: u32) {
        *self.callback.packet.borrow_mut() = None;
        self.callback.active_id.set(Some(reply_id));
    }
    fn deactivate(&mut self) {
        self.callback.active_id.set(None);
        *self.callback.packet.borrow_mut() = None;
    }
    fn send(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), EverythingError> {
        let cds = COPYDATASTRUCT {
            dwData: QUERY2W,
            cbData: u32::try_from(bytes.len()).map_err(|_| EverythingError::QueryTooLong)?,
            lpData: bytes.as_ptr().cast_mut().cast(),
        };
        probe_trace!(
            "query v2: COPYDATA id={}, native sizeof={} offsets=({}, {}, {}), packed header={}, packet={}",
            cds.dwData,
            std::mem::size_of::<COPYDATASTRUCT>(),
            std::mem::offset_of!(COPYDATASTRUCT, dwData),
            std::mem::offset_of!(COPYDATASTRUCT, cbData),
            std::mem::offset_of!(COPYDATASTRUCT, lpData),
            QUERY_HEADER_BYTES,
            bytes.len()
        );
        let accepted = self.send_message(
            WM_COPYDATA,
            WPARAM(self.reply.0 as usize),
            LPARAM((&cds as *const COPYDATASTRUCT) as isize),
            timeout,
        )?;
        if accepted == 0 {
            Err(EverythingError::Rejected)
        } else {
            Ok(())
        }
    }
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
    fn wait(
        &mut self,
        cancel: &CancelHandle,
        deadline: Duration,
    ) -> Result<EndpointReply, EverythingError> {
        probe_trace!(
            "message pump: PeekMessageW/DispatchMessageW + MsgWaitForMultipleObjectsEx, deadline={deadline:?}"
        );
        loop {
            cancelled(cancel)?;
            if self.elapsed() >= deadline {
                return Err(EverythingError::Timeout);
            }
            if let Some(packet) = self.callback.packet.borrow_mut().take() {
                return packet;
            }
            if !unsafe { IsWindow(Some(self.target)) }.as_bool() {
                return Err(EverythingError::Disconnected);
            }
            // Bound each pump batch so an unrelated posted-message stream cannot
            // starve deadline/cancellation checks or keep this worker alive forever.
            let mut message = MSG::default();
            for _ in 0..64 {
                if !unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                    break;
                }
                if message.message == WM_QUIT {
                    return Err(EverythingError::Disconnected);
                }
                unsafe { DispatchMessageW(&message) };
                if cancel.is_cancelled() || self.callback.packet.borrow().is_some() {
                    break;
                }
            }
            cancelled(cancel)?;
            if self.callback.packet.borrow().is_some() {
                continue;
            }
            let remaining = deadline.saturating_sub(self.elapsed());
            if remaining.is_zero() {
                return Err(EverythingError::Timeout);
            }
            let handles = [HANDLE(cancel.inner.event.as_raw_handle())];
            // Periodic IsWindow observation detects a peer disappearing with no reply.
            // This is an OS wait, not polling with sleeps or locks.
            let wait = unsafe {
                MsgWaitForMultipleObjectsEx(
                    Some(&handles),
                    remaining.as_millis().clamp(1, 1000) as u32,
                    QS_ALLINPUT,
                    MWMO_INPUTAVAILABLE,
                )
            };
            if wait == WAIT_FAILED {
                return Err(EverythingError::SendFailed);
            }
            if wait == WAIT_TIMEOUT {
                continue;
            }
            // Event readiness and sent-message callbacks are examined at loop top.
        }
    }
}

#[cfg(test)]
mod adapter_tests {
    use super::*;
    use std::sync::mpsc;

    const STOP: u32 = WM_APP + 291;

    // This test executable may start on Codex's isolated desktop. Bind only its
    // probe thread to the input desktop before creating any HWND. This does not
    // switch the visible desktop, launch mIV, or modify Everything's windows.
    // Win32 OpenInputDesktop/SetThreadDesktop/CloseDesktop documented contracts.
    #[link(name = "user32")]
    unsafe extern "system" {
        fn OpenInputDesktop(flags: u32, inherit: i32, access: u32) -> *mut std::ffi::c_void;
        fn SetThreadDesktop(desktop: *mut std::ffi::c_void) -> i32;
        fn CloseDesktop(desktop: *mut std::ffi::c_void) -> i32;
        fn GetThreadDesktop(thread: u32) -> *mut std::ffi::c_void;
    }
    struct ProbeDesktop(*mut std::ffi::c_void, *mut std::ffi::c_void);
    impl Drop for ProbeDesktop {
        fn drop(&mut self) {
            unsafe {
                SetThreadDesktop(self.1);
                CloseDesktop(self.0);
            }
        }
    }

    /// Manual standalone probe, explicitly excluded from every automated gate.
    /// Runs the product discovery/codec/send/pump/parser without starting mIV.
    #[test]
    #[ignore = "manual real Everything probe; requires explicit authorization"]
    fn real_everything_diagnostic_probe() {
        PROBE_TRACE.with(|enabled| enabled.set(true));
        // DESKTOP_READOBJECTS | DESKTOP_CREATEWINDOW | DESKTOP_WRITEOBJECTS.
        let desktop = ProbeDesktop(unsafe { OpenInputDesktop(0, 0, 0x83) }, unsafe {
            GetThreadDesktop(GetCurrentThreadId())
        });
        assert!(!desktop.0.is_null(), "OpenInputDesktop: {:?}", unsafe {
            windows::Win32::Foundation::GetLastError()
        });
        assert_ne!(
            unsafe { SetThreadDesktop(desktop.0) },
            0,
            "SetThreadDesktop: {:?}",
            unsafe { windows::Win32::Foundation::GetLastError() }
        );
        eprintln!("probe thread bound to input desktop (no visible desktop switch)");
        eprintln!(
            "native COPYDATASTRUCT: size={}, offsets dwData={}, cbData={}, lpData={}",
            std::mem::size_of::<COPYDATASTRUCT>(),
            std::mem::offset_of!(COPYDATASTRUCT, dwData),
            std::mem::offset_of!(COPYDATASTRUCT, cbData),
            std::mem::offset_of!(COPYDATASTRUCT, lpData)
        );
        let query = std::env::var("MIV_EVERYTHING_PROBE_QUERY")
            .expect("set MIV_EVERYTHING_PROBE_QUERY explicitly");
        let cancel = CancelHandle::new().unwrap();
        let mut endpoint = WindowsEndpoint::discover().unwrap();
        let mut pid = 0;
        let tid = unsafe { GetWindowThreadProcessId(endpoint.target, Some(&mut pid)) };
        eprintln!(
            "target pid={pid}, thread={tid}, hung={}",
            unsafe { IsHungAppWindow(endpoint.target) }.as_bool()
        );
        eprintln!("version: {:?}", endpoint.version(&cancel));
        eprintln!("database loaded: {:?}", endpoint.database_loaded());
        let id = next_reply_id().unwrap();
        let packet = encode_query(&query, endpoint.reply_hwnd(), id).unwrap();
        endpoint.activate(id);
        let budget = Duration::from_secs(60);
        let started = Instant::now();
        eprintln!("one QUERY2W send, budget_ms={}", budget.as_millis());
        let cds = COPYDATASTRUCT {
            dwData: QUERY2W,
            cbData: packet.len() as u32,
            lpData: packet.as_ptr().cast_mut().cast(),
        };
        // Diagnose one busy handler without ABORTIFHUNG's early cutoff. This
        // manual probe does not change the product flags or Everything settings.
        let sent = endpoint
            .send_message_with_flags(
                WM_COPYDATA,
                WPARAM(endpoint.reply.0 as usize),
                LPARAM((&cds as *const COPYDATASTRUCT) as isize),
                budget,
                SMTO_ERRORONEXIT,
            )
            .and_then(|accepted| {
                if accepted != 0 {
                    Ok(())
                } else {
                    Err(EverythingError::Rejected)
                }
            });
        eprintln!(
            "query send result={sent:?}, elapsed_ms={:.3}, reply_already_copied={}",
            started.elapsed().as_secs_f64() * 1000.0,
            endpoint.callback.packet.borrow().is_some()
        );
        let result = sent.and_then(|()| {
            let reply = endpoint.wait(&cancel, endpoint.elapsed() + REPLY_TIMEOUT)?;
            parse_reply(&reply.bytes)
        });
        eprintln!(
            "final result={:?}, total_elapsed_ms={:.3}",
            result
                .as_ref()
                .map(|r| (r.rows.len(), r.total_items, r.actual_sort)),
            started.elapsed().as_secs_f64() * 1000.0
        );
        endpoint.deactivate();
        assert!(result.is_ok(), "real IPC path failed: {result:?}");
    }

    enum Mode {
        Immediate,
        WrongThenImmediate,
        Malformed,
        Reject,
        Cancel(CancelHandle),
        StallStatus(CancelHandle),
        SlowQuery(Duration),
        StallQuery(CancelHandle),
    }

    struct ServerState {
        mode: Mode,
        requests: mpsc::Sender<Vec<u8>>,
    }

    unsafe extern "system" fn server_proc(
        hwnd: HWND,
        message: u32,
        wp: WPARAM,
        lp: LPARAM,
    ) -> LRESULT {
        if message == STOP {
            let _ = unsafe { DestroyWindow(hwnd) };
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        if message == WM_USER {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const ServerState;
            if !ptr.is_null() && wp.0 == GET_MAJOR {
                if let Mode::StallStatus(release) = &unsafe { &*ptr }.mode {
                    // Fake client stops pumping at the first version request. The
                    // test releases it after observing the bounded send failure.
                    // A safety deadline lets teardown finish even after a panic.
                    unsafe {
                        windows::Win32::System::Threading::WaitForSingleObject(
                            HANDLE(release.inner.event.as_raw_handle()),
                            5000,
                        );
                    }
                }
            }
            return LRESULT(match wp.0 {
                GET_MAJOR => 1,
                GET_MINOR => 4,
                GET_REVISION => 1,
                IS_DB_LOADED => 1,
                _ => 0,
            });
        }
        if message == WM_COPYDATA && lp.0 != 0 {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const ServerState;
            if ptr.is_null() {
                return LRESULT(0);
            }
            let state = unsafe { &*ptr };
            let cds = unsafe { &*(lp.0 as *const COPYDATASTRUCT) };
            if cds.dwData != QUERY2W
                || cds.cbData < QUERY_HEADER_BYTES as u32
                || cds.lpData.is_null()
            {
                return LRESULT(0);
            }
            let query =
                unsafe { std::slice::from_raw_parts(cds.lpData.cast::<u8>(), cds.cbData as usize) };
            let _ = state.requests.send(query.to_vec());
            match &state.mode {
                Mode::SlowQuery(delay) => std::thread::sleep(*delay),
                Mode::StallQuery(release) => unsafe {
                    windows::Win32::System::Threading::WaitForSingleObject(
                        HANDLE(release.inner.event.as_raw_handle()),
                        10_000,
                    );
                },
                _ => {}
            }
            if matches!(state.mode, Mode::Reject) {
                return LRESULT(0);
            }
            // Microsoft: reconstruct the truncated handle by sign extending its low32.
            let reply = HWND(dword(query, 0).unwrap() as i32 as isize as *mut _);
            let reply_id = dword(query, 4).unwrap();
            if let Mode::Cancel(cancel) = &state.mode {
                cancel.cancel();
            }
            let mut bytes = if matches!(state.mode, Mode::Malformed) {
                vec![0; 2]
            } else {
                super::super::tests::reply_bytes(&[("C:\\日本語🖼.jpg", 0)], 1, 2)
            };
            if matches!(state.mode, Mode::WrongThenImmediate) {
                send_reply(hwnd, reply, 0, &bytes);
            }
            // This call back into the sender happens BEFORE the original send returns.
            send_reply(hwnd, reply, reply_id, &bytes);
            // Callback data must already be copied, not retained as a borrowed pointer.
            bytes.fill(0xff);
            return LRESULT(1);
        }
        unsafe { DefWindowProcW(hwnd, message, wp, lp) }
    }

    fn send_reply(sender: HWND, reply: HWND, id: u32, bytes: &[u8]) {
        let cds = COPYDATASTRUCT {
            dwData: id as usize,
            cbData: bytes.len() as u32,
            lpData: bytes.as_ptr().cast_mut().cast(),
        };
        let mut acceptance = 0usize;
        let _ = unsafe {
            SendMessageTimeoutW(
                reply,
                WM_COPYDATA,
                WPARAM(sender.0 as usize),
                LPARAM((&cds as *const COPYDATASTRUCT) as isize),
                SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT,
                1000,
                Some(&mut acceptance),
            )
        };
    }

    struct FakeServer {
        hwnd: HWND,
        thread: Option<std::thread::JoinHandle<()>>,
        requests: mpsc::Receiver<Vec<u8>>,
    }

    impl FakeServer {
        fn new(mode: Mode) -> Self {
            let (ready_tx, ready_rx) = mpsc::channel();
            let (requests_tx, requests) = mpsc::channel();
            let thread = std::thread::spawn(move || {
                static REGISTERED: OnceLock<()> = OnceLock::new();
                let module = unsafe { GetModuleHandleW(None) }.unwrap();
                REGISTERED.get_or_init(|| {
                    let class = WNDCLASSW {
                        lpfnWndProc: Some(server_proc),
                        hInstance: module.into(),
                        lpszClassName: w!("mIV.Test.Everything.FakeEndpoint"),
                        ..Default::default()
                    };
                    assert_ne!(unsafe { RegisterClassW(&class) }, 0);
                });
                let state = Box::new(ServerState {
                    mode,
                    requests: requests_tx,
                });
                let hwnd = unsafe {
                    CreateWindowExW(
                        WINDOW_EX_STYLE::default(),
                        w!("mIV.Test.Everything.FakeEndpoint"),
                        w!(""),
                        WINDOW_STYLE::default(),
                        0,
                        0,
                        0,
                        0,
                        Some(HWND_MESSAGE),
                        None,
                        Some(module.into()),
                        None,
                    )
                }
                .unwrap();
                unsafe {
                    SetWindowLongPtrW(
                        hwnd,
                        GWLP_USERDATA,
                        (&*state as *const ServerState) as isize,
                    )
                };
                ready_tx.send(hwnd.0 as usize).unwrap();
                let mut message = MSG::default();
                while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
                    unsafe { DispatchMessageW(&message) };
                }
                drop(state);
            });
            let hwnd = HWND(ready_rx.recv_timeout(Duration::from_secs(5)).unwrap() as *mut _);
            Self {
                hwnd,
                thread: Some(thread),
                requests,
            }
        }
    }

    impl Drop for FakeServer {
        fn drop(&mut self) {
            let _ = unsafe { PostMessageW(Some(self.hwnd), STOP, WPARAM(0), LPARAM(0)) };
            self.thread.take().unwrap().join().unwrap();
        }
    }

    #[test]
    fn fake_hwnd_stalled_version_is_bounded_and_does_not_send_query() {
        let release = CancelHandle::new().unwrap();
        let server = FakeServer::new(Mode::StallStatus(release.clone()));
        let mut endpoint = WindowsEndpoint::for_window(server.hwnd).unwrap();
        let result = query_with_endpoint("a", &CancelHandle::new().unwrap(), &mut endpoint);
        // Unblock fake client before asserting, including on failure paths.
        release.cancel();
        assert_eq!(result, Err(EverythingError::Busy));
        assert!(
            server.requests.try_recv().is_err(),
            "no query/cancel/retry after failed version send"
        );
        let reply_hwnd = endpoint.reply;
        drop(endpoint);
        assert!(!unsafe { IsWindow(Some(reply_hwnd)) }.as_bool());
    }

    #[test]
    fn fake_hwnd_reentrant_owned_reply_wrong_id_and_actual_sort() {
        for mode in [Mode::Immediate, Mode::WrongThenImmediate] {
            let server = FakeServer::new(mode);
            let mut endpoint = WindowsEndpoint::for_window(server.hwnd).unwrap();
            let reply_hwnd = endpoint.reply;
            let result = query_with_endpoint(
                "日本語 | ext:jpg",
                &CancelHandle::new().unwrap(),
                &mut endpoint,
            )
            .unwrap();
            assert_eq!(result.rows[0].full_path, "C:\\日本語🖼.jpg");
            assert_eq!(result.actual_sort, 2);
            let request = server
                .requests
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
            assert_eq!(dword(&request, 0).unwrap(), reply_hwnd.0 as usize as u32);
            assert_eq!(dword(&request, 12).unwrap(), 0);
            drop(endpoint);
            assert!(
                !unsafe { IsWindow(Some(reply_hwnd)) }.as_bool(),
                "RAII destroyed reply window"
            );
            assert!(
                server.requests.try_recv().is_err(),
                "one query, no fallback/paging"
            );
        }
    }

    #[test]
    fn fake_hwnd_slow_handler_outlives_old_one_second_send_budget() {
        let server = FakeServer::new(Mode::SlowQuery(Duration::from_millis(1200)));
        let target = server.hwnd.0 as usize;
        let cancel = CancelHandle::new().unwrap();
        let run_cancel = cancel.clone();
        let result = run_cancellable(&cancel, move || {
            let mut endpoint = WindowsEndpoint::for_window(HWND(target as *mut _))?;
            query_with_endpoint("slow", &run_cancel, &mut endpoint)
        })
        .unwrap();
        assert_eq!(result.rows[0].full_path, "C:\\日本語🖼.jpg");
        assert!(server.requests.try_recv().is_ok());
        assert!(
            server.requests.try_recv().is_err(),
            "no retry or cancel after success"
        );
    }

    #[test]
    fn fake_hwnd_cancel_returns_before_slow_send_and_worker_keeps_hwnd_until_retirement() {
        let release = CancelHandle::new().unwrap();
        let server = FakeServer::new(Mode::StallQuery(release.clone()));
        let target = server.hwnd.0 as usize;
        let cancel = CancelHandle::new().unwrap();
        let waiter_cancel = cancel.clone();
        let (window_tx, window_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let (retired_tx, retired_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let run_cancel = waiter_cancel.clone();
            let result = run_cancellable(&waiter_cancel, move || {
                let mut endpoint = WindowsEndpoint::for_window(HWND(target as *mut _))?;
                window_tx.send(endpoint.reply.0 as usize).unwrap();
                let result = query_with_endpoint("slow", &run_cancel, &mut endpoint);
                drop(endpoint);
                retired_tx.send(()).unwrap();
                result
            });
            done_tx.send(result).unwrap();
        });
        let reply = HWND(window_rx.recv_timeout(Duration::from_secs(5)).unwrap() as *mut _);
        server
            .requests
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        cancel.cancel();
        let result = done_rx.recv_timeout(Duration::from_secs(2));
        let kept_hwnd = unsafe { IsWindow(Some(reply)) }.as_bool();
        let prematurely_retired = retired_rx.try_recv().is_ok();
        // Release even if the observations fail, so fake teardown never waits
        // for the product's long send deadline merely to report an assertion.
        release.cancel();
        waiter.join().unwrap();
        retired_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(result.unwrap(), Err(EverythingError::Cancelled));
        assert!(
            kept_hwnd,
            "native send still owns its callback/packet lifetime"
        );
        assert!(!prematurely_retired);
        assert!(!unsafe { IsWindow(Some(reply)) }.as_bool());
    }

    #[test]
    fn fake_hwnd_rejection_malformed_and_cancel_late_reply() {
        for mode in [Mode::Reject, Mode::Malformed] {
            let server = FakeServer::new(mode);
            let mut endpoint = WindowsEndpoint::for_window(server.hwnd).unwrap();
            let error =
                query_with_endpoint("a", &CancelHandle::new().unwrap(), &mut endpoint).unwrap_err();
            assert!(matches!(
                error,
                EverythingError::Rejected | EverythingError::Malformed(_)
            ));
        }
        let cancel = CancelHandle::new().unwrap();
        let server = FakeServer::new(Mode::Cancel(cancel.clone()));
        let mut endpoint = WindowsEndpoint::for_window(server.hwnd).unwrap();
        assert_eq!(
            query_with_endpoint("a", &cancel, &mut endpoint),
            Err(EverythingError::Cancelled)
        );
        assert!(cancel.is_cancelled());
        let original = server
            .requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        let cancellation = server
            .requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            dword(&original, 0).unwrap(),
            dword(&cancellation, 0).unwrap()
        );
        assert_ne!(
            dword(&original, 4).unwrap(),
            dword(&cancellation, 4).unwrap()
        );
        assert!(
            server.requests.try_recv().is_err(),
            "cancel sent exactly once"
        );
    }
}
