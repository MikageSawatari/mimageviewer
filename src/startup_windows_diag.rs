//! Opt-in, observation-only startup HWND diagnostics. Never used for placement decisions.

use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;
use windows_sys::Win32::Foundation::{FILETIME, GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows_sys::Win32::System::SystemInformation::{
    GetSystemTimePreciseAsFileTime, GetTickCount64,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, GetProcessTimes, GetStartupInfoW,
    STARTUPINFOW,
};
use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const CAPTURE_US: i64 = 10_000_000;
const AFTER_VISIBLE_US: i64 = 1_000_000;
const MAX_RECORDS: usize = 16_384;
const CLOSED: usize = 1usize << (usize::BITS - 1);
static SESSION: OnceLock<Session> = OnceLock::new();

struct Session {
    clock: Clock,
    tx: SyncSender<Record>,
    gate: AtomicUsize,
    ui_hooks: [AtomicUsize; 2],
    log_path: OnceLock<PathBuf>,
    visible_qpc: AtomicI64,
    dropped: AtomicUsize,
    stop: AtomicBool,
}

struct Recording<'a>(&'a AtomicUsize);

impl Drop for Recording<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

fn acquire(gate: &AtomicUsize) -> Option<Recording<'_>> {
    let acquired = gate
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
            (value & CLOSED == 0).then_some(value + 1)
        })
        .is_ok();
    if acquired {
        Some(Recording(gate))
    } else {
        None
    }
}

fn recording() -> Option<Recording<'static>> {
    let session = SESSION.get()?;
    if session.gate.load(Ordering::Acquire) & CLOSED != 0 {
        return None;
    }
    if capture_finished(
        session.clock,
        qpc(),
        session.visible_qpc.load(Ordering::Acquire),
        false,
    ) {
        return None;
    }
    acquire(&session.gate)
}

fn capture_finished(clock: Clock, now: i64, visible: i64, stopped: bool) -> bool {
    stopped
        || clock.entry_us(now) >= CAPTURE_US
        || (visible != 0 && clock.entry_us(now) - clock.entry_us(visible) >= AFTER_VISIBLE_US)
}

#[derive(Clone, Copy)]
struct Clock {
    entry_qpc: i64,
    frequency: i64,
    process_age_us: i64,
}

impl Clock {
    fn entry_us(self, qpc: i64) -> i64 {
        ((qpc - self.entry_qpc) as i128 * 1_000_000 / self.frequency as i128) as i64
    }

    fn process_us(self, qpc: i64) -> i64 {
        self.process_age_us + self.entry_us(qpc)
    }
}

fn qpc() -> i64 {
    let mut value = 0;
    unsafe { QueryPerformanceCounter(&mut value) };
    value
}

fn filetime(value: FILETIME) -> u64 {
    ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
}

/// The only startup-option scan; no hooks, clock APIs, files or threads when disabled.
pub fn requested_from(args: &[OsString]) -> bool {
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--" {
            break;
        }
        if args[i] == "--diag-startup-windows" {
            return true;
        }
        i += if args[i].to_str().is_some_and(crate::cli_flag_takes_value) {
            2
        } else {
            1
        };
    }
    false
}

/// Owns the worker until run_native returns. Drop only joins after the UI loop has ended.
pub struct Guard(Option<JoinHandle<()>>);

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(worker) = self.0.take() {
            SESSION.get().unwrap().stop.store(true, Ordering::Release);
            let _ = worker.join();
        }
    }
}

/// Called at core entry, before any GUI/event-loop creation or startup workers.
pub fn start() -> Option<Guard> {
    if !requested_from(&std::env::args_os().collect::<Vec<_>>()) {
        return None;
    }
    let entry_qpc = qpc();
    let mut frequency = 0;
    let mut info = STARTUPINFOW::default();
    let mut now = FILETIME::default();
    let mut creation = FILETIME::default();
    let mut unused = [FILETIME::default(); 3];
    let creation_known;
    unsafe {
        QueryPerformanceFrequency(&mut frequency);
        GetStartupInfoW(&mut info);
        creation_known = GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut unused[0] as *mut _,
            &mut unused[1] as *mut _,
            &mut unused[2] as *mut _,
        ) != 0;
        GetSystemTimePreciseAsFileTime(&mut now);
    }
    if frequency <= 0 {
        return None;
    }
    // Windows exposes creation FILETIME, not a documented creation QPC. Calibrate once;
    // retain raw QPC + entry-relative time as the precise ordering authority.
    let calibration_qpc = qpc();
    let calibration_age = if creation_known {
        filetime(now).saturating_sub(filetime(creation)) as i64 / 10
    } else {
        0
    };
    let clock = Clock {
        entry_qpc,
        frequency,
        process_age_us: calibration_age
            - ((calibration_qpc - entry_qpc) as i128 * 1_000_000 / frequency as i128) as i64,
    };
    let (tx, rx) = mpsc::sync_channel(2048);
    if SESSION
        .set(Session {
            clock,
            tx,
            gate: AtomicUsize::new(0),
            ui_hooks: [AtomicUsize::new(0), AtomicUsize::new(0)],
            log_path: OnceLock::new(),
            visible_qpc: AtomicI64::new(0),
            dropped: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
        })
        .is_err()
    {
        return None;
    }
    let header = json!({
        "schema": 1, "event": "session", "pid": unsafe { GetCurrentProcessId() },
        "ui_thread_id": unsafe { GetCurrentThreadId() }, "qpc_frequency": frequency,
        "entry_qpc": entry_qpc, "process_creation_filetime": filetime(creation),
        "process_creation_known": creation_known, "entry_process_us_estimate": clock.process_age_us,
        "startup_dwFlags": info.dwFlags, "startup_wShowWindow": info.wShowWindow,
        "capture_limit_us": CAPTURE_US, "after_visible_us": AFTER_VISIBLE_US,
        "time_contract": "qpc/entry_us exact; t_us estimated process-relative FILETIME calibration; WinEvent event_us_estimate has millisecond precision",
        "paint_contract": "first_paint.call_returned is the first post_rendering boundary; a skipped paint/surface acquire can also reach it; it is not proof of successful presentation",
        "observation_limit": "CBT/message hooks are notifications, not interception of all ShowWindow calls; CBT MINMAX cmd_show is the effective SW_ low word, not necessarily the caller argument; WinEvent snapshot is at delivery; foreground is process-filtered"
    });
    let (ready_tx, ready_rx) = mpsc::channel();
    let worker = match std::thread::Builder::new()
        .name("startup-window-diag".into())
        .spawn(move || worker(rx, ready_tx, header))
    {
        Ok(worker) => worker,
        Err(error) => {
            SESSION.get().unwrap().gate.store(CLOSED, Ordering::Release);
            eprintln!("startup window diagnostics: worker unavailable: {error}");
            return None;
        }
    };
    // Startup handshake before the event loop/UI exists: otherwise early creation can
    // race hook registration. No waiting, file writes or worker joins in App::update.
    let _ = ready_rx.recv();
    let cbt = unsafe {
        SetWindowsHookExW(
            WH_CBT,
            Some(cbt_hook),
            std::ptr::null_mut(),
            GetCurrentThreadId(),
        )
    };
    let cbt_error = if cbt.is_null() {
        unsafe { GetLastError() }
    } else {
        0
    };
    let message = unsafe {
        SetWindowsHookExW(
            WH_CALLWNDPROC,
            Some(message_hook),
            std::ptr::null_mut(),
            GetCurrentThreadId(),
        )
    };
    let message_error = if message.is_null() {
        unsafe { GetLastError() }
    } else {
        0
    };
    let session = SESSION.get().unwrap();
    session.ui_hooks[0].store(cbt as usize, Ordering::Release);
    session.ui_hooks[1].store(message as usize, Ordering::Release);
    // If startup was suspended beyond the diagnostic cap during installation,
    // the installer still owns any handle the worker has already passed over.
    if session.gate.load(Ordering::Acquire) & CLOSED != 0 {
        for hook in &session.ui_hooks {
            let hook = hook.swap(0, Ordering::AcqRel);
            if hook != 0 {
                unsafe { UnhookWindowsHookEx(hook as HHOOK) };
            }
        }
    }
    mark(
        "hooks.ui.ready",
        0,
        || json!({"cbt_error": cbt_error, "message_error": message_error}),
    );
    eframe::startup_window_observer::install(eframe_milestone);
    Some(Guard(Some(worker)))
}

pub fn enabled() -> bool {
    SESSION
        .get()
        .is_some_and(|session| session.gate.load(Ordering::Relaxed) & CLOSED == 0)
}

fn send(item: Record, _recording: &Recording<'_>) {
    let session = SESSION.get().unwrap();
    if session.tx.try_send(item).is_err() {
        session.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// Detail construction and all native calls are skipped when disabled/finished.
pub fn mark(event: &'static str, hwnd: usize, detail: impl FnOnce() -> Value) {
    if let Some(recording) = recording() {
        send(
            record("milestone", event, hwnd, detail(), false),
            &recording,
        );
    }
}

fn eframe_milestone(event: &'static str, hwnd: usize) {
    mark(event, hwnd, || json!({"requested_visible": true}));
    if event == "eframe.set_visible.complete"
        && let Some(session) = SESSION.get()
    {
        let _ =
            session
                .visible_qpc
                .compare_exchange(0, qpc(), Ordering::Release, Ordering::Relaxed);
    }
}

pub fn set_log_dir(path: PathBuf) {
    if let Some(session) = SESSION.get() {
        let _ = session.log_path.set(path.join("startup-windows.log"));
    }
}

#[derive(Clone, Serialize)]
struct Snapshot {
    sample_qpc: i64,
    valid: bool,
    top_level: bool,
    class: String,
    title: Option<String>,
    rect: Option<[i32; 4]>,
    style: u32,
    exstyle: u32,
    ws_visible: bool,
    ws_maximize: bool,
    layered: bool,
    toolwindow: bool,
    visible: bool,
    cloaked: Option<u32>,
    dpi: u32,
    owner_thread_id: u32,
    owner_pid: u32,
    foreground_hwnd: String,
}

// Never hold a lock/RefCell borrow across USER32 or DWM calls: hooks may reenter.
fn snapshot(hwnd: usize, enrich: bool, query_title: bool) -> Snapshot {
    let hwnd = hwnd as HWND;
    let mut class = [0u16; 256];
    let mut rect = RECT::default();
    unsafe {
        let sample_qpc = qpc();
        let valid = !hwnd.is_null() && IsWindow(hwnd) != 0;
        let mut owner_pid = 0;
        let owner_thread_id = GetWindowThreadProcessId(hwnd, &mut owner_pid);
        let own_process = owner_pid == GetCurrentProcessId();
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let class_len = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
        let title = if enrich && query_title && valid && own_process {
            let mut title = [0u16; 512];
            let mut result = 0;
            // GetWindowText on our process can synchronously send WM_GETTEXT. Bound that
            // wait on the diagnostic worker; never query title through messages on UI.
            if SendMessageTimeoutW(
                hwnd,
                WM_GETTEXT,
                title.len(),
                title.as_mut_ptr() as isize,
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                20,
                &mut result,
            ) != 0
            {
                Some(String::from_utf16_lossy(
                    &title[..(result as usize).min(title.len() - 1)],
                ))
            } else {
                None
            }
        } else {
            None
        };
        let mut cloaked = 0u32;
        let cloak_known = enrich
            && valid
            && own_process
            && DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED as u32,
                &mut cloaked as *mut _ as *mut _,
                std::mem::size_of::<u32>() as u32,
            ) >= 0;
        Snapshot {
            sample_qpc,
            valid,
            top_level: valid
                && own_process
                && style & WS_CHILD == 0
                && GetParent(hwnd) != HWND_MESSAGE,
            class: String::from_utf16_lossy(&class[..class_len]),
            title,
            rect: (GetWindowRect(hwnd, &mut rect) != 0).then_some([
                rect.left,
                rect.top,
                rect.right,
                rect.bottom,
            ]),
            style,
            exstyle,
            ws_visible: style & WS_VISIBLE != 0,
            ws_maximize: style & WS_MAXIMIZE != 0,
            layered: exstyle & WS_EX_LAYERED != 0,
            toolwindow: exstyle & WS_EX_TOOLWINDOW != 0,
            visible: IsWindowVisible(hwnd) != 0,
            cloaked: cloak_known.then_some(cloaked),
            dpi: GetDpiForWindow(hwnd),
            owner_thread_id,
            owner_pid,
            foreground_hwnd: format!("{:#x}", GetForegroundWindow() as usize),
        }
    }
}

#[derive(Serialize)]
struct Record {
    qpc: i64,
    t_us: i64,
    entry_us: i64,
    source: &'static str,
    event: &'static str,
    hwnd: String,
    thread_id: u32,
    detail: Value,
    snapshot: Option<Snapshot>,
    delivery_snapshot: Option<Snapshot>,
    cached_identity: Option<Snapshot>,
    native_generation: Option<u64>,
    #[serde(skip)]
    raw_hwnd: usize,
}

fn record(
    source: &'static str,
    event: &'static str,
    hwnd: usize,
    detail: Value,
    enrich: bool,
) -> Record {
    let time = qpc();
    let clock = SESSION.get().unwrap().clock;
    Record {
        qpc: time,
        t_us: clock.process_us(time),
        entry_us: clock.entry_us(time),
        source,
        event,
        hwnd: format!("{hwnd:#x}"),
        thread_id: unsafe { GetCurrentThreadId() },
        detail,
        snapshot: (hwnd != 0).then(|| snapshot(hwnd, enrich, enrich)),
        delivery_snapshot: None,
        cached_identity: None,
        native_generation: None,
        raw_hwnd: hwnd,
    }
}

unsafe fn create_detail(cs: &CREATESTRUCTW, unicode: bool) -> Value {
    // CREATESTRUCT strings may be integer atoms; don't dereference those.
    let title = if unicode && cs.lpszName as usize > 0xffff {
        let mut length = 0;
        while length < 511 && unsafe { *cs.lpszName.add(length) } != 0 {
            length += 1;
        }
        Some(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(cs.lpszName, length)
        }))
    } else {
        None
    };
    json!({"creation_style": cs.style as u32, "creation_exstyle": cs.dwExStyle,
        "creation_parent": format!("{:#x}", cs.hwndParent as usize),
        "creation_xywh": [cs.x, cs.y, cs.cx, cs.cy], "creation_title": title,
        "geometry_contract": "creation request; parent/position/size not final"})
}

unsafe extern "system" fn cbt_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0
        && let Some(recording) = recording()
    {
        let event = match code as u32 {
            HCBT_CREATEWND => Some("HCBT_CREATEWND"),
            HCBT_MINMAX => Some("HCBT_MINMAX"),
            HCBT_ACTIVATE => Some("HCBT_ACTIVATE"),
            HCBT_DESTROYWND => Some("HCBT_DESTROYWND"),
            HCBT_MOVESIZE => Some("HCBT_MOVESIZE"),
            _ => None,
        };
        if let Some(event) = event {
            let detail = if code as u32 == HCBT_CREATEWND && lp != 0 {
                let cs = unsafe { &*(*(lp as *const CBT_CREATEWNDW)).lpcs };
                if cs.style as u32 & WS_CHILD != 0 || cs.hwndParent == HWND_MESSAGE {
                    return unsafe { CallNextHookEx(std::ptr::null_mut(), code, wp, lp) };
                }
                unsafe { create_detail(cs, IsWindowUnicode(wp as HWND) != 0) }
            } else if code as u32 == HCBT_MINMAX {
                json!({"cmd_show": (lp as u32) & 0xffff, "contract": "effective SW_ notification, not intercepted caller argument"})
            } else {
                json!({})
            };
            let item = record("cbt.before", event, wp, detail, false);
            if code as u32 == HCBT_CREATEWND || item.snapshot.as_ref().is_some_and(|s| s.top_level)
            {
                send(item, &recording);
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wp, lp) }
}

unsafe extern "system" fn message_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0
        && lp != 0
        && let Some(recording) = recording()
    {
        let msg = unsafe { &*(lp as *const CWPSTRUCT) };
        let event = match msg.message {
            WM_NCCREATE => Some("WM_NCCREATE"),
            WM_CREATE => Some("WM_CREATE"),
            WM_SHOWWINDOW => Some("WM_SHOWWINDOW"),
            WM_WINDOWPOSCHANGING => Some("WM_WINDOWPOSCHANGING"),
            WM_WINDOWPOSCHANGED => Some("WM_WINDOWPOSCHANGED"),
            WM_SIZE => Some("WM_SIZE"),
            WM_DESTROY => Some("WM_DESTROY"),
            _ => None,
        };
        if let Some(event) = event {
            let detail = match msg.message {
                WM_NCCREATE | WM_CREATE if msg.lParam != 0 => unsafe {
                    create_detail(
                        &*(msg.lParam as *const CREATESTRUCTW),
                        IsWindowUnicode(msg.hwnd) != 0,
                    )
                },
                WM_WINDOWPOSCHANGING | WM_WINDOWPOSCHANGED if msg.lParam != 0 => {
                    let pos = unsafe { &*(msg.lParam as *const WINDOWPOS) };
                    json!({"xywh": [pos.x, pos.y, pos.cx, pos.cy], "flags": pos.flags,
                        "show_window": pos.flags & SWP_SHOWWINDOW != 0, "hide_window": pos.flags & SWP_HIDEWINDOW != 0,
                        "no_activate": pos.flags & SWP_NOACTIVATE != 0})
                }
                _ => json!({"wparam": msg.wParam, "lparam": msg.lParam}),
            };
            let item = record("wndproc.before", event, msg.hwnd as usize, detail, false);
            if item.snapshot.as_ref().is_some_and(|s| s.top_level) {
                send(item, &recording);
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wp, lp) }
}

unsafe extern "system" fn win_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    event_thread: u32,
    tick: u32,
) {
    if hwnd.is_null()
        || (event != EVENT_SYSTEM_FOREGROUND && (object != OBJID_WINDOW || child != 0))
    {
        return;
    }
    let Some(recording) = recording() else { return };
    let name = match event {
        EVENT_OBJECT_CREATE => "CREATE",
        EVENT_OBJECT_SHOW => "SHOW",
        EVENT_OBJECT_HIDE => "HIDE",
        EVENT_OBJECT_DESTROY => "DESTROY",
        EVENT_OBJECT_LOCATIONCHANGE => "LOCATIONCHANGE",
        EVENT_SYSTEM_FOREGROUND => "FOREGROUND",
        _ => return,
    };
    let event_age_ms = (unsafe { GetTickCount64() } as u32).wrapping_sub(tick);
    let mut item = record(
        "winevent.delivery",
        name,
        hwnd as usize,
        json!({
            "event_thread_id": event_thread, "event_tick_ms": tick, "delivery_age_ms_estimate": event_age_ms,
            "snapshot_contract": "current at asynchronous delivery; not historical event state"
        }),
        false,
    );
    item.detail["event_us_estimate"] = json!(item.t_us - event_age_ms as i64 * 1000);
    send(item, &recording);
}

#[derive(Default)]
struct NativeGenerations {
    next: u64,
    current: HashMap<usize, u64>,
}

impl NativeGenerations {
    fn observe(&mut self, source: &str, event: &str, hwnd: usize) -> Option<u64> {
        // Out-of-context events cannot identify a historical HWND generation. They
        // must never create, destroy or reset the UI thread's synchronous identity.
        if source == "winevent.delivery" {
            return None;
        }
        if event == "HCBT_CREATEWND" {
            self.next += 1;
            self.current.insert(hwnd, self.next);
        }
        self.current.get(&hwnd).copied()
    }
}

#[derive(Default)]
struct Collector {
    generations: NativeGenerations,
    native: HashMap<usize, Snapshot>,
    delivery: HashMap<usize, Snapshot>,
    records: Vec<Record>,
}

fn retain_window_record(
    source: &str,
    event: &str,
    valid: bool,
    top_level: bool,
    known_native: bool,
) -> bool {
    source == "winevent.delivery"
        || event == "HCBT_CREATEWND"
        || if valid { top_level } else { known_native }
}

impl Collector {
    fn push(&mut self, mut item: Record, enrich: bool) {
        let session = SESSION.get().unwrap();
        if self.records.len() >= MAX_RECORDS {
            session.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let id = item.raw_hwnd;
        if let Some(current) = item.snapshot.as_ref() {
            if !retain_window_record(
                item.source,
                item.event,
                current.valid,
                current.top_level,
                self.native.contains_key(&id),
            ) {
                return;
            }
            if item.source == "winevent.delivery" {
                // Short-lived worker windows can be gone/reused before all their
                // notifications arrive. Keep bare process-filtered evidence.
                item.detail["scope_contract"] = json!(if current.top_level {
                    "top-level at callback delivery; historical scope not proven"
                } else {
                    "event scope unknown at delivery (destroyed/reused/child); bare own-process OBJID_WINDOW notification"
                });
            }
            item.native_generation = self.generations.observe(item.source, item.event, id);
            if item.event == "HCBT_CREATEWND" {
                self.native.remove(&id);
                self.delivery.remove(&id);
            }
            item.cached_identity = self
                .native
                .get(&id)
                .or_else(|| self.delivery.get(&id))
                .cloned();
            if item.source == "winevent.delivery" {
                item.detail["identity_contract"] = json!(
                    "event generation unknown; snapshot is current at delivery; cached identity is last observed, not historical proof"
                );
            }
            if enrich {
                let needs_title = item
                    .detail
                    .get("creation_title")
                    .and_then(Value::as_str)
                    .is_none()
                    && item
                        .cached_identity
                        .as_ref()
                        .and_then(|s| s.title.as_ref())
                        .is_none();
                item.delivery_snapshot = Some(snapshot(id, true, needs_title));
            }
            if item.source == "winevent.delivery" {
                // Keep the worker's later sample intact; do not splice its title into
                // the earlier callback state or the synchronous UI generation.
                if let Some(delivery) = item.delivery_snapshot.as_ref().filter(|s| s.top_level) {
                    self.delivery.insert(id, delivery.clone());
                }
            } else if current.valid || item.event == "HCBT_CREATEWND" {
                let mut identity = current.clone();
                identity.title = item
                    .detail
                    .get("creation_title")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| self.native.get(&id).and_then(|s| s.title.clone()));
                self.native.insert(id, identity);
            }
        }
        self.records.push(item);
    }
}

fn worker(rx: Receiver<Record>, ready: mpsc::Sender<()>, header: Value) {
    let session = SESSION.get().unwrap();
    let mut hooks = Vec::new();
    let mut hook_results = Vec::new();
    for (min, max) in [
        (EVENT_OBJECT_CREATE, EVENT_OBJECT_LOCATIONCHANGE),
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
    ] {
        let hook = unsafe {
            SetWinEventHook(
                min,
                max,
                std::ptr::null_mut(),
                Some(win_event),
                GetCurrentProcessId(),
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        hook_results.push(json!({"min": min, "max": max, "error": if hook.is_null() { unsafe { GetLastError() } } else { 0 }}));
        if !hook.is_null() {
            hooks.push(hook);
        }
    }
    let mut collector = Collector::default();
    collector.push(
        record(
            "diagnostic",
            "hooks.winevent.ready",
            0,
            json!({"hooks": hook_results}),
            false,
        ),
        false,
    );
    let _ = ready.send(());
    let finished = || {
        capture_finished(
            session.clock,
            qpc(),
            session.visible_qpc.load(Ordering::Acquire),
            session.stop.load(Ordering::Acquire),
        )
    };
    while !finished() {
        // Worker-only finite waits. Bound pumping and enrichment work; all producers
        // also stop sampling at the deadline, even during a native message burst.
        unsafe {
            MsgWaitForMultipleObjectsEx(0, std::ptr::null(), 10, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        let mut msg = MSG::default();
        for _ in 0..32 {
            if finished()
                || unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } == 0
            {
                break;
            }
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        for _ in 0..64 {
            if finished() {
                break;
            }
            let Ok(item) = rx.try_recv() else { break };
            collector.push(item, true);
        }
    }
    // One atomic gate owns both admission and in-flight producers. No accepted
    // record can arrive after the final drain; producers never wait on the UI.
    session.gate.fetch_or(CLOSED, Ordering::AcqRel);
    for hook in hooks {
        unsafe { UnhookWinEvent(hook) };
    }
    for hook in &session.ui_hooks {
        let hook = hook.swap(0, Ordering::AcqRel);
        if hook != 0 {
            unsafe { UnhookWindowsHookEx(hook as HHOOK) };
        }
    }
    while session.gate.load(Ordering::Acquire) != CLOSED {
        while let Ok(item) = rx.try_recv() {
            collector.push(item, false);
        }
        // Only diagnostic worker waits for already-running callbacks. No enrichment
        // or title queries during shutdown; accepted callbacks contain their snapshot.
        unsafe {
            MsgWaitForMultipleObjectsEx(0, std::ptr::null(), 1, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
    }
    while let Ok(item) = rx.try_recv() {
        collector.push(item, false);
    }
    let footer = json!({"event": "session.end", "t_us": session.clock.process_us(qpc()),
        "records": collector.records.len(), "dropped": session.dropped.load(Ordering::Relaxed),
        "main_visible_commit_recorded": session.visible_qpc.load(Ordering::Acquire) != 0,
        "stop_reason": if session.stop.load(Ordering::Relaxed) { "core_returned" } else if session.visible_qpc.load(Ordering::Acquire) != 0 { "visible_commit_plus_one_second_or_cap" } else { "ten_second_cap" }});
    if let Some(path) = session.log_path.get() {
        if let Err(error) = write_log(path, &header, &mut collector.records, &footer) {
            crate::logger::log(format!(
                "startup window diagnostics: could not write {}: {error}",
                path.display()
            ));
        }
    } else {
        eprintln!(
            "startup window diagnostics: data directory was not supplied before capture ended"
        );
    }
}

fn write_log(
    path: &std::path::Path,
    header: &Value,
    records: &mut [Record],
    footer: &Value,
) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut output = std::io::BufWriter::new(std::fs::File::create(path)?);
    records.sort_by_key(|record| record.qpc);
    for line in std::iter::once(serde_json::to_string(header)?)
        .chain(
            records
                .iter()
                .map(|record| serde_json::to_string(record).unwrap()),
        )
        .chain(std::iter::once(serde_json::to_string(footer)?))
    {
        writeln!(output, "{line}")?;
    }
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_windows_flag_respects_delimiter_and_option_values() {
        let requested =
            |args: &[&str]| requested_from(&args.iter().map(OsString::from).collect::<Vec<_>>());
        assert!(requested(&[
            "miv",
            "--diag-startup-windows",
            "--data-dir",
            "fresh"
        ]));
        assert!(!requested(&["miv", "--", "--diag-startup-windows"]));
        assert!(!requested(&["miv", "--data-dir", "--diag-startup-windows"]));
        assert!(!requested(&["miv", "folder"]));
    }

    #[test]
    fn startup_windows_qpc_clock_preserves_microsecond_order() {
        let clock = Clock {
            entry_qpc: 10_000,
            frequency: 10_000_000,
            process_age_us: 123_456,
        };
        assert_eq!(clock.entry_us(10_010), 1);
        assert_eq!(clock.process_us(10_020), 123_458);
    }

    #[test]
    fn startup_windows_gate_closes_admission_before_final_drain() {
        let gate = AtomicUsize::new(0);
        let permit = acquire(&gate).unwrap();
        assert_eq!(gate.fetch_or(CLOSED, Ordering::AcqRel), 1);
        assert!(acquire(&gate).is_none());
        assert_eq!(gate.load(Ordering::Acquire), CLOSED | 1);
        assert_ne!(gate.load(Ordering::Acquire), CLOSED);
        drop(permit);
        assert_eq!(gate.load(Ordering::Acquire), CLOSED);
        assert!(acquire(&gate).is_none());
        assert_eq!(gate.load(Ordering::Acquire), CLOSED);
    }

    #[test]
    fn startup_windows_late_winevents_cannot_change_native_generation() {
        let mut generations = NativeGenerations::default();
        assert_eq!(
            generations.observe("cbt.before", "HCBT_CREATEWND", 42),
            Some(1)
        );
        assert_eq!(
            generations.observe("cbt.before", "HCBT_DESTROYWND", 42),
            Some(1)
        );
        assert_eq!(
            generations.observe("cbt.before", "HCBT_CREATEWND", 42),
            Some(2)
        );
        assert_eq!(generations.observe("winevent.delivery", "CREATE", 42), None);
        assert_eq!(
            generations.observe("winevent.delivery", "DESTROY", 42),
            None
        );
        assert_eq!(
            generations.observe("wndproc.before", "WM_SHOWWINDOW", 42),
            Some(2)
        );
    }

    #[test]
    fn startup_windows_capture_ends_after_commit_or_absolute_cap() {
        let clock = Clock {
            entry_qpc: 1,
            frequency: 1_000_000,
            process_age_us: 0,
        };
        assert!(!capture_finished(clock, 999_999, 0, false));
        assert!(!capture_finished(clock, 1_999_999, 1_000_000, false));
        assert!(capture_finished(clock, 2_000_000, 1_000_000, false));
        assert!(capture_finished(clock, 10_000_001, 0, false));
        assert!(capture_finished(clock, 2, 0, true));
    }

    #[test]
    fn startup_windows_keeps_bare_late_notifications_for_unknown_windows() {
        assert!(retain_window_record(
            "winevent.delivery",
            "CREATE",
            false,
            false,
            false
        ));
        assert!(retain_window_record(
            "winevent.delivery",
            "SHOW",
            false,
            false,
            false
        ));
        assert!(retain_window_record(
            "winevent.delivery",
            "DESTROY",
            true,
            false,
            true
        ));
        assert!(!retain_window_record(
            "wndproc.before",
            "WM_SHOWWINDOW",
            true,
            false,
            false
        ));
        assert!(retain_window_record(
            "cbt.before",
            "HCBT_CREATEWND",
            false,
            false,
            false
        ));
    }

    #[test]
    fn startup_windows_log_is_sorted_and_json_escapes_titles() {
        let temp = tempfile::tempdir().unwrap();
        let mut records = [2, 1].map(|time| Record {
            qpc: time,
            t_us: time,
            entry_us: time,
            source: "cbt.before",
            event: "HCBT_CREATEWND",
            hwnd: "0x123".into(),
            thread_id: 7,
            detail: json!({"creation_title": "日本語\n\"title\""}),
            snapshot: None,
            delivery_snapshot: None,
            cached_identity: None,
            native_generation: None,
            raw_hwnd: 0x123,
        });
        let path = temp.path().join("logs/startup-windows.log");
        write_log(
            &path,
            &json!({"event": "session"}),
            &mut records,
            &json!({"event": "session.end"}),
        )
        .unwrap();
        let contents = std::fs::read_to_string(path).unwrap();
        let lines = contents
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[1]["qpc"], 1);
        assert_eq!(lines[2]["detail"]["creation_title"], "日本語\n\"title\"");
        assert!(lines[1].get("raw_hwnd").is_none());
    }
}
