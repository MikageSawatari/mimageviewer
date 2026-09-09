//! Opt-in, behavior-neutral provenance trace for native video Back/Forward input.
//!
//! This trace is intentionally narrow and bounded. It records only Browser
//! Back/Forward, comparison Left/Right keys, and XButton1/XButton2. Enabling it
//! must not change dispatch, repeat, generation, overlay, or App decisions.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::video::native_touch::NativeVideoWindowSource;
use crate::video::native_window::{
    NativeVideoMouseButton, NativeVideoWindowEvent, NativeVideoWindowEventEnvelope,
};

const TRACE_ENV: &str = "MIV_MOUSE_SEEK_DEBUG";
const TRACE_DATA_LINE_LIMIT: u32 = 4096;

static TRACE_ENABLED: OnceLock<bool> = OnceLock::new();
static NEXT_INPUT_RECEIPT_ID: AtomicU64 = AtomicU64::new(1);
static TRACE_DATA_LINES: AtomicU32 = AtomicU32::new(0);
static TRACE_CAP_REPORTED: AtomicBool = AtomicBool::new(false);

/// Stable identity for one decoded input receipt. Route-local envelope sequence
/// numbers are deliberately separate because pump/render clones are resequenced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeVideoInputReceipt {
    pub(crate) input_receipt_id: u64,
    pub(crate) origin: NativeVideoInputOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeVideoInputOrigin {
    Win32Key {
        window_source: NativeVideoWindowSource,
        receiver_hwnd: u64,
        message: u32,
    },
    Win32AppCommand {
        window_source: NativeVideoWindowSource,
        receiver_hwnd: u64,
        command_source_hwnd: u64,
        raw_command_word: u16,
        command: u16,
        device_flags: u16,
        key_state: u16,
    },
    Win32MouseButton {
        window_source: NativeVideoWindowSource,
        receiver_hwnd: u64,
        message: u32,
        button_code: u16,
    },
    Win32MouseCleanup {
        window_source: NativeVideoWindowSource,
        receiver_hwnd: u64,
        cause_message: u32,
    },
    EguiBackdrop {
        viewport_id: egui::ViewportId,
    },
    Gamepad,
    #[cfg(test)]
    Test,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GetMessageRemoval {
    NoRemove,
    Remove,
    Unknown(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookCandidate {
    Key {
        message: u32,
        virtual_key: u32,
        scan_code: u16,
        extended: bool,
        repeat: bool,
    },
    AppCommand {
        command_source_hwnd: u64,
        raw_command_word: u16,
        command: u16,
        device_flags: u16,
        key_state: u16,
    },
}

pub(crate) fn is_enabled() -> bool {
    *TRACE_ENABLED.get_or_init(|| {
        std::env::var(TRACE_ENV)
            .map(|value| debug_env_value_enabled(&value))
            .unwrap_or(false)
    })
}

fn debug_env_value_enabled(value: &str) -> bool {
    let value = value.trim();
    !(value.is_empty()
        || value == "0"
        || value.eq_ignore_ascii_case("false")
        || value.eq_ignore_ascii_case("off")
        || value.eq_ignore_ascii_case("no"))
}

fn should_prepare_record() -> bool {
    is_enabled() && !TRACE_CAP_REPORTED.load(Ordering::Acquire)
}

fn next_receipt(origin: NativeVideoInputOrigin) -> NativeVideoInputReceipt {
    NativeVideoInputReceipt {
        input_receipt_id: NEXT_INPUT_RECEIPT_ID.fetch_add(1, Ordering::Relaxed),
        origin,
    }
}

pub(crate) fn win32_key_receipt(
    window_source: NativeVideoWindowSource,
    receiver_hwnd: u64,
    message: u32,
) -> NativeVideoInputReceipt {
    next_receipt(NativeVideoInputOrigin::Win32Key {
        window_source,
        receiver_hwnd,
        message,
    })
}

pub(crate) fn win32_app_command_receipt(
    window_source: NativeVideoWindowSource,
    receiver_hwnd: u64,
    command_source_hwnd: u64,
    lparam: isize,
) -> NativeVideoInputReceipt {
    let key_state = (lparam as u64 & 0xffff) as u16;
    let raw_command_word = ((lparam as u64 >> 16) & 0xffff) as u16;
    next_receipt(NativeVideoInputOrigin::Win32AppCommand {
        window_source,
        receiver_hwnd,
        command_source_hwnd,
        raw_command_word,
        command: raw_command_word & 0x0fff,
        device_flags: raw_command_word & 0xf000,
        key_state,
    })
}

pub(crate) fn win32_mouse_button_receipt(
    window_source: NativeVideoWindowSource,
    receiver_hwnd: u64,
    message: u32,
    button_code: u16,
) -> NativeVideoInputReceipt {
    next_receipt(NativeVideoInputOrigin::Win32MouseButton {
        window_source,
        receiver_hwnd,
        message,
        button_code,
    })
}

pub(crate) fn win32_mouse_cleanup_receipt(
    window_source: NativeVideoWindowSource,
    receiver_hwnd: u64,
    cause_message: u32,
) -> NativeVideoInputReceipt {
    next_receipt(NativeVideoInputOrigin::Win32MouseCleanup {
        window_source,
        receiver_hwnd,
        cause_message,
    })
}

pub(crate) fn egui_backdrop_receipt(viewport_id: egui::ViewportId) -> NativeVideoInputReceipt {
    next_receipt(NativeVideoInputOrigin::EguiBackdrop { viewport_id })
}

pub(crate) fn gamepad_receipt() -> NativeVideoInputReceipt {
    next_receipt(NativeVideoInputOrigin::Gamepad)
}

#[cfg(test)]
pub(crate) fn test_receipt(input_receipt_id: u64) -> NativeVideoInputReceipt {
    NativeVideoInputReceipt {
        input_receipt_id,
        origin: NativeVideoInputOrigin::Test,
    }
}

pub(crate) fn is_comparison_key(virtual_key: u32) -> bool {
    matches!(virtual_key, 0x25 | 0x27 | 0xA6 | 0xA7)
}

fn candidate_event(event: &NativeVideoWindowEvent) -> Option<String> {
    match event {
        NativeVideoWindowEvent::KeyDown(key) | NativeVideoWindowEvent::KeyUp(key)
            if is_comparison_key(key.virtual_key) =>
        {
            Some(format!(
                "kind=key edge={} vk=0x{:02X} scan=0x{:04X} extended={} repeat={} ctrl={} shift={} alt={}",
                if matches!(event, NativeVideoWindowEvent::KeyDown(_)) {
                    "down"
                } else {
                    "up"
                },
                key.virtual_key,
                key.scan_code,
                key.extended,
                key.repeat,
                key.ctrl,
                key.shift,
                key.alt,
            ))
        }
        NativeVideoWindowEvent::MouseButton(mouse)
            if matches!(
                mouse.button,
                NativeVideoMouseButton::Extra1 | NativeVideoMouseButton::Extra2
            ) =>
        {
            Some(format!(
                "kind=xbutton button={:?} edge={} double_click={} x={} y={} ctrl={} shift={}",
                mouse.button,
                if mouse.down { "down" } else { "up" },
                mouse.double_click,
                mouse.x,
                mouse.y,
                mouse.ctrl,
                mouse.shift,
            ))
        }
        _ => None,
    }
}

fn event_receipt(event: &NativeVideoWindowEvent) -> Option<NativeVideoInputReceipt> {
    match event {
        NativeVideoWindowEvent::KeyDown(key) | NativeVideoWindowEvent::KeyUp(key) => {
            Some(key.receipt)
        }
        NativeVideoWindowEvent::MouseButton(mouse) => Some(mouse.receipt),
        _ => None,
    }
}

fn emit(fields: impl std::fmt::Display) {
    if !is_enabled() || TRACE_CAP_REPORTED.load(Ordering::Acquire) {
        return;
    }
    match TRACE_DATA_LINES.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
        (count < TRACE_DATA_LINE_LIMIT).then_some(count + 1)
    }) {
        Ok(_) => crate::logger::log(format!("[mouse-seek-input] {fields}")),
        Err(_) if !TRACE_CAP_REPORTED.swap(true, Ordering::AcqRel) => {
            crate::logger::log(format!(
                "[mouse-seek-input] trace_capped data_line_limit={TRACE_DATA_LINE_LIMIT} further_records=dropped"
            ));
        }
        Err(_) => {}
    }
}

fn format_origin(origin: NativeVideoInputOrigin) -> String {
    match origin {
        NativeVideoInputOrigin::Win32Key {
            window_source,
            receiver_hwnd,
            message,
        } => format!(
            "origin=win32_key window_source={window_source:?} receiver_hwnd=0x{receiver_hwnd:X} message=0x{message:04X}"
        ),
        NativeVideoInputOrigin::Win32AppCommand {
            window_source,
            receiver_hwnd,
            command_source_hwnd,
            raw_command_word,
            command,
            device_flags,
            key_state,
        } => format!(
            "origin=win32_appcommand window_source={window_source:?} receiver_hwnd=0x{receiver_hwnd:X} command_source_hwnd=0x{command_source_hwnd:X} raw_command_word=0x{raw_command_word:04X} command={command} device_flags=0x{device_flags:04X} key_state=0x{key_state:04X}"
        ),
        NativeVideoInputOrigin::Win32MouseButton {
            window_source,
            receiver_hwnd,
            message,
            button_code,
        } => format!(
            "origin=win32_mouse window_source={window_source:?} receiver_hwnd=0x{receiver_hwnd:X} message=0x{message:04X} button_code={button_code}"
        ),
        NativeVideoInputOrigin::Win32MouseCleanup {
            window_source,
            receiver_hwnd,
            cause_message,
        } => format!(
            "origin=win32_mouse_cleanup window_source={window_source:?} receiver_hwnd=0x{receiver_hwnd:X} cause_message=0x{cause_message:04X}"
        ),
        NativeVideoInputOrigin::EguiBackdrop { viewport_id } => {
            format!("origin=egui_backdrop viewport_id={viewport_id:?}")
        }
        NativeVideoInputOrigin::Gamepad => "origin=gamepad".to_string(),
        #[cfg(test)]
        NativeVideoInputOrigin::Test => "origin=test".to_string(),
    }
}

fn origin_window_source(origin: NativeVideoInputOrigin) -> Option<NativeVideoWindowSource> {
    match origin {
        NativeVideoInputOrigin::Win32Key { window_source, .. }
        | NativeVideoInputOrigin::Win32AppCommand { window_source, .. }
        | NativeVideoInputOrigin::Win32MouseButton { window_source, .. }
        | NativeVideoInputOrigin::Win32MouseCleanup { window_source, .. } => Some(window_source),
        NativeVideoInputOrigin::EguiBackdrop { .. } | NativeVideoInputOrigin::Gamepad => None,
        #[cfg(test)]
        NativeVideoInputOrigin::Test => None,
    }
}

pub(crate) fn log_producer_enqueued(
    epoch: u64,
    generation: u64,
    source: NativeVideoWindowSource,
    event: &NativeVideoWindowEvent,
) {
    if !should_prepare_record() {
        return;
    }
    let Some(candidate) = candidate_event(event) else {
        return;
    };
    let Some(receipt) = event_receipt(event) else {
        return;
    };
    if let Some(origin_source) = origin_window_source(receipt.origin) {
        debug_assert_eq!(origin_source, source);
    }
    emit(format_args!(
        "stage=producer_enqueued input_receipt_id={} envelope_source={source:?} envelope_epoch={epoch} envelope_generation={generation} {} {candidate}",
        receipt.input_receipt_id,
        format_origin(receipt.origin),
    ));
}

pub(crate) fn log_render_gate(
    envelope: &NativeVideoWindowEventEnvelope,
    current_generation: u64,
    accepted: bool,
) {
    if !should_prepare_record() {
        return;
    }
    let Some(candidate) = candidate_event(&envelope.event) else {
        return;
    };
    let Some(receipt) = event_receipt(&envelope.event) else {
        return;
    };
    emit(format_args!(
        "stage=render_generation_gate disposition={} input_receipt_id={} render_route_sequence={} envelope_source={:?} envelope_epoch={} envelope_generation={} current_generation={} {} {candidate}",
        if accepted { "accepted" } else { "stale" },
        receipt.input_receipt_id,
        envelope.sequence,
        envelope.source,
        envelope.epoch,
        envelope.generation,
        current_generation,
        format_origin(receipt.origin),
    ));
}

pub(crate) fn log_render_disposition(
    envelope: &NativeVideoWindowEventEnvelope,
    forwarded_to_app: bool,
) {
    if !should_prepare_record() {
        return;
    }
    let Some(candidate) = candidate_event(&envelope.event) else {
        return;
    };
    let Some(receipt) = event_receipt(&envelope.event) else {
        return;
    };
    emit(format_args!(
        "stage=render_disposition disposition={} input_receipt_id={} render_route_sequence={} envelope_source={:?} envelope_epoch={} envelope_generation={} {} {candidate}",
        if forwarded_to_app {
            "forwarded_to_app"
        } else {
            "overlay_owned"
        },
        receipt.input_receipt_id,
        envelope.sequence,
        envelope.source,
        envelope.epoch,
        envelope.generation,
        format_origin(receipt.origin),
    ));
}

pub(crate) fn log_backdrop_observation(
    receipt: NativeVideoInputReceipt,
    virtual_key: u32,
    pressed: bool,
    repeat: bool,
) {
    if !should_prepare_record() {
        return;
    }
    if !is_comparison_key(virtual_key) {
        return;
    }
    emit(format_args!(
        "stage=backdrop_observed disposition={} input_receipt_id={} {} kind=key edge={} vk=0x{virtual_key:02X} scan=0x0000 extended=false repeat={repeat}",
        if pressed {
            "dispatch_to_app"
        } else {
            "diagnostic_only"
        },
        receipt.input_receipt_id,
        format_origin(receipt.origin),
        if pressed { "down" } else { "up" },
    ));
}

pub(crate) fn log_gamepad_observation(
    receipt: NativeVideoInputReceipt,
    virtual_key: u32,
    repeat: bool,
) {
    if !should_prepare_record() || !is_comparison_key(virtual_key) {
        return;
    }
    emit(format_args!(
        "stage=gamepad_observed disposition=dispatch_to_app input_receipt_id={} {} kind=key edge=down vk=0x{virtual_key:02X} scan=0x0000 extended=false repeat={repeat}",
        receipt.input_receipt_id,
        format_origin(receipt.origin),
    ));
}

pub(crate) fn log_app_received(event: &NativeVideoWindowEvent) {
    if !should_prepare_record() {
        return;
    }
    let Some(candidate) = candidate_event(event) else {
        return;
    };
    let Some(receipt) = event_receipt(event) else {
        return;
    };
    emit(format_args!(
        "stage=app_received input_receipt_id={} {} {candidate}",
        receipt.input_receipt_id,
        format_origin(receipt.origin),
    ));
}

pub(crate) fn log_app_key(receipt: NativeVideoInputReceipt, outcome: &str) {
    if !should_prepare_record() {
        return;
    }
    emit(format_args!(
        "stage=app_key_outcome input_receipt_id={} {} outcome={outcome}",
        receipt.input_receipt_id,
        format_origin(receipt.origin),
    ));
}

pub(crate) fn log_app_mouse(receipt: NativeVideoInputReceipt, outcome: &str) {
    if !should_prepare_record() {
        return;
    }
    emit(format_args!(
        "stage=app_mouse_outcome input_receipt_id={} {} outcome={outcome}",
        receipt.input_receipt_id,
        format_origin(receipt.origin),
    ));
}

pub(crate) fn log_app_mouse_hold_repeat(
    initial_receipt: NativeVideoInputReceipt,
    slot: &str,
    action: &str,
) {
    if !should_prepare_record() {
        return;
    }
    emit(format_args!(
        "stage=app_mouse_hold_repeat input_receipt_id={} {} slot={slot} action={action}",
        initial_receipt.input_receipt_id,
        format_origin(initial_receipt.origin),
    ));
}

pub(crate) fn classify_hook_candidate(
    message: u32,
    message_wparam: usize,
    message_lparam: isize,
) -> Option<HookCandidate> {
    // WM_KEYDOWN/UP and WM_SYSKEYDOWN/UP.
    if matches!(message, 0x0100 | 0x0101 | 0x0104 | 0x0105) {
        let virtual_key = (message_wparam & 0xff) as u32;
        if !is_comparison_key(virtual_key) {
            return None;
        }
        let raw = message_lparam as u64;
        return Some(HookCandidate::Key {
            message,
            virtual_key,
            scan_code: ((raw >> 16) & 0xff) as u16,
            extended: (raw & (1 << 24)) != 0,
            repeat: (raw & (1 << 30)) != 0,
        });
    }
    // WM_APPCOMMAND.
    if message == 0x0319 {
        let raw_command_word = ((message_lparam as u64 >> 16) & 0xffff) as u16;
        let command = raw_command_word & 0x0fff;
        if !matches!(command, 1 | 2) {
            return None;
        }
        return Some(HookCandidate::AppCommand {
            command_source_hwnd: message_wparam as u64,
            raw_command_word,
            command,
            device_flags: raw_command_word & 0xf000,
            key_state: (message_lparam as u64 & 0xffff) as u16,
        });
    }
    None
}

fn getmessage_removal(hook_wparam: usize) -> GetMessageRemoval {
    match hook_wparam {
        0 => GetMessageRemoval::NoRemove,
        1 => GetMessageRemoval::Remove,
        value => GetMessageRemoval::Unknown(value),
    }
}

pub(crate) fn log_hook_observation(hook_wparam: usize, hwnd: u64, candidate: HookCandidate) {
    if !should_prepare_record() {
        return;
    }
    let removal = getmessage_removal(hook_wparam);
    match candidate {
        HookCandidate::Key {
            message,
            virtual_key,
            scan_code,
            extended,
            repeat,
        } => emit(format_args!(
            "stage=getmessage_hook removal={removal:?} hwnd=0x{hwnd:X} kind=key message=0x{message:04X} vk=0x{virtual_key:02X} scan=0x{scan_code:04X} extended={extended} repeat={repeat}"
        )),
        HookCandidate::AppCommand {
            command_source_hwnd,
            raw_command_word,
            command,
            device_flags,
            key_state,
        } => emit(format_args!(
            "stage=getmessage_hook removal={removal:?} hwnd=0x{hwnd:X} kind=appcommand command_source_hwnd=0x{command_source_hwnd:X} raw_command_word=0x{raw_command_word:04X} command={command} device_flags=0x{device_flags:04X} key_state=0x{key_state:04X}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_classifier_filters_unrelated_keys_and_commands() {
        assert!(classify_hook_candidate(0x0100, 0x41, 0).is_none());
        assert!(classify_hook_candidate(0x0319, 0, 3_isize << 16).is_none());
        assert!(matches!(
            classify_hook_candidate(0x0101, 0xA6, 0),
            Some(HookCandidate::Key {
                virtual_key: 0xA6,
                message: 0x0101,
                ..
            })
        ));
    }

    #[test]
    fn getmessage_hook_removal_values_stay_distinct() {
        assert_eq!(getmessage_removal(0), GetMessageRemoval::NoRemove);
        assert_eq!(getmessage_removal(1), GetMessageRemoval::Remove);
        assert_eq!(getmessage_removal(7), GetMessageRemoval::Unknown(7));
    }

    #[test]
    fn debug_env_false_spellings_stay_disabled() {
        for value in ["", "0", "false", "FALSE", "off", "No", "  false  "] {
            assert!(!debug_env_value_enabled(value), "value={value:?}");
        }
        for value in ["1", "true", "on", "yes", "trace"] {
            assert!(debug_env_value_enabled(value), "value={value:?}");
        }
    }

    #[test]
    fn app_command_classifier_preserves_device_and_key_state_bits() {
        let lparam = ((0x8002_u32 << 16) | 0x0005) as isize;
        assert_eq!(
            classify_hook_candidate(0x0319, 0xCAFE, lparam),
            Some(HookCandidate::AppCommand {
                command_source_hwnd: 0xCAFE,
                raw_command_word: 0x8002,
                command: 2,
                device_flags: 0x8000,
                key_state: 0x0005,
            })
        );
    }

    #[test]
    fn app_command_receipt_keeps_receiver_and_command_source_hwnds() {
        let receipt = win32_app_command_receipt(
            NativeVideoWindowSource::Presenter,
            0x1111,
            0x2222,
            ((0x4001_u32 << 16) | 0x000C) as isize,
        );
        assert!(matches!(
            receipt.origin,
            NativeVideoInputOrigin::Win32AppCommand {
                window_source: NativeVideoWindowSource::Presenter,
                receiver_hwnd: 0x1111,
                command_source_hwnd: 0x2222,
                raw_command_word: 0x4001,
                command: 1,
                device_flags: 0x4000,
                key_state: 0x000C,
            }
        ));
    }

    #[test]
    fn routed_win32_origin_exposes_its_window_source() {
        let receipt = win32_key_receipt(NativeVideoWindowSource::Hud, 0x1234, 0x0100);
        assert_eq!(
            origin_window_source(receipt.origin),
            Some(NativeVideoWindowSource::Hud)
        );
    }
}
