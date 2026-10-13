//! Before-queue foreground facts for main-window mouse presses.
//!
//! WH_MOUSE_LL runs on its installing thread. Each registration has its own
//! message loop, callback TLS, bounded channel, and stop event; neither callback
//! state nor worker shutdown borrows the UI observer.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};

use windows::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SetLastError,
    WAIT_FAILED, WAIT_OBJECT_0, WIN32_ERROR, WPARAM,
};
use windows::Win32::Graphics::Gdi::{PtInRect, ScreenToClient};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GA_ROOT, GW_OWNER, GetAncestor, GetClientRect,
    GetForegroundWindow, GetWindow, GetWindowThreadProcessId, HHOOK, IsWindow, MSG, MSLLHOOKSTRUCT,
    MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_NOREMOVE, PM_REMOVE, PeekMessageW,
    QS_ALLINPUT, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_MOUSE_LL,
    WM_LBUTTONDOWN, WM_QUIT, WindowFromPoint,
};

const MAX_PATH: usize = 64;
const RECORD_CAPACITY: usize = 512;

/// Packet metadata, not an assumed unique identifier or a timing threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct PacketKey {
    pub time: u32,
    pub x: i32,
    pub y: i32,
    pub extra: usize,
}

#[derive(Clone, Debug)]
pub(crate) enum ForegroundFact {
    Unknown,
    /// A native contact/keyboard observation resolved by the existing owner-chain path.
    Window(u64),
    BeforeInput(ForegroundPath),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ForegroundRelation {
    InFront,
    Other,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct WindowIdentity {
    hwnd: u64,
    pid: u32,
}

/// Fixed-size immutable foreground/root/owner-chain observation. No HWND is
/// dereferenced later to reconstruct the foreground that existed before input.
#[derive(Clone)]
pub(crate) struct ForegroundPath {
    nodes: [WindowIdentity; MAX_PATH],
    len: usize,
    complete: bool,
}

impl std::fmt::Debug for ForegroundPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ForegroundPath")
            .field("nodes", &&self.nodes[..self.len])
            .field("complete", &self.complete)
            .finish()
    }
}

impl ForegroundPath {
    pub(crate) fn relation(&self, editor_hwnd: u64, editor_pid: u32) -> ForegroundRelation {
        if !self.complete || self.len == 0 || editor_hwnd == 0 || editor_pid == 0 {
            return ForegroundRelation::Unknown;
        }
        let nodes = &self.nodes[..self.len];
        if nodes.iter().any(|node| node.hwnd == 0 || node.pid == 0)
            || nodes
                .iter()
                .any(|node| node.hwnd == editor_hwnd && node.pid != editor_pid)
        {
            return ForegroundRelation::Unknown;
        }
        if nodes
            .iter()
            .any(|node| node.hwnd == editor_hwnd && node.pid == editor_pid)
        {
            ForegroundRelation::InFront
        } else {
            ForegroundRelation::Other
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(nodes: &[(u64, u32)], complete: bool) -> Self {
        let mut path = Self {
            nodes: [WindowIdentity::default(); MAX_PATH],
            len: nodes.len().min(MAX_PATH),
            complete: complete && nodes.len() <= MAX_PATH,
        };
        for (dst, &(hwnd, pid)) in path.nodes.iter_mut().zip(nodes) {
            *dst = WindowIdentity { hwnd, pid };
        }
        path
    }
}

fn collect_path(
    mut hwnd: u64,
    mut read_node: impl FnMut(u64) -> Option<(u32, u64)>,
) -> ForegroundPath {
    let mut path = ForegroundPath {
        nodes: [WindowIdentity::default(); MAX_PATH],
        len: 0,
        complete: false,
    };
    while hwnd != 0 && path.len < MAX_PATH {
        if path.nodes[..path.len].iter().any(|node| node.hwnd == hwnd) {
            return path;
        }
        let Some((pid, next)) = read_node(hwnd) else {
            return path;
        };
        if pid == 0 {
            return path;
        }
        path.nodes[path.len] = WindowIdentity { hwnd, pid };
        path.len += 1;
        hwnd = next;
        if hwnd == 0 {
            path.complete = true;
        }
    }
    path
}

fn native_node(hwnd: u64) -> Option<(u32, u64)> {
    let handle = HWND(hwnd as usize as *mut _);
    if !unsafe { IsWindow(Some(handle)) }.as_bool() {
        return None;
    }
    let mut pid = 0;
    if unsafe { GetWindowThreadProcessId(handle, Some(&mut pid)) } == 0 || pid == 0 {
        return None;
    }
    let root = unsafe { GetAncestor(handle, GA_ROOT) };
    if root.0.is_null() {
        return None;
    }
    let next = if root != handle {
        root.0 as usize as u64
    } else {
        // A missing owner is a normal terminator. Distinguish it from API failure
        // without leaking changes to this callback thread's last-error value.
        let previous_error = unsafe { GetLastError() };
        unsafe { SetLastError(WIN32_ERROR(0)) };
        let owner = unsafe { GetWindow(handle, GW_OWNER) };
        let owner_error = unsafe { GetLastError() };
        unsafe { SetLastError(previous_error) };
        match owner {
            Ok(owner) => owner.0 as usize as u64,
            Err(_) if owner_error.0 == 0 => 0,
            Err(_) => return None,
        }
    };
    let mut final_pid = 0;
    if !unsafe { IsWindow(Some(handle)) }.as_bool()
        || unsafe { GetWindowThreadProcessId(handle, Some(&mut final_pid)) } == 0
        || final_pid != pid
    {
        return None;
    }
    Some((pid, next))
}

fn foreground_before_input() -> ForegroundFact {
    let foreground = unsafe { GetForegroundWindow() };
    let path = collect_path(foreground.0 as usize as u64, native_node);
    if path.complete {
        ForegroundFact::BeforeInput(path)
    } else {
        ForegroundFact::Unknown
    }
}

#[derive(Clone, Debug)]
pub(super) struct CaptureRecord {
    pub key: PacketKey,
    pub foreground: ForegroundFact,
}

const PENDING: u8 = 0;
const READY: u8 = 1;
const FAILED: u8 = 2;
const STOPPED: u8 = 3;

/// Registration phases never return to Pending/Ready after loss or retirement.
#[derive(Default)]
struct Lifecycle(AtomicU8);

impl Lifecycle {
    fn mark_ready(&self) -> bool {
        self.0
            .compare_exchange(PENDING, READY, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn available(&self) -> bool {
        self.0.load(Ordering::Acquire) == READY
    }

    fn failed(&self) -> bool {
        self.0.load(Ordering::Acquire) == FAILED
    }

    fn finished(&self) -> bool {
        matches!(self.0.load(Ordering::Acquire), FAILED | STOPPED)
    }

    fn fail(&self) {
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |phase| {
                matches!(phase, PENDING | READY).then_some(FAILED)
            });
    }

    fn retire(&self) {
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |phase| {
                matches!(phase, PENDING | READY).then_some(STOPPED)
            });
    }
}

struct Registration {
    lifecycle: Lifecycle,
    stop: HANDLE,
}

// The manual-reset event belongs to this Arc and is only signalled/waited on
// across threads. Its handle is closed after worker and observer release it.
unsafe impl Send for Registration {}
unsafe impl Sync for Registration {}

impl Registration {
    fn wake(&self) {
        let _ = unsafe { SetEvent(self.stop) };
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.stop) };
    }
}

pub(super) struct CaptureWorker {
    registration: Arc<Registration>,
    records: Receiver<CaptureRecord>,
}

impl CaptureWorker {
    pub(super) fn try_recv(&self) -> Result<CaptureRecord, TryRecvError> {
        if self.registration.lifecycle.finished() {
            return Err(TryRecvError::Disconnected);
        }
        if !self.available() {
            return Err(TryRecvError::Empty);
        }
        let record = self.records.try_recv()?;
        // Overflow/loss invalidates even records buffered before the missing packet.
        if !self.available() {
            return Err(TryRecvError::Disconnected);
        }
        Ok(record)
    }

    pub(super) fn available(&self) -> bool {
        self.registration.lifecycle.available()
    }

    pub(super) fn failed(&self) -> bool {
        self.registration.lifecycle.failed()
    }
}

impl Drop for CaptureWorker {
    fn drop(&mut self) {
        self.registration.lifecycle.retire();
        self.registration.wake();
        // Deliberately no join, completion receive, or wait on the UI thread.
    }
}

struct CallbackState {
    main_hwnd: u64,
    registration: Arc<Registration>,
    records: SyncSender<CaptureRecord>,
}

thread_local! {
    static CALLBACK: RefCell<Option<CallbackState>> = const { RefCell::new(None) };
}

fn main_client_press(main_hwnd: u64, point: POINT) -> bool {
    let main = HWND(main_hwnd as usize as *mut _);
    let target = unsafe { WindowFromPoint(point) };
    // The egui toolbar receives input at the exact root HWND. Native video
    // children have their own message producers and never consume this journal.
    if target != main {
        return false;
    }
    let mut client = point;
    let mut rect = RECT::default();
    unsafe { ScreenToClient(main, &mut client) }.as_bool()
        && unsafe { GetClientRect(main, &mut rect) }.is_ok()
        && unsafe { PtInRect(&rect, client) }.as_bool()
}

fn publish_record(
    lifecycle: &Lifecycle,
    sender: &SyncSender<CaptureRecord>,
    record: CaptureRecord,
) -> bool {
    if !lifecycle.available() {
        return false;
    }
    match sender.try_send(record) {
        Ok(()) => true,
        Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
            lifecycle.fail();
            false
        }
    }
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == 0 && wparam.0 == WM_LBUTTONDOWN as usize && lparam.0 != 0 {
        CALLBACK.with(|callback| {
            let Ok(callback) = callback.try_borrow() else {
                return;
            };
            let Some(state) = callback.as_ref() else {
                return;
            };
            if !state.registration.lifecycle.available() {
                return;
            }
            // Valid only during the callback. Copy the metadata and path before returning.
            let packet = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            if packet.dwExtraInfo as u32 & 0xffff_ff00 == 0xff51_5700 {
                return; // Native contacts own promoted pen/touch mouse facts.
            }
            if !main_client_press(state.main_hwnd, packet.pt) {
                return;
            }
            let record = CaptureRecord {
                key: PacketKey {
                    time: packet.time,
                    x: packet.pt.x,
                    y: packet.pt.y,
                    extra: packet.dwExtraInfo,
                },
                foreground: foreground_before_input(),
            };
            // Injected input is intentionally accepted; flags never suppress a packet.
            if !publish_record(&state.registration.lifecycle, &state.records, record) {
                state.registration.wake();
            }
        });
    }
    // Never eat input, including unhandled events and negative hook codes.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

struct HookGuard(HHOOK);

impl Drop for HookGuard {
    fn drop(&mut self) {
        let _ = unsafe { UnhookWindowsHookEx(self.0) };
    }
}

struct WorkerExit<'a>(&'a Lifecycle);

impl Drop for WorkerExit<'_> {
    fn drop(&mut self) {
        // A worker that exits unexpectedly (including unwinding) cannot leave
        // an observer believing that its hook is still installed.
        if !self.0.finished() {
            self.0.fail();
        }
    }
}

fn run_worker(main_hwnd: u64, registration: Arc<Registration>, records: SyncSender<CaptureRecord>) {
    let _exit = WorkerExit(&registration.lifecycle);
    let result = run_message_loop(main_hwnd, &registration, records);
    if let Err(error) = result {
        registration.lifecycle.fail();
        crate::logger::log(format!(
            "EffeTune before-input capture unavailable: {error}"
        ));
    } else if registration.lifecycle.failed() {
        crate::logger::log("EffeTune before-input capture journal lost; foreground is unknown");
    }
    registration.lifecycle.retire();
}

fn run_message_loop(
    main_hwnd: u64,
    registration: &Arc<Registration>,
    records: SyncSender<CaptureRecord>,
) -> Result<(), String> {
    // Create this thread's queue before publishing any installed state.
    let mut message = MSG::default();
    let _ = unsafe { PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE) };
    if registration.lifecycle.finished() {
        return Ok(());
    }
    CALLBACK.with(|callback| {
        *callback.borrow_mut() = Some(CallbackState {
            main_hwnd,
            registration: Arc::clone(registration),
            records,
        });
    });
    // WH_MOUSE_LL calls the installing thread, not foreign-process code. The
    // callback lives in this core module for the whole process lifetime.
    let installation = (|| {
        let module = unsafe { GetModuleHandleW(None) }.map_err(|error| error.to_string())?;
        let hook = unsafe {
            SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(HINSTANCE(module.0)), 0)
        }
        .map_err(|error| format!("WH_MOUSE_LL installation failed: {error}"))?;
        let _hook = HookGuard(hook);
        if !registration.lifecycle.mark_ready() {
            return Ok(());
        }
        while !registration.lifecycle.finished() {
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                if message.message == WM_QUIT {
                    return Err("capture thread received WM_QUIT".to_owned());
                }
                let _ = unsafe { TranslateMessage(&message) };
                unsafe { DispatchMessageW(&message) };
                if registration.lifecycle.finished() {
                    return Ok(());
                }
            }
            // PeekMessage can deliver the hook callback without returning a queued MSG.
            if registration.lifecycle.finished() {
                break;
            }
            let wait = unsafe {
                MsgWaitForMultipleObjectsEx(
                    Some(&[registration.stop]),
                    INFINITE,
                    QS_ALLINPUT,
                    MWMO_INPUTAVAILABLE,
                )
            };
            if wait == WAIT_OBJECT_0 {
                break;
            }
            if wait == WAIT_FAILED {
                return Err(format!("capture message wait failed: {:?}", unsafe {
                    GetLastError()
                }));
            }
        }
        Ok(())
    })();
    // The guard has unhooked before dropping this registration's callback state.
    CALLBACK.with(|callback| *callback.borrow_mut() = None);
    installation
}

pub(super) fn start(main_hwnd: u64) -> Result<CaptureWorker, String> {
    if main_hwnd == 0 {
        return Err("before-input capture requires a main HWND".to_owned());
    }
    let stop = unsafe { CreateEventW(None, true, false, None) }
        .map_err(|error| format!("before-input stop event creation failed: {error}"))?;
    let registration = Arc::new(Registration {
        lifecycle: Lifecycle::default(),
        stop,
    });
    let (sender, records) = mpsc::sync_channel(RECORD_CAPACITY);
    let worker_registration = Arc::clone(&registration);
    std::thread::Builder::new()
        .name("effetune-press-capture".to_owned())
        .spawn(move || run_worker(main_hwnd, worker_registration, sender))
        .map_err(|error| format!("before-input capture thread creation failed: {error}"))?;
    Ok(CaptureWorker {
        registration,
        records,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_path_requires_exact_hwnd_and_pid() {
        let path = ForegroundPath::for_test(&[(11, 1), (22, 2), (33, 3)], true);
        assert_eq!(path.relation(22, 2), ForegroundRelation::InFront);
        assert_eq!(path.relation(44, 2), ForegroundRelation::Other);
        assert_eq!(path.relation(22, 9), ForegroundRelation::Unknown);
        assert_eq!(path.relation(0, 2), ForegroundRelation::Unknown);
    }

    #[test]
    fn foreground_path_keeps_root_and_owner_without_later_queries() {
        let path = collect_path(11, |hwnd| match hwnd {
            11 => Some((1, 22)),
            22 => Some((1, 33)),
            33 => Some((2, 0)),
            _ => None,
        });
        assert!(path.complete);
        assert_eq!(path.relation(33, 2), ForegroundRelation::InFront);
        assert_eq!(path.clone().relation(44, 2), ForegroundRelation::Other);
    }

    #[test]
    fn foreground_path_null_invalid_cycle_and_truncation_are_unknown() {
        let null = collect_path(0, |_| panic!("null foreground must not be queried"));
        let invalid = collect_path(11, |_| None);
        let cycle = collect_path(11, |_| Some((1, 11)));
        let truncated = collect_path(1, |hwnd| Some((1, hwnd + 1)));
        for path in [null, invalid, cycle, truncated] {
            assert!(!path.complete);
            assert_eq!(path.relation(11, 1), ForegroundRelation::Unknown);
        }
        assert!(collect_path(1, |hwnd| Some((1, if hwnd == 64 { 0 } else { hwnd + 1 }))).complete);
    }

    #[test]
    fn capture_lifecycle_retirement_before_ready_cannot_be_reopened() {
        let lifecycle = Lifecycle::default();
        assert!(!lifecycle.available());
        assert!(!lifecycle.failed());
        lifecycle.retire();
        assert!(!lifecycle.mark_ready());
        assert!(!lifecycle.available());
        assert!(lifecycle.finished());
        lifecycle.fail();
        assert!(!lifecycle.failed());
    }

    fn record(time: u32) -> CaptureRecord {
        CaptureRecord {
            key: PacketKey {
                time,
                x: -5,
                y: 7,
                extra: 9,
            },
            foreground: ForegroundFact::Unknown,
        }
    }

    #[test]
    fn capture_lifecycle_full_channel_invalidates_buffered_records_permanently() {
        let lifecycle = Lifecycle::default();
        let (sender, receiver) = mpsc::sync_channel(1);
        assert!(lifecycle.mark_ready());
        assert!(publish_record(&lifecycle, &sender, record(1)));
        assert!(!publish_record(&lifecycle, &sender, record(2)));
        assert!(lifecycle.failed());
        assert!(!lifecycle.available());
        assert_eq!(receiver.try_recv().unwrap().key.time, 1);
        assert!(!publish_record(&lifecycle, &sender, record(3)));
        assert!(!lifecycle.mark_ready());
        lifecycle.retire();
        assert!(lifecycle.failed());
    }

    #[test]
    fn capture_lifecycle_old_registration_cannot_publish_into_new_registration() {
        let old = Lifecycle::default();
        let new = Lifecycle::default();
        let (old_sender, old_receiver) = mpsc::sync_channel(1);
        let (new_sender, new_receiver) = mpsc::sync_channel(1);
        assert!(old.mark_ready());
        old.retire();
        assert!(new.mark_ready());
        assert!(!publish_record(&old, &old_sender, record(1)));
        assert!(publish_record(&new, &new_sender, record(2)));
        assert!(matches!(old_receiver.try_recv(), Err(TryRecvError::Empty)));
        assert_eq!(new_receiver.try_recv().unwrap().key.time, 2);
    }

    #[test]
    fn capture_lifecycle_disconnection_and_unexpected_exit_are_failures() {
        let disconnected = Lifecycle::default();
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        assert!(disconnected.mark_ready());
        assert!(!publish_record(&disconnected, &sender, record(1)));
        assert!(disconnected.failed());

        let early_exit = Lifecycle::default();
        {
            let _exit = WorkerExit(&early_exit);
        }
        assert!(early_exit.failed());
        assert!(!early_exit.mark_ready());
    }
}
