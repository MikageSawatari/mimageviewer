//! Low-overhead observation of Win32 presentation calls during an F12 transition.
//!
//! The probe snapshots state only after an application-issued presentation call. It never waits
//! for DWM or changes a transition decision.

use std::cell::Cell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    CBT_CREATEWNDW, CBTACTIVATESTRUCT, CHILDID_SELF, CREATESTRUCTW, CWPSTRUCT, CallNextHookEx,
    DestroyWindow, EVENT_OBJECT_HIDE, EVENT_OBJECT_SHOW, EVENT_SYSTEM_FOREGROUND, GA_PARENT,
    GW_HWNDNEXT, GW_OWNER, GetAncestor, GetClassNameW, GetForegroundWindow, GetTopWindow,
    GetWindow, GetWindowRect, GetWindowThreadProcessId, HC_ACTION, HCBT_ACTIVATE, HCBT_CREATEWND,
    HCBT_DESTROYWND, HCBT_MINMAX, HCBT_MOVESIZE, HCBT_SETFOCUS, HCBT_SYSCOMMAND, HHOOK, IsWindow,
    IsWindowVisible, OBJID_WINDOW, SET_WINDOW_POS_FLAGS, SHOW_WINDOW_CMD, STYLESTRUCT,
    SetForegroundWindow, SetWindowPos, SetWindowsHookExW, ShowWindow, UnhookWindowsHookEx,
    WA_ACTIVE, WA_CLICKACTIVE, WA_INACTIVE, WH_CALLWNDPROC, WH_CBT, WINDOWPOS,
    WINEVENT_OUTOFCONTEXT, WM_ACTIVATE, WM_ACTIVATEAPP, WM_CREATE, WM_DESTROY, WM_KILLFOCUS,
    WM_NCACTIVATE, WM_NCCREATE, WM_NCDESTROY, WM_SETFOCUS, WM_SHOWWINDOW, WM_SIZE, WM_STYLECHANGED,
    WM_STYLECHANGING, WM_WINDOWPOSCHANGED, WM_WINDOWPOSCHANGING, WS_MAXIMIZE, WS_VISIBLE,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransitionTarget {
    Main,
    Fullscreen,
    Detached,
    Unknown,
}

impl TransitionTarget {
    fn as_u8(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Main => 1,
            Self::Fullscreen => 2,
            Self::Detached => 3,
        }
    }
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Main,
            2 => Self::Fullscreen,
            3 => Self::Detached,
            _ => Self::Unknown,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Fullscreen => "fullscreen",
            Self::Detached => "detached",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowRole {
    Main,
    Host,
    Presenter,
    Backdrop,
    Hud,
    Other,
}

impl WindowRole {
    fn label(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Host => "host",
            Self::Presenter => "presenter",
            Self::Backdrop => "backdrop",
            Self::Hud => "hud",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowAction {
    Visible,
    Focus,
    Publish,
    Raise,
    Destroy,
    ShowWindow,
    SetWindowPos,
    SetForegroundWindow,
    SetActiveWindow,
    SetFocus,
    DwmCloak,
    OsMessage,
}

impl WindowAction {
    fn label(self) -> &'static str {
        match self {
            Self::Visible => "Visible",
            Self::Focus => "Focus",
            Self::Publish => "Publish",
            Self::Raise => "Raise",
            Self::Destroy => "Destroy",
            Self::ShowWindow => "ShowWindow",
            Self::SetWindowPos => "SetWindowPos",
            Self::SetForegroundWindow => "SetForegroundWindow",
            Self::SetActiveWindow => "SetActiveWindow",
            Self::SetFocus => "SetFocus",
            Self::DwmCloak => "DWMWA_CLOAK",
            Self::OsMessage => "OS",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct KnownWindows {
    pub(crate) main: u64,
    pub(crate) host: u64,
    pub(crate) presenter: u64,
    pub(crate) backdrop: u64,
    pub(crate) hud: u64,
}

static ACTIVE_ID: AtomicU64 = AtomicU64::new(0);
static ACTIVE_TARGET: AtomicU8 = AtomicU8::new(0);
static MAIN_HWND: AtomicU64 = AtomicU64::new(0);
static HOST_HWND: AtomicU64 = AtomicU64::new(0);
static PRESENTER_HWND: AtomicU64 = AtomicU64::new(0);
static BACKDROP_HWND: AtomicU64 = AtomicU64::new(0);
static HUD_HWND: AtomicU64 = AtomicU64::new(0);

// mIV presentation observer (backlog 1.139): all native hook callbacks are transition-scoped
// and enqueue only copied fixed-size data. Formatting and file I/O happen after phase=end.
const INSTRUMENTATION_CAPACITY: usize = 8192;
const CLASS_NAME_CAPACITY: usize = 96;
static INSTRUMENTATION_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static INSTRUMENTATION_DROPPED: AtomicU64 = AtomicU64::new(0);
static CALLWNDPROC_HOOK: AtomicUsize = AtomicUsize::new(0);
static CBT_HOOK: AtomicUsize = AtomicUsize::new(0);
static FOREGROUND_HOOK: AtomicUsize = AtomicUsize::new(0);
static SHOW_HIDE_HOOK: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
enum MessagePayload {
    None,
    Create {
        style: u32,
        ex_style: u32,
        parent: u64,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
    },
    Activate {
        state: u32,
        minimized: bool,
        other: u64,
    },
    ActivateApp {
        active: bool,
        other_thread: u32,
    },
    NcActivate {
        active: bool,
        raw_lparam: isize,
    },
    ShowWindow {
        shown: bool,
        reason: isize,
    },
    Focus {
        other: u64,
    },
    WindowPos {
        insert_after: u64,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    },
    Style {
        index: isize,
        old: u32,
        new: u32,
    },
    Size {
        kind: usize,
        width: u16,
        height: u16,
    },
}

#[derive(Clone, Copy)]
struct WindowIdentity {
    pid: u32,
    tid: u32,
    class_len: u8,
    class_name: [u16; CLASS_NAME_CAPACITY],
}

impl Default for WindowIdentity {
    fn default() -> Self {
        Self {
            pid: 0,
            tid: 0,
            class_len: 0,
            class_name: [0; CLASS_NAME_CAPACITY],
        }
    }
}

#[derive(Clone, Copy)]
enum CbtPayload {
    Create {
        style: u32,
        ex_style: u32,
        parent: u64,
        insert_after: u64,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
    },
    MinMax {
        command: u32,
    },
    Activate {
        other: u64,
        mouse: bool,
    },
    SetFocus {
        other: u64,
    },
    MoveSize {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    },
    SysCommand {
        command: usize,
        raw_lparam: isize,
    },
    Destroy,
}

#[derive(Clone, Copy)]
enum InstrumentationKind {
    Message {
        hwnd: u64,
        msg: u32,
        payload: MessagePayload,
    },
    Cbt {
        hwnd: u64,
        payload: CbtPayload,
    },
    WinEvent {
        event: u32,
        hwnd: u64,
        object: i32,
        child: i32,
        event_thread: u32,
        event_time_ms: u32,
        identity: WindowIdentity,
    },
    Backend {
        stage: &'static str,
        phase: &'static str,
        viewport: u64,
        arg0: u64,
        arg1: u64,
    },
    Hooks {
        phase: &'static str,
        callwndproc: bool,
        cbt: bool,
        foreground: bool,
        show_hide: bool,
    },
}

#[derive(Clone, Copy)]
struct TimedInstrumentationEvent {
    id: u64,
    target: u8,
    t_us: u64,
    sequence: u64,
    kind: InstrumentationKind,
}

struct InstrumentationQueue {
    tx: crossbeam_channel::Sender<TimedInstrumentationEvent>,
    rx: crossbeam_channel::Receiver<TimedInstrumentationEvent>,
}

fn instrumentation_queue() -> &'static InstrumentationQueue {
    static QUEUE: OnceLock<InstrumentationQueue> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::bounded(INSTRUMENTATION_CAPACITY);
        InstrumentationQueue { tx, rx }
    })
}

struct PendingViewportCommand {
    id: AtomicU64,
    target: AtomicU8,
    hwnd: AtomicU64,
    mask: AtomicU8,
}

impl PendingViewportCommand {
    const fn new() -> Self {
        Self {
            id: AtomicU64::new(0),
            target: AtomicU8::new(0),
            hwnd: AtomicU64::new(0),
            mask: AtomicU8::new(0),
        }
    }
}

const PENDING_VISIBLE: u8 = 1;
const PENDING_FOCUS: u8 = 2;
const PENDING_DESTROY: u8 = 4;
static PENDING_MAIN: PendingViewportCommand = PendingViewportCommand::new();
static PENDING_HOST: PendingViewportCommand = PendingViewportCommand::new();
static PENDING_BACKDROP: PendingViewportCommand = PendingViewportCommand::new();

thread_local! {
    static SCOPED_ID: Cell<u64> = const { Cell::new(0) };
    static SCOPED_TARGET: Cell<u8> = const { Cell::new(0) };
}

pub(crate) struct TransitionScope {
    previous_id: u64,
    previous_target: u8,
}

impl TransitionScope {
    pub(crate) fn enter(id: u64, target: TransitionTarget) -> Self {
        let previous_id = SCOPED_ID.replace(id);
        let previous_target = SCOPED_TARGET.replace(target.as_u8());
        Self {
            previous_id,
            previous_target,
        }
    }
}

impl Drop for TransitionScope {
    fn drop(&mut self) {
        SCOPED_ID.set(self.previous_id);
        SCOPED_TARGET.set(self.previous_target);
    }
}

/// Connect vendored eframe/egui-wgpu stage markers to this temporary observer.
pub(crate) fn install_backend_stage_observer() {
    let _ = instrumentation_queue();
    egui_wgpu::presentation_diag::set_sink(record_backend_stage);
}

fn record_backend_stage(event: egui_wgpu::presentation_diag::Event) {
    record_instrumentation(InstrumentationKind::Backend {
        stage: event.stage,
        phase: event.phase,
        viewport: event.viewport,
        arg0: event.arg0,
        arg1: event.arg1,
    });
}

fn record_instrumentation(kind: InstrumentationKind) {
    let id = ACTIVE_ID.load(Ordering::Acquire);
    if id == 0 {
        return;
    }
    let event = TimedInstrumentationEvent {
        id,
        target: ACTIVE_TARGET.load(Ordering::Acquire),
        t_us: crate::logger::elapsed_micros(),
        sequence: INSTRUMENTATION_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        kind,
    };
    if instrumentation_queue().tx.try_send(event).is_err() {
        INSTRUMENTATION_DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

fn start_transition_hooks() {
    // A transition is not expected to overlap, but never leave a stale native hook installed if
    // an earlier diagnostic run ended abnormally inside the reducer.
    stop_transition_hooks_without_record();
    let thread_id = unsafe { GetCurrentThreadId() };
    let callwndproc = unsafe {
        SetWindowsHookExW(WH_CALLWNDPROC, Some(callwndproc_hook_proc), None, thread_id).ok()
    };
    let cbt = unsafe { SetWindowsHookExW(WH_CBT, Some(cbt_hook_proc), None, thread_id).ok() };
    if let Some(hook) = callwndproc {
        CALLWNDPROC_HOOK.store(hook.0 as usize, Ordering::Release);
    }
    if let Some(hook) = cbt {
        CBT_HOOK.store(hook.0 as usize, Ordering::Release);
    }

    let foreground = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    let show_hide = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_SHOW,
            EVENT_OBJECT_HIDE,
            None,
            Some(win_event_proc),
            GetCurrentProcessId(),
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    if !foreground.0.is_null() {
        FOREGROUND_HOOK.store(foreground.0 as usize, Ordering::Release);
    }
    if !show_hide.0.is_null() {
        SHOW_HIDE_HOOK.store(show_hide.0 as usize, Ordering::Release);
    }
    record_instrumentation(InstrumentationKind::Hooks {
        phase: "install",
        callwndproc: callwndproc.is_some(),
        cbt: cbt.is_some(),
        foreground: !foreground.0.is_null(),
        show_hide: !show_hide.0.is_null(),
    });
}

fn stop_transition_hooks() {
    let (callwndproc, cbt, foreground, show_hide) = stop_transition_hooks_without_record();
    record_instrumentation(InstrumentationKind::Hooks {
        phase: "uninstall",
        callwndproc,
        cbt,
        foreground,
        show_hide,
    });
}

fn stop_transition_hooks_without_record() -> (bool, bool, bool, bool) {
    let callwndproc = unhook_thread_hook(&CALLWNDPROC_HOOK);
    let cbt = unhook_thread_hook(&CBT_HOOK);
    let foreground = unhook_win_event(&FOREGROUND_HOOK);
    let show_hide = unhook_win_event(&SHOW_HIDE_HOOK);
    (callwndproc, cbt, foreground, show_hide)
}

fn unhook_thread_hook(slot: &AtomicUsize) -> bool {
    let raw = slot.swap(0, Ordering::AcqRel);
    raw == 0 || unsafe { UnhookWindowsHookEx(HHOOK(raw as *mut _)).is_ok() }
}

fn unhook_win_event(slot: &AtomicUsize) -> bool {
    let raw = slot.swap(0, Ordering::AcqRel);
    raw == 0 || unsafe { UnhookWinEvent(HWINEVENTHOOK(raw as *mut _)).as_bool() }
}

unsafe extern "system" fn callwndproc_hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= HC_ACTION as i32 && lparam.0 != 0 {
        let message = unsafe { &*(lparam.0 as *const CWPSTRUCT) };
        if observed_message(message.message) {
            record_instrumentation(InstrumentationKind::Message {
                hwnd: hwnd_raw(message.hwnd),
                msg: message.message,
                payload: unsafe {
                    capture_message_payload(message.message, message.wParam, message.lParam)
                },
            });
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn cbt_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let hwnd = wparam.0 as u64;
    let decoded = match code as u32 {
        HCBT_CREATEWND if lparam.0 != 0 => {
            let create = unsafe { &*(lparam.0 as *const CBT_CREATEWNDW) };
            if create.lpcs.is_null() {
                None
            } else {
                let cs = unsafe { &*create.lpcs };
                Some((
                    hwnd,
                    CbtPayload::Create {
                        style: cs.style as u32,
                        ex_style: cs.dwExStyle.0,
                        parent: hwnd_raw(cs.hwndParent),
                        insert_after: hwnd_raw(create.hwndInsertAfter),
                        x: cs.x,
                        y: cs.y,
                        cx: cs.cx,
                        cy: cs.cy,
                    },
                ))
            }
        }
        HCBT_MINMAX => Some((
            hwnd,
            CbtPayload::MinMax {
                command: lparam.0 as u32 & 0xffff,
            },
        )),
        HCBT_ACTIVATE if lparam.0 != 0 => {
            let activate = unsafe { &*(lparam.0 as *const CBTACTIVATESTRUCT) };
            Some((
                hwnd,
                CbtPayload::Activate {
                    other: hwnd_raw(activate.hWndActive),
                    mouse: activate.fMouse.as_bool(),
                },
            ))
        }
        HCBT_SETFOCUS => Some((
            hwnd,
            CbtPayload::SetFocus {
                other: lparam.0 as u64,
            },
        )),
        HCBT_MOVESIZE if lparam.0 != 0 => {
            let rect = unsafe { &*(lparam.0 as *const RECT) };
            Some((
                hwnd,
                CbtPayload::MoveSize {
                    left: rect.left,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                },
            ))
        }
        HCBT_SYSCOMMAND => Some((
            0,
            CbtPayload::SysCommand {
                command: wparam.0,
                raw_lparam: lparam.0,
            },
        )),
        HCBT_DESTROYWND => Some((hwnd, CbtPayload::Destroy)),
        _ => None,
    };
    if let Some((hwnd, payload)) = decoded {
        record_instrumentation(InstrumentationKind::Cbt { hwnd, payload });
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    event_thread: u32,
    event_time_ms: u32,
) {
    let foreground = event == EVENT_SYSTEM_FOREGROUND;
    let window_show_hide = matches!(event, EVENT_OBJECT_SHOW | EVENT_OBJECT_HIDE)
        && object == OBJID_WINDOW.0
        && child == CHILDID_SELF as i32;
    if !foreground && !window_show_hide {
        return;
    }
    record_instrumentation(InstrumentationKind::WinEvent {
        event,
        hwnd: hwnd_raw(hwnd),
        object,
        child,
        event_thread,
        event_time_ms,
        identity: unsafe { window_identity(hwnd) },
    });
}

unsafe fn window_identity(hwnd: HWND) -> WindowIdentity {
    if hwnd.0.is_null() {
        return WindowIdentity::default();
    }
    let mut identity = WindowIdentity::default();
    identity.tid = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut identity.pid)) };
    let len = unsafe { GetClassNameW(hwnd, &mut identity.class_name) };
    identity.class_len = len.max(0).min((CLASS_NAME_CAPACITY - 1) as i32) as u8;
    identity
}

fn hwnd_raw(hwnd: HWND) -> u64 {
    hwnd.0 as usize as u64
}

fn observed_message(msg: u32) -> bool {
    matches!(
        msg,
        WM_NCCREATE
            | WM_CREATE
            | WM_SHOWWINDOW
            | WM_WINDOWPOSCHANGING
            | WM_WINDOWPOSCHANGED
            | WM_STYLECHANGING
            | WM_STYLECHANGED
            | WM_SIZE
            | WM_SETFOCUS
            | WM_KILLFOCUS
            | WM_ACTIVATE
            | WM_ACTIVATEAPP
            | WM_NCACTIVATE
            | WM_DESTROY
            | WM_NCDESTROY
    )
}

unsafe fn capture_message_payload(msg: u32, wparam: WPARAM, lparam: LPARAM) -> MessagePayload {
    match msg {
        WM_NCCREATE | WM_CREATE if lparam.0 != 0 => {
            let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
            MessagePayload::Create {
                style: create.style as u32,
                ex_style: create.dwExStyle.0,
                parent: hwnd_raw(create.hwndParent),
                x: create.x,
                y: create.y,
                cx: create.cx,
                cy: create.cy,
            }
        }
        WM_ACTIVATE => MessagePayload::Activate {
            state: (wparam.0 & 0xffff) as u32,
            minimized: ((wparam.0 >> 16) & 0xffff) != 0,
            other: lparam.0 as u64,
        },
        WM_ACTIVATEAPP => MessagePayload::ActivateApp {
            active: wparam.0 != 0,
            other_thread: lparam.0 as u32,
        },
        WM_NCACTIVATE => MessagePayload::NcActivate {
            active: wparam.0 != 0,
            raw_lparam: lparam.0,
        },
        WM_SHOWWINDOW => MessagePayload::ShowWindow {
            shown: wparam.0 != 0,
            reason: lparam.0,
        },
        WM_SETFOCUS | WM_KILLFOCUS => MessagePayload::Focus {
            other: wparam.0 as u64,
        },
        WM_WINDOWPOSCHANGING | WM_WINDOWPOSCHANGED if lparam.0 != 0 => {
            let pos = unsafe { &*(lparam.0 as *const WINDOWPOS) };
            MessagePayload::WindowPos {
                insert_after: hwnd_raw(pos.hwndInsertAfter),
                x: pos.x,
                y: pos.y,
                cx: pos.cx,
                cy: pos.cy,
                flags: pos.flags.0,
            }
        }
        WM_STYLECHANGING | WM_STYLECHANGED if lparam.0 != 0 => {
            let style = unsafe { &*(lparam.0 as *const STYLESTRUCT) };
            MessagePayload::Style {
                index: wparam.0 as isize,
                old: style.styleOld,
                new: style.styleNew,
            }
        }
        WM_SIZE => MessagePayload::Size {
            kind: wparam.0,
            width: (lparam.0 as u32 & 0xffff) as u16,
            height: ((lparam.0 as u32 >> 16) & 0xffff) as u16,
        },
        _ => MessagePayload::None,
    }
}

fn flush_instrumentation() {
    let mut events = instrumentation_queue().rx.try_iter().collect::<Vec<_>>();
    events.sort_unstable_by_key(|event| (event.t_us, event.sequence));
    let last_transition = events.last().map(|event| (event.id, event.target));
    for event in events {
        crate::logger::log(format_instrumentation_event(event));
    }
    let dropped = INSTRUMENTATION_DROPPED.swap(0, Ordering::AcqRel);
    if dropped != 0 {
        let (id, target) = last_transition.unwrap_or((0, TransitionTarget::Unknown.as_u8()));
        crate::logger::log(format!(
            "[presentation-hook] t_us={} transition={} target={} source=queue event=dropped count={dropped}",
            crate::logger::elapsed_micros(),
            id,
            TransitionTarget::from_u8(target).label(),
        ));
    }
}

fn format_instrumentation_event(event: TimedInstrumentationEvent) -> String {
    let target = TransitionTarget::from_u8(event.target);
    match event.kind {
        InstrumentationKind::Message { hwnd, msg, payload } => format!(
            "[presentation-hook] t_us={} transition={} target={} source=callwndproc role={} hwnd={} class={:?} message={} {}",
            event.t_us,
            event.id,
            target.label(),
            role_for(hwnd, WindowRole::Other).label(),
            fmt_hwnd(hwnd),
            class_name_for_raw(hwnd),
            message_name(msg),
            format_message_payload(payload),
        ),
        InstrumentationKind::Cbt { hwnd, payload } => {
            format_cbt_event(event, target, hwnd, payload)
        }
        InstrumentationKind::WinEvent {
            event: win_event,
            hwnd,
            object,
            child,
            event_thread,
            event_time_ms,
            identity,
        } => format!(
            "[presentation-winevent] t_us={} transition={} target={} event={} hwnd={} role={} class={:?} pid={} tid={} object={} child={} event_thread={} event_time_ms={}",
            event.t_us,
            event.id,
            target.label(),
            win_event_name(win_event),
            fmt_hwnd(hwnd),
            role_for(hwnd, WindowRole::Other).label(),
            identity_class_name(&identity),
            identity.pid,
            identity.tid,
            object,
            child,
            event_thread,
            event_time_ms,
        ),
        InstrumentationKind::Backend {
            stage,
            phase,
            viewport,
            arg0,
            arg1,
        } => format!(
            "[presentation-viewport] t_us={} transition={} target={} stage={} phase={} viewport=0x{:x} arg0={} arg1={}",
            event.t_us,
            event.id,
            target.label(),
            stage,
            phase,
            viewport,
            arg0,
            arg1,
        ),
        InstrumentationKind::Hooks {
            phase,
            callwndproc,
            cbt,
            foreground,
            show_hide,
        } => format!(
            "[presentation-hook] t_us={} transition={} target={} source=lifecycle phase={} callwndproc={} cbt={} foreground={} show_hide={}",
            event.t_us,
            event.id,
            target.label(),
            phase,
            callwndproc,
            cbt,
            foreground,
            show_hide,
        ),
    }
}

fn format_message_payload(payload: MessagePayload) -> String {
    match payload {
        MessagePayload::None => "payload=none".to_owned(),
        MessagePayload::Create {
            style,
            ex_style,
            parent,
            x,
            y,
            cx,
            cy,
        } => format!(
            "style=0x{style:x} ex_style=0x{ex_style:x} ws_visible={} ws_maximized={} parent={} rect=({x},{y},{cx}x{cy})",
            style & WS_VISIBLE.0 != 0,
            style & WS_MAXIMIZE.0 != 0,
            fmt_hwnd(parent),
        ),
        MessagePayload::Activate {
            state,
            minimized,
            other,
        } => format!(
            "state={}({state}) minimized={minimized} other={}",
            activate_state_name(state),
            fmt_hwnd(other),
        ),
        MessagePayload::ActivateApp {
            active,
            other_thread,
        } => format!("active={active} other_thread={other_thread}"),
        MessagePayload::NcActivate { active, raw_lparam } => {
            format!("active={active} raw_lparam=0x{:x}", raw_lparam as usize)
        }
        MessagePayload::ShowWindow { shown, reason } => format!(
            "shown={shown} reason={}({reason})",
            show_window_reason_name(reason),
        ),
        MessagePayload::Focus { other } => format!("other={}", fmt_hwnd(other)),
        MessagePayload::WindowPos {
            insert_after,
            x,
            y,
            cx,
            cy,
            flags,
        } => format!(
            "insert_after={} rect=({x},{y},{cx}x{cy}) flags=0x{flags:x}",
            fmt_hwnd(insert_after),
        ),
        MessagePayload::Style { index, old, new } => format!(
            "index={index} old=0x{old:x} new=0x{new:x} old_visible={} new_visible={} old_maximized={} new_maximized={}",
            old & WS_VISIBLE.0 != 0,
            new & WS_VISIBLE.0 != 0,
            old & WS_MAXIMIZE.0 != 0,
            new & WS_MAXIMIZE.0 != 0,
        ),
        MessagePayload::Size {
            kind,
            width,
            height,
        } => format!(
            "kind={}({kind}) size={}x{}",
            size_kind_name(kind),
            width,
            height,
        ),
    }
}

fn format_cbt_event(
    event: TimedInstrumentationEvent,
    target: TransitionTarget,
    hwnd: u64,
    payload: CbtPayload,
) -> String {
    let (name, detail) = match payload {
        CbtPayload::Create {
            style,
            ex_style,
            parent,
            insert_after,
            x,
            y,
            cx,
            cy,
        } => (
            "HCBT_CREATEWND",
            format!(
                "style=0x{style:x} ex_style=0x{ex_style:x} ws_visible={} ws_maximized={} parent={} insert_after={} rect=({x},{y},{cx}x{cy})",
                style & WS_VISIBLE.0 != 0,
                style & WS_MAXIMIZE.0 != 0,
                fmt_hwnd(parent),
                fmt_hwnd(insert_after),
            ),
        ),
        CbtPayload::MinMax { command } => (
            "HCBT_MINMAX",
            format!("command={}({command})", show_command_name(command)),
        ),
        CbtPayload::Activate { other, mouse } => (
            "HCBT_ACTIVATE",
            format!("other={} mouse={mouse}", fmt_hwnd(other)),
        ),
        CbtPayload::SetFocus { other } => ("HCBT_SETFOCUS", format!("other={}", fmt_hwnd(other))),
        CbtPayload::MoveSize {
            left,
            top,
            right,
            bottom,
        } => (
            "HCBT_MOVESIZE",
            format!("rect=({left},{top},{}x{})", right - left, bottom - top),
        ),
        CbtPayload::SysCommand {
            command,
            raw_lparam,
        } => (
            "HCBT_SYSCOMMAND",
            format!(
                "command=0x{command:x} raw_lparam=0x{:x}",
                raw_lparam as usize
            ),
        ),
        CbtPayload::Destroy => ("HCBT_DESTROYWND", "payload=none".to_owned()),
    };
    format!(
        "[presentation-hook] t_us={} transition={} target={} source=cbt role={} hwnd={} class={:?} event={} {}",
        event.t_us,
        event.id,
        target.label(),
        role_for(hwnd, WindowRole::Other).label(),
        fmt_hwnd(hwnd),
        class_name_for_raw(hwnd),
        name,
        detail,
    )
}

fn activate_state_name(state: u32) -> &'static str {
    match state {
        WA_INACTIVE => "WA_INACTIVE",
        WA_ACTIVE => "WA_ACTIVE",
        WA_CLICKACTIVE => "WA_CLICKACTIVE",
        _ => "unknown",
    }
}

fn show_command_name(command: u32) -> &'static str {
    match command {
        0 => "SW_HIDE",
        1 => "SW_SHOWNORMAL",
        2 => "SW_SHOWMINIMIZED",
        3 => "SW_MAXIMIZE",
        4 => "SW_SHOWNOACTIVATE",
        5 => "SW_SHOW",
        6 => "SW_MINIMIZE",
        7 => "SW_SHOWMINNOACTIVE",
        8 => "SW_SHOWNA",
        9 => "SW_RESTORE",
        10 => "SW_SHOWDEFAULT",
        11 => "SW_FORCEMINIMIZE",
        _ => "unknown",
    }
}

fn show_window_reason_name(reason: isize) -> &'static str {
    match reason {
        0 => "direct",
        1 => "SW_PARENTCLOSING",
        2 => "SW_OTHERZOOM",
        3 => "SW_PARENTOPENING",
        4 => "SW_OTHERUNZOOM",
        _ => "unknown",
    }
}

fn size_kind_name(kind: usize) -> &'static str {
    match kind {
        0 => "SIZE_RESTORED",
        1 => "SIZE_MINIMIZED",
        2 => "SIZE_MAXIMIZED",
        3 => "SIZE_MAXSHOW",
        4 => "SIZE_MAXHIDE",
        _ => "unknown",
    }
}

fn win_event_name(event: u32) -> &'static str {
    match event {
        EVENT_SYSTEM_FOREGROUND => "EVENT_SYSTEM_FOREGROUND",
        EVENT_OBJECT_SHOW => "EVENT_OBJECT_SHOW",
        EVENT_OBJECT_HIDE => "EVENT_OBJECT_HIDE",
        _ => "unknown",
    }
}

fn identity_class_name(identity: &WindowIdentity) -> String {
    let len = usize::from(identity.class_len).min(CLASS_NAME_CAPACITY);
    if len == 0 {
        "<unavailable>".to_owned()
    } else {
        String::from_utf16_lossy(&identity.class_name[..len])
    }
}

fn class_name_for_raw(hwnd: u64) -> String {
    if hwnd == 0 {
        return "<none>".to_owned();
    }
    let identity = unsafe { window_identity(HWND(hwnd as usize as *mut _)) };
    identity_class_name(&identity)
}

pub(crate) fn begin_transition(id: u64, target: TransitionTarget, w: KnownWindows) {
    PENDING_MAIN.mask.store(0, Ordering::Release);
    PENDING_HOST.mask.store(0, Ordering::Release);
    PENDING_BACKDROP.mask.store(0, Ordering::Release);
    register(WindowRole::Main, w.main);
    register(WindowRole::Host, w.host);
    register(WindowRole::Presenter, w.presenter);
    register(WindowRole::Backdrop, w.backdrop);
    register(WindowRole::Hud, w.hud);
    ACTIVE_TARGET.store(target.as_u8(), Ordering::Release);
    ACTIVE_ID.store(id, Ordering::Release);
    crate::logger::log(format!(
        "[presentation-window] t_us={} transition={} target={} phase=begin main={} host={} presenter={} backdrop={} hud={}",
        crate::logger::elapsed_micros(),
        id,
        target.label(),
        fmt_hwnd(w.main),
        fmt_hwnd(w.host),
        fmt_hwnd(w.presenter),
        fmt_hwnd(w.backdrop),
        fmt_hwnd(w.hud),
    ));
    start_transition_hooks();
}

pub(crate) fn finish_transition() {
    let id = ACTIVE_ID.load(Ordering::Acquire);
    if id == 0 {
        return;
    }
    stop_transition_hooks();
    let id = ACTIVE_ID.swap(0, Ordering::AcqRel);
    let target = TransitionTarget::from_u8(ACTIVE_TARGET.swap(0, Ordering::AcqRel));
    crate::logger::log(format!(
        "[presentation-window] t_us={} transition={} target={} phase=end",
        crate::logger::elapsed_micros(),
        id,
        target.label(),
    ));
    flush_instrumentation();
}

pub(crate) fn active_transition_id() -> Option<u64> {
    let scoped = SCOPED_ID.get();
    let id = if scoped != 0 {
        scoped
    } else {
        ACTIVE_ID.load(Ordering::Acquire)
    };
    (id != 0).then_some(id)
}

pub(crate) fn active_target() -> TransitionTarget {
    let scoped = SCOPED_TARGET.get();
    TransitionTarget::from_u8(if scoped != 0 {
        scoped
    } else {
        ACTIVE_TARGET.load(Ordering::Acquire)
    })
}

pub(crate) fn register(role: WindowRole, hwnd_raw: u64) {
    if hwnd_raw != 0 {
        role_slot(role).store(hwnd_raw, Ordering::Release);
    }
}

pub(crate) fn unregister(role: WindowRole, hwnd_raw: u64) {
    if hwnd_raw != 0 {
        let _ = role_slot(role).compare_exchange(hwnd_raw, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}

fn role_slot(role: WindowRole) -> &'static AtomicU64 {
    match role {
        WindowRole::Main => &MAIN_HWND,
        WindowRole::Host => &HOST_HWND,
        WindowRole::Presenter => &PRESENTER_HWND,
        WindowRole::Backdrop => &BACKDROP_HWND,
        WindowRole::Hud => &HUD_HWND,
        WindowRole::Other => &HOST_HWND,
    }
}

fn role_for(hwnd_raw: u64, fallback: WindowRole) -> WindowRole {
    for (role, slot) in [
        (WindowRole::Main, &MAIN_HWND),
        (WindowRole::Host, &HOST_HWND),
        (WindowRole::Presenter, &PRESENTER_HWND),
        (WindowRole::Backdrop, &BACKDROP_HWND),
        (WindowRole::Hud, &HUD_HWND),
    ] {
        if hwnd_raw != 0 && slot.load(Ordering::Acquire) == hwnd_raw {
            return role;
        }
    }
    fallback
}

fn pending_slot(role: WindowRole) -> &'static PendingViewportCommand {
    match role {
        WindowRole::Main => &PENDING_MAIN,
        WindowRole::Backdrop => &PENDING_BACKDROP,
        _ => &PENDING_HOST,
    }
}

fn pending_bit(action: WindowAction) -> u8 {
    match action {
        WindowAction::Visible => PENDING_VISIBLE,
        WindowAction::Focus => PENDING_FOCUS,
        WindowAction::Destroy => PENDING_DESTROY,
        _ => 0,
    }
}

fn remember_viewport_command(
    id: u64,
    target: TransitionTarget,
    action: WindowAction,
    role: WindowRole,
    hwnd_raw: u64,
) {
    let bit = pending_bit(action);
    if bit == 0 {
        return;
    }
    let pending = pending_slot(role);
    pending.target.store(target.as_u8(), Ordering::Release);
    pending.hwnd.store(hwnd_raw, Ordering::Release);
    pending.id.store(id, Ordering::Release);
    pending.mask.fetch_or(bit, Ordering::AcqRel);
}

pub(crate) fn observe(
    action: WindowAction,
    role: WindowRole,
    hwnd_raw: u64,
    source: &'static str,
    detail: impl AsRef<str>,
) {
    let Some(id) = active_transition_id() else {
        return;
    };
    observe_for_transition(id, active_target(), action, role, hwnd_raw, source, detail);
}

pub(crate) fn observe_viewport_command(
    action: WindowAction,
    role: WindowRole,
    hwnd_raw: u64,
    source: &'static str,
    detail: impl AsRef<str>,
) {
    let Some(id) = active_transition_id() else {
        return;
    };
    let target = active_target();
    remember_viewport_command(id, target, action, role, hwnd_raw);
    observe_for_transition(id, target, action, role, hwnd_raw, source, detail);
}

pub(crate) fn observe_viewport_command_for_transition(
    id: u64,
    target: TransitionTarget,
    action: WindowAction,
    role: WindowRole,
    hwnd_raw: u64,
    source: &'static str,
    detail: impl AsRef<str>,
) {
    remember_viewport_command(id, target, action, role, hwnd_raw);
    observe_for_transition(id, target, action, role, hwnd_raw, source, detail);
}

pub(crate) fn observe_for_transition(
    id: u64,
    target: TransitionTarget,
    action: WindowAction,
    fallback_role: WindowRole,
    hwnd_raw: u64,
    source: &'static str,
    detail: impl AsRef<str>,
) {
    let role = role_for(hwnd_raw, fallback_role);
    crate::logger::log(format!(
        "[presentation-window] t_us={} transition={} target={} action={} source={} role={} hwnd={} {} {}",
        crate::logger::elapsed_micros(),
        id,
        target.label(),
        action.label(),
        source,
        role.label(),
        fmt_hwnd(hwnd_raw),
        detail.as_ref(),
        snapshot(hwnd_raw),
    ));
}

pub(crate) fn observe_window_message(
    hwnd: HWND,
    fallback_role: WindowRole,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) {
    if !observed_message(msg) {
        return;
    }
    let hwnd_raw = hwnd.0 as usize as u64;
    let payload = unsafe { capture_message_payload(msg, wparam, lparam) };
    if active_transition_id().is_some() {
        observe(
            WindowAction::OsMessage,
            fallback_role,
            hwnd_raw,
            "wndproc",
            format!(
                "message={} {}",
                message_name(msg),
                format_message_payload(payload)
            ),
        );
        return;
    }
    let role = role_for(hwnd_raw, fallback_role);
    let pending = pending_slot(role);
    let bit = pending_message_bit(msg);
    let mask = pending.mask.load(Ordering::Acquire);
    let expected_hwnd = pending.hwnd.load(Ordering::Acquire);
    if bit == 0 || mask & bit == 0 || (expected_hwnd != 0 && expected_hwnd != hwnd_raw) {
        return;
    }
    let id = pending.id.load(Ordering::Acquire);
    if id == 0 {
        return;
    }
    let target = TransitionTarget::from_u8(pending.target.load(Ordering::Acquire));
    observe_for_transition(
        id,
        target,
        WindowAction::OsMessage,
        role,
        hwnd_raw,
        "wndproc",
        format!(
            "message={} {} correlation=pending_viewport_command",
            message_name(msg),
            format_message_payload(payload)
        ),
    );
    if consumes_pending_bit(msg) {
        pending.mask.fetch_and(!bit, Ordering::AcqRel);
    }
}

fn pending_message_bit(msg: u32) -> u8 {
    match msg {
        WM_SHOWWINDOW => PENDING_VISIBLE,
        WM_SETFOCUS | WM_KILLFOCUS | WM_ACTIVATE | WM_ACTIVATEAPP | WM_NCACTIVATE => PENDING_FOCUS,
        WM_DESTROY | WM_NCDESTROY => PENDING_DESTROY,
        WM_WINDOWPOSCHANGING | WM_WINDOWPOSCHANGED => PENDING_VISIBLE | PENDING_DESTROY,
        _ => 0,
    }
}

fn consumes_pending_bit(msg: u32) -> bool {
    matches!(msg, WM_SHOWWINDOW | WM_SETFOCUS | WM_DESTROY | WM_NCDESTROY)
}

pub(crate) unsafe fn show_window(
    hwnd: HWND,
    command: SHOW_WINDOW_CMD,
    role: WindowRole,
    source: &'static str,
) -> bool {
    let previous = unsafe { ShowWindow(hwnd, command) }.as_bool();
    observe(
        WindowAction::ShowWindow,
        role,
        hwnd.0 as usize as u64,
        source,
        format!("command={} previous_visible={previous}", command.0),
    );
    previous
}

#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn set_window_pos(
    hwnd: HWND,
    insert_after: Option<HWND>,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    flags: SET_WINDOW_POS_FLAGS,
    role: WindowRole,
    source: &'static str,
) -> windows::core::Result<()> {
    let result = unsafe { SetWindowPos(hwnd, insert_after, x, y, cx, cy, flags) };
    let after = insert_after
        .map(|value| fmt_hwnd(value.0 as usize as u64))
        .unwrap_or_else(|| "none".to_owned());
    observe(
        WindowAction::SetWindowPos,
        role,
        hwnd.0 as usize as u64,
        source,
        format!(
            "after={after} x={x} y={y} cx={cx} cy={cy} flags=0x{:x} ok={}",
            flags.0,
            result.is_ok()
        ),
    );
    result
}

pub(crate) unsafe fn set_foreground_window(
    hwnd: HWND,
    role: WindowRole,
    source: &'static str,
) -> bool {
    let result = unsafe { SetForegroundWindow(hwnd) }.as_bool();
    observe(
        WindowAction::SetForegroundWindow,
        role,
        hwnd.0 as usize as u64,
        source,
        format!("ok={result}"),
    );
    result
}

pub(crate) unsafe fn set_focus(
    hwnd: Option<HWND>,
    role: WindowRole,
    source: &'static str,
) -> windows::core::Result<HWND> {
    let result = unsafe { SetFocus(hwnd) };
    let raw = hwnd.map(|value| value.0 as usize as u64).unwrap_or(0);
    observe(
        WindowAction::SetFocus,
        role,
        raw,
        source,
        format!("ok={}", result.is_ok()),
    );
    result
}

pub(crate) unsafe fn set_active_window(
    hwnd: HWND,
    role: WindowRole,
    source: &'static str,
) -> windows::core::Result<HWND> {
    let result = unsafe { SetActiveWindow(hwnd) };
    observe(
        WindowAction::SetActiveWindow,
        role,
        hwnd.0 as usize as u64,
        source,
        format!("ok={}", result.is_ok()),
    );
    result
}

pub(crate) unsafe fn destroy_window(
    hwnd: HWND,
    role: WindowRole,
    source: &'static str,
) -> windows::core::Result<()> {
    let result = unsafe { DestroyWindow(hwnd) };
    observe(
        WindowAction::Destroy,
        role,
        hwnd.0 as usize as u64,
        source,
        format!("api=DestroyWindow ok={}", result.is_ok()),
    );
    result
}

pub(crate) fn observe_dwm_cloak(
    hwnd: HWND,
    role: WindowRole,
    cloaked: bool,
    source: &'static str,
    ok: bool,
) {
    observe(
        WindowAction::DwmCloak,
        role,
        hwnd.0 as usize as u64,
        source,
        format!("requested={cloaked} ok={ok}"),
    );
}

fn snapshot(hwnd_raw: u64) -> String {
    let foreground = unsafe { GetForegroundWindow() };
    let foreground_raw = foreground.0 as usize as u64;
    let foreground_role = role_for(foreground_raw, WindowRole::Other);
    let mut foreground_pid = 0_u32;
    let foreground_tid = if foreground_raw == 0 {
        0
    } else {
        unsafe { GetWindowThreadProcessId(foreground, Some(&mut foreground_pid)) }
    };
    let foreground_class = if foreground_raw == 0 {
        "<none>".to_owned()
    } else if foreground_role == WindowRole::Other {
        class_name_for_raw(foreground_raw)
    } else {
        "<ours>".to_owned()
    };
    let foreground_fields = format!(
        "fg={} fg_role={} fg_pid={} fg_tid={} fg_class={:?}",
        fmt_hwnd(foreground_raw),
        foreground_role.label(),
        foreground_pid,
        foreground_tid,
        foreground_class,
    );
    if hwnd_raw == 0 {
        return format!(
            "alive=false {foreground_fields} z=- visible=false rect=<none> parent=0x0 owner=0x0 cloaked=?"
        );
    }
    unsafe {
        let hwnd = HWND(hwnd_raw as usize as *mut _);
        let alive = IsWindow(Some(hwnd)).as_bool();
        if !alive {
            return format!(
                "alive=false {foreground_fields} z=- visible=false rect=<destroyed> parent=0x0 owner=0x0 cloaked=?"
            );
        }
        let visible = IsWindowVisible(hwnd).as_bool();
        let mut rect = RECT::default();
        let rect_text = if GetWindowRect(hwnd, &mut rect).is_ok() {
            format!(
                "({},{},{}x{})",
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top
            )
        } else {
            "<err>".to_owned()
        };
        let parent = GetAncestor(hwnd, GA_PARENT);
        let owner = GetWindow(hwnd, GW_OWNER).unwrap_or_default();
        let mut cloaked = 0_u32;
        let cloak_ok = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
        )
        .is_ok();
        format!(
            "alive=true {} z={} visible={} rect={} parent={} owner={} cloaked={}",
            foreground_fields,
            z_rank(hwnd, parent)
                .map(|rank| rank.to_string())
                .unwrap_or_else(|| "?".to_owned()),
            visible,
            rect_text,
            fmt_hwnd(parent.0 as usize as u64),
            fmt_hwnd(owner.0 as usize as u64),
            if cloak_ok {
                cloaked.to_string()
            } else {
                "?".to_owned()
            },
        )
    }
}

unsafe fn z_rank(hwnd: HWND, parent: HWND) -> Option<u32> {
    let parent = (!parent.0.is_null()).then_some(parent);
    let mut cursor = unsafe { GetTopWindow(parent) }.ok()?;
    let mut rank = 0_u32;
    while rank < 256 {
        if cursor == hwnd {
            return Some(rank);
        }
        cursor = unsafe { GetWindow(cursor, GW_HWNDNEXT) }.ok()?;
        if cursor.0.is_null() {
            return None;
        }
        rank += 1;
    }
    None
}

fn fmt_hwnd(hwnd_raw: u64) -> String {
    format!("0x{hwnd_raw:x}")
}

fn message_name(msg: u32) -> &'static str {
    match msg {
        WM_NCCREATE => "WM_NCCREATE",
        WM_CREATE => "WM_CREATE",
        WM_SHOWWINDOW => "WM_SHOWWINDOW",
        WM_WINDOWPOSCHANGING => "WM_WINDOWPOSCHANGING",
        WM_WINDOWPOSCHANGED => "WM_WINDOWPOSCHANGED",
        WM_STYLECHANGING => "WM_STYLECHANGING",
        WM_STYLECHANGED => "WM_STYLECHANGED",
        WM_SIZE => "WM_SIZE",
        WM_SETFOCUS => "WM_SETFOCUS",
        WM_KILLFOCUS => "WM_KILLFOCUS",
        WM_ACTIVATE => "WM_ACTIVATE",
        WM_ACTIVATEAPP => "WM_ACTIVATEAPP",
        WM_NCACTIVATE => "WM_NCACTIVATE",
        WM_DESTROY => "WM_DESTROY",
        WM_NCDESTROY => "WM_NCDESTROY",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::{TransitionTarget, WindowAction, WindowRole};

    #[test]
    fn probe_labels_are_stable_for_grep_and_timeline_joining() {
        assert_eq!(TransitionTarget::Detached.label(), "detached");
        assert_eq!(WindowRole::Presenter.label(), "presenter");
        assert_eq!(WindowAction::SetWindowPos.label(), "SetWindowPos");
    }
}
