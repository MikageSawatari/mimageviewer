//! Low-overhead observation of Win32 presentation calls during an F12 transition.
//!
//! The probe snapshots state only after an application-issued presentation call. It never waits
//! for DWM or changes a transition decision.

use std::cell::Cell;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GA_PARENT, GW_OWNER, GetAncestor, GetClassNameW, GetForegroundWindow, GetWindow,
    GetWindowRect, GetWindowThreadProcessId, IsWindow, IsWindowVisible, SET_WINDOW_POS_FLAGS,
    SHOW_WINDOW_CMD, SetForegroundWindow, SetWindowPos, ShowWindow,
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

fn class_name_for_raw(hwnd: u64) -> String {
    if hwnd == 0 {
        return "<none>".to_owned();
    }
    let hwnd = HWND(hwnd as usize as *mut _);
    let mut class_name = [0_u16; 96];
    let len = unsafe { GetClassNameW(hwnd, &mut class_name) };
    if len <= 0 {
        "<unavailable>".to_owned()
    } else {
        String::from_utf16_lossy(&class_name[..len as usize])
    }
}

pub(crate) fn begin_transition(id: u64, target: TransitionTarget, w: KnownWindows) {
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
}

pub(crate) fn finish_transition() {
    let id = ACTIVE_ID.swap(0, Ordering::AcqRel);
    if id == 0 {
        return;
    }
    let target = TransitionTarget::from_u8(ACTIVE_TARGET.swap(0, Ordering::AcqRel));
    crate::logger::log(format!(
        "[presentation-window] t_us={} transition={} target={} phase=end",
        crate::logger::elapsed_micros(),
        id,
        target.label(),
    ));
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
            "alive=false {foreground_fields} visible=false rect=<none> parent=0x0 owner=0x0 cloaked=?"
        );
    }
    unsafe {
        let hwnd = HWND(hwnd_raw as usize as *mut _);
        let alive = IsWindow(Some(hwnd)).as_bool();
        if !alive {
            return format!(
                "alive=false {foreground_fields} visible=false rect=<destroyed> parent=0x0 owner=0x0 cloaked=?"
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
            "alive=true {} visible={} rect={} parent={} owner={} cloaked={}",
            foreground_fields,
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

fn fmt_hwnd(hwnd_raw: u64) -> String {
    format!("0x{hwnd_raw:x}")
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
