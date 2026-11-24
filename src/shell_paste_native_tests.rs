//! Opt-in real Shell tests. These replace the desktop clipboard; run serially on the
//! designated test desktop, without clipboard sharing/managers or concurrent user writes.
//! They create only a hidden native owner HWND and disposable files, never the product.
//! InvokeCommand completion and transfer completion are deliberately checked separately.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardOwner, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_HDROP, OleInitialize, OleSetClipboard, OleUninitialize};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetWindowThreadProcessId, MSG,
    MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
    WINDOW_EX_STYLE, WINDOW_STYLE,
};
use windows::core::w;

use crate::cut_clipboard::{ClipboardTransferSignal, LocalCutDataObject};
use crate::native_context_menu::{ShellClipboardVerb, invoke_shell_file_verb};
use crate::shell_paste::ShellPasteService;

static CLIPBOARD_TEST: Mutex<()> = Mutex::new(());
const NATIVE_TIMEOUT: Duration = Duration::from_secs(30);

type CompletionObservations = Arc<Mutex<Vec<(ClipboardTransferSignal, ThreadId)>>>;

struct OleSta;

impl Drop for OleSta {
    fn drop(&mut self) {
        unsafe { OleUninitialize() };
    }
}

struct HiddenOwner(HWND);

impl Drop for HiddenOwner {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

struct ClearOwnClipboard(u32);

impl Drop for ClearOwnClipboard {
    fn drop(&mut self) {
        // Do not erase a newer clipboard owned by another application. OLE's hidden
        // clipboard window belongs to the apartment that called OleSetClipboard.
        if let Ok(owner) = unsafe { GetClipboardOwner() }
            && unsafe { GetWindowThreadProcessId(owner, None) } == self.0
        {
            let _ = unsafe { OleSetClipboard(None::<&IDataObject>) };
        }
    }
}

fn pump_once() {
    unsafe {
        MsgWaitForMultipleObjectsEx(None, 10, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            DispatchMessageW(&message);
        }
    }
}

fn put_hdrop(hwnd: HWND, path: &Path) -> Result<(), String> {
    let mut bytes = vec![0_u8; 20]; // DROPFILES, UTF-16, double-NUL-terminated path list.
    bytes[..4].copy_from_slice(&20_u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&1_u32.to_le_bytes());
    let path = path.to_string_lossy();
    let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
    bytes.extend(path.encode_utf16().chain([0, 0]).flat_map(u16::to_le_bytes));

    let memory =
        unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }.map_err(|error| error.to_string())?;
    let pointer = unsafe { GlobalLock(memory) };
    if pointer.is_null() {
        let _ = unsafe { GlobalFree(Some(memory)) };
        return Err("GlobalLock(CF_HDROP) failed".into());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(memory);
    }
    let opened = unsafe { OpenClipboard(Some(hwnd)) }.map_err(|error| error.to_string());
    if let Err(error) = opened {
        let _ = unsafe { GlobalFree(Some(memory)) };
        return Err(error);
    }
    let written = unsafe { EmptyClipboard() }
        .and_then(|()| unsafe { SetClipboardData(u32::from(CF_HDROP.0), Some(HANDLE(memory.0))) })
        .map(|_| ())
        .map_err(|error| error.to_string());
    // Only a successful SetClipboardData transfers the allocation to Windows.
    if written.is_err() {
        let _ = unsafe { GlobalFree(Some(memory)) };
    }
    let closed = unsafe { CloseClipboard() }.map_err(|error| error.to_string());
    written.and(closed)
}

struct ClipboardSta {
    hwnd: isize,
    thread_id: ThreadId,
    observations: Option<CompletionObservations>,
    stop: mpsc::Sender<()>,
    done: mpsc::Receiver<()>,
    worker: Option<JoinHandle<()>>,
}

impl ClipboardSta {
    fn start(path: PathBuf, cut: bool) -> Self {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (stop, stop_rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("shell-paste-test-ui-sta".into())
            .spawn(move || {
                let run = || -> Result<(), String> {
                    unsafe { OleInitialize(None) }.map_err(|error| error.to_string())?;
                    let _ole = OleSta;
                    // No WS_VISIBLE, ShowWindow, activation, foreground change or input.
                    let hwnd = HiddenOwner(
                        unsafe {
                            CreateWindowExW(
                                WINDOW_EX_STYLE::default(),
                                w!("STATIC"),
                                w!("mIV Shell paste test owner"),
                                WINDOW_STYLE::default(),
                                0,
                                0,
                                0,
                                0,
                                None,
                                None,
                                None,
                                None,
                            )
                        }
                        .map_err(|error| error.to_string())?,
                    );
                    let _clipboard = ClearOwnClipboard(unsafe { GetCurrentThreadId() });
                    let hwnd_value = hwnd.0.0 as isize;
                    let observations = if cut {
                        let owner = LocalCutDataObject::inert_for_test([0x85; 16], 1859);
                        let observations = owner.completion_thread_observations_for_test();
                        // The same production path builds the Shell IDataObject, wraps it
                        // with LocalCutDataObject and calls OleSetClipboard on this STA.
                        invoke_shell_file_verb(
                            hwnd_value,
                            &[path],
                            ShellClipboardVerb::Cut,
                            Some(owner),
                        )?;
                        Some(observations)
                    } else {
                        put_hdrop(hwnd.0, &path)?;
                        None
                    };
                    ready_tx
                        .send(Ok((hwnd_value, thread::current().id(), observations)))
                        .map_err(|_| "test stopped before clipboard was ready".to_string())?;
                    // Test-only finite lifetime; the production service has its own pump.
                    let deadline = Instant::now() + Duration::from_secs(90);
                    while matches!(stop_rx.try_recv(), Err(mpsc::TryRecvError::Empty))
                        && Instant::now() < deadline
                    {
                        pump_once();
                    }
                    Ok(())
                };
                if let Err(error) = run() {
                    let _ = ready_tx.send(Err(error));
                }
                let _ = done_tx.send(());
            })
            .expect("spawn clipboard owner STA");
        let (hwnd, thread_id, observations) = ready_rx
            .recv_timeout(NATIVE_TIMEOUT)
            .expect("clipboard owner STA initialization timed out")
            .expect("prepare native clipboard fixture");
        Self {
            hwnd,
            thread_id,
            observations,
            stop,
            done,
            worker: Some(worker),
        }
    }

    fn finish(&mut self) -> bool {
        let _ = self.stop.send(());
        if self.done.recv_timeout(Duration::from_secs(2)).is_err() {
            return false;
        }
        self.worker
            .take()
            .is_none_or(|worker| worker.join().is_ok())
    }
}

impl Drop for ClipboardSta {
    fn drop(&mut self) {
        if self.worker.is_some() {
            // Do not hang the test harness if an external Shell component stops replying.
            let _ = self.finish();
        }
    }
}

fn wait_for_transfer(mut completed: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + NATIVE_TIMEOUT;
    while !completed() {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    true
}

#[test]
#[ignore = "real Shell: replaces desktop clipboard; run alone on the designated test desktop"]
fn shell_paste_native_hdrop_copy_uses_resident_sta() {
    let _serial = CLIPBOARD_TEST
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let fixture = tempfile::Builder::new()
        .prefix("miv-shell-copy-")
        .tempdir()
        .unwrap();
    let source = fixture.path().join("copy-source.txt");
    let destination = fixture.path().join("destination");
    let payload = b"resident shell STA CF_HDROP copy fixture";
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(&source, payload).unwrap();
    let mut owner = ClipboardSta::start(source.clone(), false);
    let mut service = ShellPasteService::default();
    let invocation = service
        .submit(owner.hwnd, destination.clone(), egui::Context::default())
        .expect("submit CF_HDROP copy");
    invocation
        .recv_timeout(NATIVE_TIMEOUT)
        .expect("copy InvokeCommand timed out")
        .expect("copy InvokeCommand failed");
    // Keep both apartments alive after InvokeCommand; Shell owns transfer completion.
    assert!(
        wait_for_transfer(|| {
            std::fs::read(destination.join("copy-source.txt")).is_ok_and(|bytes| bytes == payload)
        }),
        "Shell copy did not complete after invocation returned"
    );
    assert_eq!(
        std::fs::read(&source).unwrap(),
        payload,
        "copy preserves source"
    );
    assert!(owner.finish(), "clipboard STA did not finish cleanup");
}

#[test]
#[ignore = "real Shell: replaces desktop clipboard; run alone on the designated test desktop"]
fn shell_paste_native_local_cut_completes_on_original_sta() {
    let _serial = CLIPBOARD_TEST
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let fixture = tempfile::Builder::new()
        .prefix("miv-shell-cut-")
        .tempdir()
        .unwrap();
    let source = fixture.path().join("cut-source.txt");
    let destination = fixture.path().join("destination");
    let payload = b"mIV LocalCutDataObject original apartment fixture";
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(&source, payload).unwrap();
    let mut owner = ClipboardSta::start(source.clone(), true);
    let observations = owner.observations.as_ref().unwrap().clone();
    let mut service = ShellPasteService::default();
    let invocation = service
        .submit(owner.hwnd, destination.clone(), egui::Context::default())
        .expect("submit mIV local cut");
    invocation
        .recv_timeout(NATIVE_TIMEOUT)
        .expect("cut InvokeCommand timed out")
        .expect("cut InvokeCommand failed");
    // A fast Shell may already have notified during InvokeCommand. Do not require a
    // race-dependent callback ordering; continue pumping both STAs until transfer and
    // the actual production SetData completion observation are both present.
    assert!(
        wait_for_transfer(|| {
            !source.exists()
                && std::fs::read(destination.join("cut-source.txt"))
                    .is_ok_and(|bytes| bytes == payload)
                && observations
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .iter()
                    .any(|(signal, _)| {
                        matches!(
                            signal,
                            ClipboardTransferSignal::PerformedMove
                                | ClipboardTransferSignal::PerformedOptimizedMove
                                | ClipboardTransferSignal::PasteSucceededMove
                                | ClipboardTransferSignal::LogicalMove
                        )
                    })
        }),
        "Shell move or LocalCutDataObject SetData completion did not arrive"
    );
    let seen = observations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    assert!(!seen.is_empty());
    assert!(
        seen.iter().all(|(_, thread)| *thread == owner.thread_id),
        "Shell completion must be marshaled to the original UI STA: {seen:?}"
    );
    assert_ne!(owner.thread_id, thread::current().id());
    assert!(owner.finish(), "clipboard STA did not finish cleanup");
}
