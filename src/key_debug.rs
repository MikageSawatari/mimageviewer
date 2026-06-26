//! Keyboard shortcut diagnostics gated by `MIV_KEY_DEBUG`.
//!
//! This is intentionally side-effect free unless the environment variable is
//! enabled.  It helps verify the Win32 physical-key queue, native video key
//! forwarding, and the final `KeyAction` resolution without touching normal
//! shortcut behavior.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::key_input::KeyEdge;
use crate::keymap::{Chord, KeyAction, KeyContext};

const MAX_ENTRIES: usize = 32;
const PRESSED_DEDUP_WINDOW: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyDebugSource {
    MainWin32,
    NativeVideo,
}

impl KeyDebugSource {
    fn label(self) -> &'static str {
        match self {
            KeyDebugSource::MainWin32 => "main",
            KeyDebugSource::NativeVideo => "native-video",
        }
    }
}

#[derive(Clone)]
struct KeyDebugEntry {
    line: String,
}

#[derive(Default)]
struct KeyDebugState {
    entries: VecDeque<KeyDebugEntry>,
    last_pressed: Option<(KeyAction, Chord, Instant)>,
}

fn state() -> &'static Mutex<KeyDebugState> {
    static STATE: OnceLock<Mutex<KeyDebugState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(KeyDebugState::default()))
}

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("MIV_KEY_DEBUG")
            .map(|value| {
                let value = value.trim();
                !value.is_empty()
                    && !matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "off")
            })
            .unwrap_or(false)
    })
}

pub fn overlay_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        if !enabled() {
            return false;
        }
        if std::env::var("MIV_KEY_DEBUG_OVERLAY")
            .map(|value| {
                let value = value.trim();
                !value.is_empty()
                    && !matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "off")
            })
            .unwrap_or(false)
        {
            return true;
        }
        std::env::var("MIV_KEY_DEBUG")
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "overlay" | "ui" | "both"
                )
            })
            .unwrap_or(false)
    })
}

pub fn record_raw_edge(source: KeyDebugSource, edge: KeyEdge) {
    if !enabled() {
        return;
    }
    let line = format!(
        "raw {:<12} {:<4} vk=0x{:02X} scan=0x{:02X} ext={} repeat={} mods={}",
        source.label(),
        if edge.pressed { "down" } else { "up" },
        edge.virtual_key,
        edge.scan_code,
        edge.extended as u8,
        edge.repeat as u8,
        modifier_label(edge.ctrl, edge.shift, edge.alt)
    );
    // Keep the on-screen overlay complete, but avoid flooding the log with
    // every key-up and repeat edge.  Action resolution lines are always logged.
    push_line(line, edge.pressed && !edge.repeat);
}

pub fn record_native_video_key(
    key: crate::video::native_window::NativeVideoKeyEvent,
    pressed: bool,
) {
    record_raw_edge(
        KeyDebugSource::NativeVideo,
        KeyEdge {
            virtual_key: key.virtual_key,
            scan_code: key.scan_code,
            extended: key.extended,
            pressed,
            repeat: key.repeat,
            ctrl: key.ctrl,
            shift: key.shift,
            alt: key.alt,
        },
    );
}

pub fn record_consumed_action(
    action: KeyAction,
    context: KeyContext,
    chord: Chord,
    source: &'static str,
) {
    if !enabled() {
        return;
    }
    let line = action_line("consume", action, context, chord, source);
    push_line(line, true);
}

pub fn record_pressed_action(
    action: KeyAction,
    context: KeyContext,
    chord: Chord,
    source: &'static str,
) {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    if let Ok(mut guard) = state().lock() {
        if let Some((last_action, last_chord, last_at)) = guard.last_pressed
            && last_action == action
            && last_chord == chord
            && now.saturating_duration_since(last_at) < PRESSED_DEDUP_WINDOW
        {
            return;
        }
        guard.last_pressed = Some((action, chord, now));
    }
    let line = action_line("pressed", action, context, chord, source);
    push_line(line, true);
}

pub fn render_overlay(ctx: &egui::Context) {
    if !overlay_enabled() {
        return;
    }
    let lines = snapshot_lines();
    egui::Area::new("key_debug_overlay".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_TOP, [-12.0, 72.0])
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(360.0);
                ui.label(egui::RichText::new("MIV_KEY_DEBUG").strong());
                ui.monospace(format!(
                    "frame_active={} key_down={}",
                    crate::key_input::is_frame_active(),
                    crate::key_input::frame_had_key_down()
                ));
                ui.separator();
                if lines.is_empty() {
                    ui.monospace("(no key events)");
                } else {
                    for line in lines.into_iter().rev().take(10) {
                        ui.monospace(line);
                    }
                }
            });
        });
}

fn action_line(
    kind: &'static str,
    action: KeyAction,
    context: KeyContext,
    chord: Chord,
    source: &'static str,
) -> String {
    format!(
        "{kind:<7} {:<28} [{:<12}] chord={} via={}",
        action.ini_name(),
        context.ini_name(),
        chord.display_name(),
        source
    )
}

fn modifier_label(ctrl: bool, shift: bool, alt: bool) -> String {
    let mut parts = Vec::new();
    if ctrl {
        parts.push("Ctrl");
    }
    if shift {
        parts.push("Shift");
    }
    if alt {
        parts.push("Alt");
    }
    if parts.is_empty() {
        "-".to_owned()
    } else {
        parts.join("+")
    }
}

fn push_line(line: String, write_log: bool) {
    if let Ok(mut guard) = state().lock() {
        while guard.entries.len() >= MAX_ENTRIES {
            guard.entries.pop_front();
        }
        guard
            .entries
            .push_back(KeyDebugEntry { line: line.clone() });
    }
    if write_log {
        crate::logger::log(format!("[key-debug] {line}"));
    }
}

fn snapshot_lines() -> Vec<String> {
    state()
        .lock()
        .map(|guard| {
            guard
                .entries
                .iter()
                .map(|entry| entry.line.clone())
                .collect()
        })
        .unwrap_or_default()
}
