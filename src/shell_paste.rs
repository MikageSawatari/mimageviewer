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

pub(crate) struct ShellPasteService {
    state: ServiceState,
    #[cfg(test)]
    test_invoker: Option<TestInvoker>,
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
    sender: Option<mpsc::Sender<ShellPasteResult>>,
    ctx: egui::Context,
}

impl Completion {
    fn finish(mut self, result: ShellPasteResult) {
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
    ) -> Result<mpsc::Receiver<ShellPasteResult>, String> {
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
        if let Some(mut invoke) = self.test_invoker.take() {
            let (sender, receiver) = mpsc::channel::<PasteRequest>();
            std::thread::Builder::new()
                .name("shell-paste-test".into())
                .spawn(move || {
                    while let Ok(request) = receiver.recv() {
                        let result = invoke(request.hwnd, &request.folder);
                        request.completion.finish(result);
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
        Self {
            state: ServiceState::NotStarted,
            test_invoker: Some(Box::new(invoker)),
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
        Foundation::{HANDLE, WAIT_FAILED},
        System::{
            Ole::{OleInitialize, OleUninitialize},
            Threading::{CreateEventW, INFINITE},
        },
        UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
            TranslateMessage, WM_QUIT,
        },
    };

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
                    request
                        .completion
                        .finish(Err(ShellPasteError::Start(message.clone())));
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
                    request.completion.finish(result);
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
            assert_eq!(result.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(()));
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
            Err(ShellPasteError::Invocation("expected failure".into()))
        );
    }
}
