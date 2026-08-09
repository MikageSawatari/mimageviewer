//! Phase 1 / Step 0 touch diagnostics.
//!
//! This module only observes existing egui input. Gesture recognition and event
//! mutation belong to later touch-support steps.

use std::fmt::Write as _;

/// Whether the opt-in touch diagnostics probe is enabled for this process.
///
/// Cache the environment lookup because this gate is checked from hot input
/// paths. Changing the environment after the first check intentionally has no
/// effect.
pub(crate) fn touch_debug_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MIV_TOUCH_DEBUG").is_some())
}

#[cfg(windows)]
pub(crate) fn log_native_touch_ownership(
    window: TouchDebugWindow,
    pointer_id: u32,
    owned: bool,
    reason: &str,
) {
    if touch_debug_enabled() {
        crate::logger::log(format!(
            "[TOUCH-DEBUG] native {} pointer_id={} ownership={} reason={}",
            window.label(),
            pointer_id,
            if owned { "owned" } else { "passed" },
            reason,
        ));
    }
}

#[cfg(windows)]
pub(crate) fn log_native_touch_coordinates(
    window: TouchDebugWindow,
    pointer_id: u32,
    client: [i32; 2],
    points: egui::Pos2,
) {
    if touch_debug_enabled() {
        crate::logger::log(format!(
            "[TOUCH-DEBUG] native {} pointer_id={} client=({},{}) points=({:.2},{:.2})",
            window.label(),
            pointer_id,
            client[0],
            client[1],
            points.x,
            points.y,
        ));
    }
}

#[cfg(windows)]
pub(crate) fn log_native_touch_command(
    window: TouchDebugWindow,
    command: &crate::video::native_touch::NativeVideoTouchCommand,
) {
    if touch_debug_enabled() {
        crate::logger::log(format!(
            "[TOUCH-DEBUG] native {} command={command:?}",
            window.label(),
        ));
    }
}

#[cfg(windows)]
pub(crate) fn log_native_touch_mouse_discard(window: TouchDebugWindow, msg: u32) {
    if touch_debug_enabled() {
        crate::logger::log(format!(
            "[TOUCH-DEBUG] native {} mouse_discarded msg=0x{msg:04x} source=IMDT_TOUCH",
            window.label(),
        ));
    }
}

/// Log the ordered egui event list only when the current viewport frame
/// contains at least one touch event. This function never mutates or consumes
/// the input queue.
pub(crate) fn log_egui_touch_events(ctx: &egui::Context, frame: u64) {
    if !touch_debug_enabled() {
        return;
    }

    let viewport_id = ctx.viewport_id();
    let events = ctx.input(|input| {
        input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::Touch { .. }))
            .then(|| format_egui_events(&input.events))
    });
    if let Some(events) = events {
        crate::logger::log(format!(
            "[TOUCH-DEBUG] egui viewport={viewport_id:?} frame={frame} events=[{events}]"
        ));
    }
}

fn format_egui_events(events: &[egui::Event]) -> String {
    let mut out = String::new();
    for (index, event) in events.iter().enumerate() {
        if index != 0 {
            out.push_str(" -> ");
        }
        match event {
            egui::Event::Touch {
                device_id,
                id,
                phase,
                pos,
                force,
            } => {
                let _ = write!(
                    out,
                    "Touch(device_id={} id={} phase={phase:?} pos=({:.1},{:.1}) force={force:?})",
                    device_id.0, id.0, pos.x, pos.y
                );
            }
            egui::Event::PointerMoved(pos) => {
                let _ = write!(out, "PointerMoved(pos=({:.1},{:.1}))", pos.x, pos.y);
            }
            egui::Event::PointerButton {
                button, pressed, ..
            } => {
                let _ = write!(out, "PointerButton(button={button:?} pressed={pressed})");
            }
            egui::Event::PointerGone => out.push_str("PointerGone"),
            _ => out.push_str("Other"),
        }
    }
    out
}

#[cfg(windows)]
#[derive(Clone, Copy)]
pub(crate) enum TouchDebugWindow {
    Presenter,
    Hud,
}

#[cfg(windows)]
impl TouchDebugWindow {
    fn label(self) -> &'static str {
        match self {
            Self::Presenter => "presenter",
            Self::Hud => "hud",
        }
    }

    fn pointer_update_rate_limit(self) -> &'static std::sync::atomic::AtomicU64 {
        static PRESENTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        static HUD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        match self {
            Self::Presenter => &PRESENTER,
            Self::Hud => &HUD,
        }
    }

    fn mouse_move_rate_limit(self) -> &'static std::sync::atomic::AtomicU64 {
        static PRESENTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        static HUD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        match self {
            Self::Presenter => &PRESENTER,
            Self::Hud => &HUD,
        }
    }
}

#[cfg(windows)]
fn rate_limit_100ms(last_log_ms: &std::sync::atomic::AtomicU64) -> bool {
    use std::sync::atomic::Ordering;

    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let now_ms = START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis()
        .min(u64::MAX as u128) as u64;
    let now_ms = now_ms.saturating_add(1);
    let mut previous = last_log_ms.load(Ordering::Relaxed);
    loop {
        if previous != 0 && now_ms.saturating_sub(previous) < 100 {
            return false;
        }
        match last_log_ms.compare_exchange_weak(
            previous,
            now_ms,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return true,
            Err(actual) => previous = actual,
        }
    }
}

/// Diagnostics-only observation hook for the native presenter and HUD wndprocs.
///
/// This function deliberately does not return an `LRESULT` and does not alter
/// message routing. Both callers invoke it once immediately before their
/// existing `match msg` dispatch.
#[cfg(windows)]
pub(crate) fn log_win32_message(
    window: TouchDebugWindow,
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) {
    if !touch_debug_enabled() {
        return;
    }

    use windows::Win32::Graphics::Gdi::ScreenToClient;
    use windows::Win32::UI::Input::Pointer::{
        GetPointerInfo, GetPointerType, POINTER_FLAG_CANCELED, POINTER_FLAG_INCONTACT,
        POINTER_FLAG_INRANGE, POINTER_FLAG_PRIMARY, POINTER_INFO,
    };
    use windows::Win32::UI::Input::{
        GetCurrentInputMessageSource, IMDT_MOUSE, IMDT_PEN, IMDT_TOUCH, IMDT_TOUCHPAD,
        IMDT_UNAVAILABLE, IMO_HARDWARE, IMO_INJECTED, IMO_SYSTEM, IMO_UNAVAILABLE,
        INPUT_MESSAGE_DEVICE_TYPE, INPUT_MESSAGE_ORIGIN_ID, INPUT_MESSAGE_SOURCE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        POINTER_INPUT_TYPE, PT_MOUSE, PT_PEN, PT_POINTER, PT_TOUCH, PT_TOUCHPAD, WM_GESTURE,
        WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK, WM_MBUTTONDOWN,
        WM_MBUTTONUP, WM_MOUSEMOVE, WM_NCPOINTERDOWN, WM_NCPOINTERUP, WM_NCPOINTERUPDATE,
        WM_POINTERCAPTURECHANGED, WM_POINTERDOWN, WM_POINTERENTER, WM_POINTERLEAVE, WM_POINTERUP,
        WM_POINTERUPDATE, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_TOUCH,
        WM_XBUTTONDBLCLK, WM_XBUTTONDOWN, WM_XBUTTONUP,
    };

    fn pointer_type_label(pointer_type: POINTER_INPUT_TYPE) -> &'static str {
        match pointer_type {
            PT_TOUCH => "PT_TOUCH",
            PT_PEN => "PT_PEN",
            PT_MOUSE => "PT_MOUSE",
            PT_TOUCHPAD => "PT_TOUCHPAD",
            PT_POINTER => "PT_POINTER",
            _ => "PT_OTHER",
        }
    }

    fn device_type_label(device_type: INPUT_MESSAGE_DEVICE_TYPE) -> &'static str {
        match device_type {
            IMDT_TOUCH => "IMDT_TOUCH",
            IMDT_PEN => "IMDT_PEN",
            IMDT_MOUSE => "IMDT_MOUSE",
            IMDT_TOUCHPAD => "IMDT_TOUCHPAD",
            IMDT_UNAVAILABLE => "IMDT_UNAVAILABLE",
            _ => "IMDT_OTHER",
        }
    }

    fn origin_label(origin: INPUT_MESSAGE_ORIGIN_ID) -> &'static str {
        match origin {
            IMO_HARDWARE => "IMO_HARDWARE",
            IMO_INJECTED => "IMO_INJECTED",
            IMO_SYSTEM => "IMO_SYSTEM",
            IMO_UNAVAILABLE => "IMO_UNAVAILABLE",
            _ => "IMO_OTHER",
        }
    }

    fn message_name(msg: u32) -> &'static str {
        match msg {
            WM_POINTERDOWN => "WM_POINTERDOWN",
            WM_POINTERUP => "WM_POINTERUP",
            WM_POINTERUPDATE => "WM_POINTERUPDATE",
            WM_POINTERENTER => "WM_POINTERENTER",
            WM_POINTERLEAVE => "WM_POINTERLEAVE",
            WM_POINTERCAPTURECHANGED => "WM_POINTERCAPTURECHANGED",
            WM_NCPOINTERDOWN => "WM_NCPOINTERDOWN",
            WM_NCPOINTERUP => "WM_NCPOINTERUP",
            WM_NCPOINTERUPDATE => "WM_NCPOINTERUPDATE",
            WM_TOUCH => "WM_TOUCH",
            WM_GESTURE => "WM_GESTURE",
            WM_MOUSEMOVE => "WM_MOUSEMOVE",
            WM_LBUTTONDOWN => "WM_LBUTTONDOWN",
            WM_LBUTTONUP => "WM_LBUTTONUP",
            WM_LBUTTONDBLCLK => "WM_LBUTTONDBLCLK",
            WM_RBUTTONDOWN => "WM_RBUTTONDOWN",
            WM_RBUTTONUP => "WM_RBUTTONUP",
            WM_RBUTTONDBLCLK => "WM_RBUTTONDBLCLK",
            WM_MBUTTONDOWN => "WM_MBUTTONDOWN",
            WM_MBUTTONUP => "WM_MBUTTONUP",
            WM_MBUTTONDBLCLK => "WM_MBUTTONDBLCLK",
            WM_XBUTTONDOWN => "WM_XBUTTONDOWN",
            WM_XBUTTONUP => "WM_XBUTTONUP",
            WM_XBUTTONDBLCLK => "WM_XBUTTONDBLCLK",
            _ => "WM_OTHER",
        }
    }

    let is_pointer_message = matches!(
        msg,
        WM_POINTERDOWN
            | WM_POINTERUP
            | WM_POINTERUPDATE
            | WM_POINTERENTER
            | WM_POINTERLEAVE
            | WM_POINTERCAPTURECHANGED
            | WM_NCPOINTERDOWN
            | WM_NCPOINTERUP
            | WM_NCPOINTERUPDATE
    );
    if is_pointer_message {
        if msg == WM_POINTERUPDATE && !rate_limit_100ms(window.pointer_update_rate_limit()) {
            return;
        }

        let pointer_id = (wparam.0 & 0xFFFF) as u32;
        let mut pointer_type = POINTER_INPUT_TYPE::default();
        let pointer_type_result = unsafe { GetPointerType(pointer_id, &mut pointer_type) };
        let pointer_type_text = match pointer_type_result {
            Ok(()) => format!("{}({})", pointer_type_label(pointer_type), pointer_type.0),
            Err(err) => format!("ERROR({err})"),
        };

        let mut info = POINTER_INFO::default();
        match unsafe { GetPointerInfo(pointer_id, &mut info) } {
            Ok(()) => {
                let screen = info.ptPixelLocation;
                let mut client = screen;
                let client_text = if unsafe { ScreenToClient(hwnd, &mut client) }.as_bool() {
                    format!("({},{})", client.x, client.y)
                } else {
                    "ERROR".to_string()
                };
                let flags = info.pointerFlags;
                crate::logger::log(format!(
                    "[TOUCH-DEBUG] win32 window={} hwnd={:p} msg={} pointer_id={} \
                     type={} screen=({},{}) client={} flags=0x{:08x} \
                     in_contact={} in_range={} primary={} canceled={}",
                    window.label(),
                    hwnd.0,
                    message_name(msg),
                    pointer_id,
                    pointer_type_text,
                    screen.x,
                    screen.y,
                    client_text,
                    flags.0,
                    flags & POINTER_FLAG_INCONTACT == POINTER_FLAG_INCONTACT,
                    flags & POINTER_FLAG_INRANGE == POINTER_FLAG_INRANGE,
                    flags & POINTER_FLAG_PRIMARY == POINTER_FLAG_PRIMARY,
                    flags & POINTER_FLAG_CANCELED == POINTER_FLAG_CANCELED,
                ));
            }
            Err(err) => crate::logger::log(format!(
                "[TOUCH-DEBUG] win32 window={} hwnd={:p} msg={} pointer_id={} \
                 type={} pointer_info=ERROR({err})",
                window.label(),
                hwnd.0,
                message_name(msg),
                pointer_id,
                pointer_type_text,
            )),
        }
        return;
    }

    if matches!(msg, WM_TOUCH | WM_GESTURE) {
        crate::logger::log(format!(
            "[TOUCH-DEBUG] win32 window={} hwnd={:p} msg={} wparam=0x{:x} lparam=0x{:x}",
            window.label(),
            hwnd.0,
            message_name(msg),
            wparam.0,
            lparam.0,
        ));
        return;
    }

    let is_mouse_button = matches!(
        msg,
        WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_LBUTTONDBLCLK
            | WM_RBUTTONDOWN
            | WM_RBUTTONUP
            | WM_RBUTTONDBLCLK
            | WM_MBUTTONDOWN
            | WM_MBUTTONUP
            | WM_MBUTTONDBLCLK
            | WM_XBUTTONDOWN
            | WM_XBUTTONUP
            | WM_XBUTTONDBLCLK
    );
    if msg != WM_MOUSEMOVE && !is_mouse_button {
        return;
    }
    if msg == WM_MOUSEMOVE && !rate_limit_100ms(window.mouse_move_rate_limit()) {
        return;
    }

    let mut source = INPUT_MESSAGE_SOURCE::default();
    match unsafe { GetCurrentInputMessageSource(&mut source) } {
        Ok(()) => crate::logger::log(format!(
            "[TOUCH-DEBUG] win32 window={} hwnd={:p} msg={} source_device={}({}) \
             source_origin={}({})",
            window.label(),
            hwnd.0,
            message_name(msg),
            device_type_label(source.deviceType),
            source.deviceType.0,
            origin_label(source.originId),
            source.originId.0,
        )),
        Err(err) => crate::logger::log(format!(
            "[TOUCH-DEBUG] win32 window={} hwnd={:p} msg={} input_source=ERROR({err})",
            window.label(),
            hwnd.0,
            message_name(msg),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::format_egui_events;

    #[test]
    fn formats_touch_signature_in_original_event_order() {
        let pos = egui::pos2(12.5, 34.0);
        let events = vec![
            egui::Event::Touch {
                device_id: egui::TouchDeviceId(7),
                id: egui::TouchId(11),
                phase: egui::TouchPhase::Start,
                pos,
                force: Some(0.5),
            },
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerGone,
        ];

        assert_eq!(
            format_egui_events(&events),
            "Touch(device_id=7 id=11 phase=Start pos=(12.5,34.0) force=Some(0.5)) -> \
             PointerMoved(pos=(12.5,34.0)) -> \
             PointerButton(button=Primary pressed=true) -> PointerGone"
        );
    }
}
