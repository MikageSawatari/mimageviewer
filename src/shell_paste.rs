//! One application-lifetime STA for Shell paste invocation and its OLE proxies.
//!
//! A result means InvokeCommand returned, not that Shell finished transferring
//! files. The caller owns admission and selection correlation. This service
//! never reads or compares the clipboard, waits for initialization, or joins.

use std::path::PathBuf;
use std::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShellPasteError {
    Start(String),
    Invocation(String),
}

pub(crate) type ShellPasteResult = Result<(), ShellPasteError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusReturn {
    AlreadyMain,
    Restored,
    OtherMivWindow,
    OtherProcess,
    MainGone,
    RestoreFailed,
    NotInvoked,
}

impl FocusReturn {
    pub(crate) fn perf_tag(self) -> &'static str {
        match self {
            Self::AlreadyMain => "already_main",
            Self::Restored => "restored",
            Self::OtherMivWindow => "other_miv_window",
            Self::OtherProcess => "other_process",
            Self::MainGone => "main_gone",
            Self::RestoreFailed => "restore_failed",
            Self::NotInvoked => "not_invoked",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ShellPasteCompletion {
    pub(crate) invocation: ShellPasteResult,
    pub(crate) focus_return: FocusReturn,
}

/// One snapshot at the invocation's return edge; no UI-thread state is consulted.
pub(crate) struct FocusSnapshot {
    pub(crate) main_valid: bool,
    pub(crate) main_thread: u32,
    pub(crate) current_process: u32,
    pub(crate) foreground_hwnd: isize,
    pub(crate) foreground_thread: u32,
    pub(crate) foreground_process: u32,
}

pub(crate) trait FocusApi {
    fn snapshot(&mut self, main_hwnd: isize) -> FocusSnapshot;
    fn set_foreground(&mut self, main_hwnd: isize) -> bool;
    fn class_name(&self, foreground_hwnd: isize) -> String;
}

#[derive(Debug, PartialEq, Eq)]
enum FocusDecision {
    Keep(FocusReturn),
    Restore,
}

fn decide_focus_return(main_hwnd: isize, snapshot: &FocusSnapshot) -> FocusDecision {
    if !snapshot.main_valid {
        FocusDecision::Keep(FocusReturn::MainGone)
    } else if snapshot.foreground_hwnd == main_hwnd {
        FocusDecision::Keep(FocusReturn::AlreadyMain)
    } else if snapshot.foreground_hwnd == 0 {
        FocusDecision::Restore
    } else if snapshot.foreground_process != snapshot.current_process {
        FocusDecision::Keep(FocusReturn::OtherProcess)
    } else if snapshot.foreground_thread == snapshot.main_thread {
        FocusDecision::Keep(FocusReturn::OtherMivWindow)
    } else {
        FocusDecision::Restore
    }
}

fn return_focus(main_hwnd: isize, api: &mut dyn FocusApi) -> FocusReturn {
    let snapshot = api.snapshot(main_hwnd);
    let class_name = crate::perf::is_enabled().then(|| api.class_name(snapshot.foreground_hwnd));
    let result = match decide_focus_return(main_hwnd, &snapshot) {
        FocusDecision::Keep(result) => result,
        FocusDecision::Restore => {
            if api.set_foreground(main_hwnd) {
                FocusReturn::Restored
            } else {
                crate::logger::log("shell_paste: SetForegroundWindow(main) failed; not retrying");
                FocusReturn::RestoreFailed
            }
        }
    };
    if let Some(class_name) = class_name {
        let has_foreground = snapshot.foreground_hwnd != 0;
        crate::perf::event(
            "shell_paste",
            "focus_return",
            None,
            0,
            &[
                ("focus_return", result.perf_tag().into()),
                ("foreground_class", class_name.into()),
                (
                    "same_thread",
                    has_foreground
                        .then_some(snapshot.foreground_thread == snapshot.main_thread)
                        .into(),
                ),
                (
                    "same_process",
                    has_foreground
                        .then_some(snapshot.foreground_process == snapshot.current_process)
                        .into(),
                ),
            ],
        );
    }
    result
}

fn complete_invocation(
    main_hwnd: isize,
    invocation: ShellPasteResult,
    focus: &mut dyn FocusApi,
) -> ShellPasteCompletion {
    let focus_return = if matches!(invocation, Err(ShellPasteError::Start(_))) {
        FocusReturn::NotInvoked
    } else {
        return_focus(main_hwnd, focus)
    };
    ShellPasteCompletion {
        invocation,
        focus_return,
    }
}

pub(crate) struct ShellPasteService {
    state: ServiceState,
    #[cfg(test)]
    test_invoker: Option<(TestInvoker, Box<dyn FocusApi + Send>)>,
}

enum ServiceState {
    NotStarted,
    Running(Endpoint),
    Unavailable(String),
}

struct Endpoint {
    sender: mpsc::Sender<PasteRequest>,
    wake: WakeEvent,
}

struct PasteRequest {
    hwnd: isize,
    folder: PathBuf,
    completion: Completion,
}

/// Wake even on worker unwind/disconnection, after closing the result sender.
struct Completion {
    sender: Option<mpsc::Sender<ShellPasteCompletion>>,
    ctx: egui::Context,
}

impl Completion {
    fn finish(mut self, result: ShellPasteCompletion) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(result);
        }
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        drop(self.sender.take());
        self.ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}

impl Default for ShellPasteService {
    fn default() -> Self {
        Self {
            state: ServiceState::NotStarted,
            #[cfg(test)]
            test_invoker: None,
        }
    }
}

impl ShellPasteService {
    pub(crate) fn submit(
        &mut self,
        hwnd: isize,
        folder: PathBuf,
        ctx: egui::Context,
    ) -> Result<mpsc::Receiver<ShellPasteCompletion>, String> {
        if matches!(self.state, ServiceState::NotStarted) {
            self.state = match self.start_worker() {
                Ok(endpoint) => ServiceState::Running(endpoint),
                Err(error) => ServiceState::Unavailable(error),
            };
        }
        let endpoint = match &self.state {
            ServiceState::Running(endpoint) => endpoint,
            ServiceState::Unavailable(error) => return Err(error.clone()),
            ServiceState::NotStarted => unreachable!(),
        };
        let (sender, receiver) = mpsc::channel();
        endpoint
            .sender
            .send(PasteRequest {
                hwnd,
                folder,
                completion: Completion {
                    sender: Some(sender),
                    ctx,
                },
            })
            .map_err(|_| "Shell paste worker disconnected".to_owned())?;
        endpoint.wake.signal()?;
        Ok(receiver)
    }

    fn start_worker(&mut self) -> Result<Endpoint, String> {
        #[cfg(test)]
        if let Some((mut invoke, mut focus)) = self.test_invoker.take() {
            let (sender, receiver) = mpsc::channel::<PasteRequest>();
            std::thread::Builder::new()
                .name("shell-paste-test".into())
                .spawn(move || {
                    while let Ok(request) = receiver.recv() {
                        let result = invoke(request.hwnd, &request.folder);
                        let completion = complete_invocation(request.hwnd, result, focus.as_mut());
                        request.completion.finish(completion);
                    }
                })
                .map_err(|error| format!("Could not start Shell paste worker: {error}"))?;
            return Ok(Endpoint {
                sender,
                wake: WakeEvent::headless(),
            });
        }
        native::start_worker()
    }

    /// Each test owns its invoker and worker; no process-global hook or COM.
    #[cfg(test)]
    pub(crate) fn with_test_invoker(
        invoker: impl FnMut(isize, &std::path::Path) -> ShellPasteResult + Send + 'static,
    ) -> Self {
        Self::with_test_invoker_and_focus(invoker, NoMainForTest)
    }

    #[cfg(test)]
    pub(crate) fn with_test_invoker_and_focus(
        invoker: impl FnMut(isize, &std::path::Path) -> ShellPasteResult + Send + 'static,
        focus: impl FocusApi + Send + 'static,
    ) -> Self {
        Self {
            state: ServiceState::NotStarted,
            test_invoker: Some((Box::new(invoker), Box::new(focus))),
        }
    }
}

impl Drop for ShellPasteService {
    fn drop(&mut self) {
        if let ServiceState::Running(endpoint) =
            std::mem::replace(&mut self.state, ServiceState::NotStarted)
        {
            // Disconnect first; the worker then observes shutdown after waking.
            drop(endpoint.sender);
            if let Err(error) = endpoint.wake.signal() {
                crate::logger::log(format!("shell_paste: shutdown wake failed: {error}"));
            }
        }
    }
}

#[cfg(test)]
type TestInvoker = Box<dyn FnMut(isize, &std::path::Path) -> ShellPasteResult + Send + 'static>;

#[cfg(test)]
struct NoMainForTest;

#[cfg(test)]
impl FocusApi for NoMainForTest {
    fn snapshot(&mut self, _: isize) -> FocusSnapshot {
        FocusSnapshot {
            main_valid: false,
            main_thread: 0,
            current_process: 0,
            foreground_hwnd: 0,
            foreground_thread: 0,
            foreground_process: 0,
        }
    }

    fn set_foreground(&mut self, _: isize) -> bool {
        panic!("a headless test must never activate a native window")
    }

    fn class_name(&self, _: isize) -> String {
        "<headless>".into()
    }
}

#[derive(Clone)]
struct WakeEvent {
    #[cfg(windows)]
    handle: Option<std::sync::Arc<std::os::windows::io::OwnedHandle>>,
}

impl WakeEvent {
    #[cfg(test)]
    fn headless() -> Self {
        Self {
            #[cfg(windows)]
            handle: None,
        }
    }

    fn signal(&self) -> Result<(), String> {
        #[cfg(windows)]
        if let Some(handle) = &self.handle {
            use std::os::windows::io::AsRawHandle;
            use windows::Win32::{Foundation::HANDLE, System::Threading::SetEvent};
            unsafe { SetEvent(HANDLE(handle.as_raw_handle())) }
                .map_err(|error| format!("Shell paste wake failed: {error}"))?;
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod native {
    use super::Endpoint;

    pub(super) fn start_worker() -> Result<Endpoint, String> {
        Err("Shell paste is available only on Windows".into())
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::sync::Arc;
    use windows::Win32::{
        Foundation::{HANDLE, HWND, WAIT_FAILED},
        System::{
            Ole::{OleInitialize, OleUninitialize},
            Threading::{CreateEventW, GetCurrentProcessId, INFINITE},
        },
        UI::WindowsAndMessaging::{
            DispatchMessageW, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId,
            IsWindow, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
            SetForegroundWindow, TranslateMessage, WM_QUIT,
        },
    };

    struct NativeFocus;

    impl FocusApi for NativeFocus {
        fn snapshot(&mut self, main_hwnd: isize) -> FocusSnapshot {
            let main = HWND(main_hwnd as *mut _);
            let main_valid = unsafe { IsWindow(Some(main)) }.as_bool();
            let main_thread = if main_valid {
                unsafe { GetWindowThreadProcessId(main, None) }
            } else {
                0
            };
            let foreground = unsafe { GetForegroundWindow() };
            let mut foreground_process = 0;
            let foreground_thread = if foreground.0.is_null() {
                0
            } else {
                unsafe { GetWindowThreadProcessId(foreground, Some(&mut foreground_process)) }
            };
            FocusSnapshot {
                main_valid,
                main_thread,
                current_process: unsafe { GetCurrentProcessId() },
                foreground_hwnd: foreground.0 as isize,
                foreground_thread,
                foreground_process,
            }
        }

        fn set_foreground(&mut self, main_hwnd: isize) -> bool {
            unsafe { SetForegroundWindow(HWND(main_hwnd as *mut _)) }.as_bool()
        }

        fn class_name(&self, foreground_hwnd: isize) -> String {
            if foreground_hwnd == 0 {
                return "<none>".into();
            }
            let mut name = [0_u16; 256];
            let len = unsafe { GetClassNameW(HWND(foreground_hwnd as *mut _), &mut name) };
            String::from_utf16_lossy(&name[..len.max(0) as usize])
        }
    }

    struct OleSta;

    impl Drop for OleSta {
        fn drop(&mut self) {
            unsafe { OleUninitialize() };
        }
    }

    pub(super) fn start_worker() -> Result<Endpoint, String> {
        // Auto-reset is safe: the channel is the queue, the event only wakes it.
        let handle = unsafe { CreateEventW(None, false, false, None) }
            .map_err(|error| format!("Could not create Shell paste wake event: {error}"))?;
        // SAFETY: ownership of this fresh handle moves into OwnedHandle once.
        let handle = Arc::new(unsafe { OwnedHandle::from_raw_handle(handle.0) });
        let wake = WakeEvent {
            handle: Some(handle.clone()),
        };
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("shell-paste".into())
            .spawn(move || run(receiver, handle))
            .map_err(|error| format!("Could not start Shell paste worker: {error}"))?;
        Ok(Endpoint { sender, wake })
    }

    fn run(receiver: mpsc::Receiver<PasteRequest>, wake: Arc<OwnedHandle>) {
        let _ole = match unsafe { OleInitialize(None) } {
            Ok(()) => OleSta,
            Err(error) => {
                let message = format!("Shell paste OLE initialization failed: {error}");
                // Never retry initialization, and never call Shell without OLE.
                while let Ok(request) = receiver.recv() {
                    request.completion.finish(ShellPasteCompletion {
                        invocation: Err(ShellPasteError::Start(message.clone())),
                        focus_return: FocusReturn::NotInvoked,
                    });
                }
                return;
            }
        };
        let handles = [HANDLE(wake.as_raw_handle())];
        loop {
            let mut message = MSG::default();
            // Drain before waiting: MsgWait only signals messages not yet seen.
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                if message.message == WM_QUIT {
                    return;
                }
                unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            // Drain queued requests too, without waiting for another event edge.
            match receiver.try_recv() {
                Ok(request) => {
                    let result = crate::native_context_menu::invoke_shell_folder_background_verb(
                        request.hwnd,
                        &request.folder,
                        crate::native_context_menu::ShellClipboardVerb::Paste,
                    )
                    .map_err(ShellPasteError::Invocation);
                    let completion = complete_invocation(request.hwnd, result, &mut NativeFocus);
                    request.completion.finish(completion);
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => return,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            // Keep pumping for Shell transfer proxies even after invocation ends.
            if unsafe { MsgWaitForMultipleObjects(Some(&handles), false, INFINITE, QS_ALLINPUT) }
                == WAIT_FAILED
            {
                crate::logger::log("shell_paste: message wait failed".to_owned());
                return;
            }
        }
    }
}

#[cfg(all(test, windows))]
#[path = "shell_paste_native_tests.rs"]
mod native_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn shell_paste_runs_off_caller_and_reuses_worker_after_invocation() {
        let caller = std::thread::current().id();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = calls.clone();
        let mut service = ShellPasteService::with_test_invoker(move |hwnd, folder| {
            observed.lock().unwrap().push((
                std::thread::current().id(),
                hwnd,
                folder.to_path_buf(),
            ));
            Ok(())
        });
        for folder in ["first", "second"] {
            let result = service
                .submit(42, folder.into(), egui::Context::default())
                .unwrap();
            assert_eq!(
                result.recv_timeout(Duration::from_secs(5)).unwrap(),
                ShellPasteCompletion {
                    invocation: Ok(()),
                    focus_return: FocusReturn::MainGone,
                }
            );
        }
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_ne!(calls[0].0, caller);
        assert_eq!(calls[0].0, calls[1].0);
        assert_eq!(calls[0].1, 42);
        assert_eq!(calls[0].2, PathBuf::from("first"));
        assert_eq!(calls[1].2, PathBuf::from("second"));
    }

    #[test]
    fn shell_paste_submission_and_drop_do_not_wait_for_invocation() {
        let (entered, running) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let mut service = ShellPasteService::with_test_invoker(move |_, _| {
            entered.send(()).unwrap();
            blocked.recv().unwrap();
            Err(ShellPasteError::Invocation("expected failure".into()))
        });
        let result = service
            .submit(1, "folder".into(), egui::Context::default())
            .unwrap();
        running.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(result.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(service);
        release.send(()).unwrap();
        assert_eq!(
            result.recv_timeout(Duration::from_secs(5)).unwrap(),
            ShellPasteCompletion {
                invocation: Err(ShellPasteError::Invocation("expected failure".into())),
                focus_return: FocusReturn::MainGone,
            }
        );
    }

    struct FakeFocus {
        main_valid: bool,
        foreground: (isize, u32, u32),
        restore_succeeds: bool,
        snapshots: usize,
        restored: Vec<isize>,
    }

    impl FocusApi for FakeFocus {
        fn snapshot(&mut self, main_hwnd: isize) -> FocusSnapshot {
            assert_eq!(main_hwnd, 42);
            self.snapshots += 1;
            FocusSnapshot {
                main_valid: self.main_valid,
                main_thread: 10,
                current_process: 100,
                foreground_hwnd: self.foreground.0,
                foreground_thread: self.foreground.1,
                foreground_process: self.foreground.2,
            }
        }

        fn set_foreground(&mut self, main_hwnd: isize) -> bool {
            self.restored.push(main_hwnd);
            self.restore_succeeds
        }

        fn class_name(&self, _: isize) -> String {
            "<fake>".into()
        }
    }

    #[test]
    fn shell_paste_focus_return_decision_table() {
        use FocusDecision::{Keep, Restore};
        use FocusReturn::*;
        // HWND, thread, process. Main is HWND 42, thread 10, process 100.
        let cases = [
            (true, (42, 10, 100), true, Keep(AlreadyMain), AlreadyMain, 0),
            (true, (0, 0, 0), true, Restore, Restored, 1),
            (true, (43, 11, 100), true, Restore, Restored, 1),
            (
                true,
                (43, 10, 100),
                true,
                Keep(OtherMivWindow),
                OtherMivWindow,
                0,
            ),
            (
                true,
                (43, 11, 101),
                true,
                Keep(OtherProcess),
                OtherProcess,
                0,
            ),
            // Main validity takes precedence even when it is the foreground HWND.
            (false, (42, 10, 100), true, Keep(MainGone), MainGone, 0),
            (true, (43, 11, 100), false, Restore, RestoreFailed, 1),
        ];
        for (main_valid, foreground, restore_succeeds, decision, expected, attempts) in cases {
            let mut api = FakeFocus {
                main_valid,
                foreground,
                restore_succeeds,
                snapshots: 0,
                restored: Vec::new(),
            };
            let snapshot = api.snapshot(42);
            assert_eq!(decide_focus_return(42, &snapshot), decision);
            api.snapshots = 0;
            assert_eq!(return_focus(42, &mut api), expected);
            assert_eq!(api.snapshots, 1);
            assert_eq!(api.restored, vec![42; attempts]);
        }
    }

    #[test]
    fn shell_paste_not_invoked_skips_focus_queries_and_restore() {
        let mut api = FakeFocus {
            main_valid: true,
            foreground: (43, 11, 100),
            restore_succeeds: true,
            snapshots: 0,
            restored: Vec::new(),
        };
        let completion = complete_invocation(
            42,
            Err(ShellPasteError::Start("OLE failed".into())),
            &mut api,
        );
        assert_eq!(completion.focus_return, FocusReturn::NotInvoked);
        assert_eq!(completion.focus_return.perf_tag(), "not_invoked");
        assert_eq!(api.snapshots, 0);
        assert!(api.restored.is_empty());
    }

    #[test]
    fn shell_paste_focus_return_perf_tags() {
        for (result, tag) in [
            (FocusReturn::AlreadyMain, "already_main"),
            (FocusReturn::Restored, "restored"),
            (FocusReturn::OtherMivWindow, "other_miv_window"),
            (FocusReturn::OtherProcess, "other_process"),
            (FocusReturn::MainGone, "main_gone"),
            (FocusReturn::RestoreFailed, "restore_failed"),
            (FocusReturn::NotInvoked, "not_invoked"),
        ] {
            assert_eq!(result.perf_tag(), tag);
        }
    }
}
