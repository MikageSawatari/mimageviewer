//! Win32 ownership adapter. All windows, sends, callbacks and waits stay on one worker.
//! Microsoft API contracts: WM_COPYDATA, SendMessageTimeoutW, and
//! MsgWaitForMultipleObjectsEx; callback buffers are copied before returning.

use super::*;
use std::cell::{Cell, RefCell};
use std::os::windows::io::AsRawHandle;
use std::sync::OnceLock;
use std::time::Instant;
use windows::Win32::Foundation::{
    HANDLE, HWND, LPARAM, LRESULT, WAIT_FAILED, WAIT_TIMEOUT, WPARAM,
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
        let mut response = 0usize;
        unsafe {
            windows::Win32::Foundation::SetLastError(windows::Win32::Foundation::WIN32_ERROR(0))
        };
        // No SMTO_BLOCK: sent-message callbacks must remain dispatchable during send.
        // No NOTIMEOUTIFNOTHUNG: each send remains bounded even when the peer pumps.
        let sent = unsafe {
            SendMessageTimeoutW(
                self.target,
                message,
                wp,
                lp,
                SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT,
                timeout.as_millis().clamp(1, SEND_TIMEOUT.as_millis()) as u32,
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
            crate::logger::log(format!(
                "everything IPC send failed: message={message:#x}, status_code={}, win32_error={}",
                if message == WM_USER { wp.0 } else { 0 },
                last_error.0
            ));
            Err(EverythingError::SendFailed)
        } else {
            Ok(response)
        }
    }

    fn status(&self, code: usize) -> Result<u32, EverythingError> {
        let response = self.send_message(WM_USER, WPARAM(code), LPARAM(0), SEND_TIMEOUT)?;
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
        let result = query_with_endpoint(&query, &cancel, &mut endpoint);
        eprintln!(
            "product result: {:?}",
            result
                .as_ref()
                .map(|r| (r.rows.len(), r.total_items, r.actual_sort))
        );
        for code in [GET_MINOR, GET_REVISION, 3, IS_DB_LOADED, 403] {
            eprintln!(
                "additional read-only status {code}: {:?}",
                endpoint.status(code)
            );
        }
        let mut filter = CHANGEFILTERSTRUCT {
            cbSize: std::mem::size_of::<CHANGEFILTERSTRUCT>() as u32,
            ..Default::default()
        };
        let allowed = unsafe {
            ChangeWindowMessageFilterEx(
                endpoint.reply,
                WM_COPYDATA,
                MSGFLT_ALLOW,
                Some(&mut filter),
            )
        };
        eprintln!(
            "reply-only WM_COPYDATA filter allow: {allowed:?}, ExtStatus={:?}",
            filter.ExtStatus
        );
        for v2 in [true, false] {
            let id = if v2 { 0x137402 } else { 0x137401 };
            let packet = encode_query(&query, endpoint.reply_hwnd(), id).unwrap();
            let v1;
            let bytes = if v2 {
                &packet
            } else {
                v1 = [&packet[..20], &packet[28..]].concat();
                &v1
            };
            endpoint.activate(id);
            let cds = COPYDATASTRUCT {
                dwData: if v2 { 18 } else { 2 },
                cbData: bytes.len() as u32,
                lpData: bytes.as_ptr().cast_mut().cast(),
            };
            eprintln!(
                "diagnostic query v{}: COPYDATA id={}, header={}, packet={}",
                if v2 { 2 } else { 1 },
                cds.dwData,
                if v2 { 28 } else { 20 },
                bytes.len()
            );
            let sent = endpoint.send_message(
                WM_COPYDATA,
                WPARAM(endpoint.reply.0 as usize),
                LPARAM((&cds as *const COPYDATASTRUCT) as isize),
                SEND_TIMEOUT,
            );
            eprintln!("diagnostic query send: {sent:?}");
            if matches!(sent, Ok(1)) {
                let deadline = endpoint.elapsed() + REPLY_TIMEOUT;
                let reply = endpoint.wait(&cancel, deadline);
                eprintln!(
                    "diagnostic reply: {:?}",
                    reply.as_ref().map(|r| r.bytes.len())
                );
            }
            endpoint.deactivate();
        }
        assert!(result.is_ok(), "product IPC path failed: {result:?}");
    }

    enum Mode {
        Immediate,
        WrongThenImmediate,
        Malformed,
        Reject,
        Cancel(CancelHandle),
        StallStatus(CancelHandle),
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
        assert_eq!(result, Err(EverythingError::SendFailed));
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
