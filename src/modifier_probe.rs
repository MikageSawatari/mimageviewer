//! Passive modifier-state instrumentation for input diagnosis.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::keymap::{ModKind, modifier_held_via_os};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
const MAX_KEYS_PER_EVENT: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ModifierLevels {
    ctrl: bool,
    shift: bool,
    alt: bool,
}

impl ModifierLevels {
    fn from_egui(modifiers: egui::Modifiers) -> Self {
        Self {
            ctrl: modifiers.ctrl,
            shift: modifiers.shift,
            alt: modifiers.alt,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ModifierSnapshot {
    focused: bool,
    egui: ModifierLevels,
    egui_command: bool,
    os: Option<ModifierLevels>,
}

impl ModifierSnapshot {
    fn from_raw_input(input: &egui::RawInput) -> Self {
        Self::new(input.viewport_id, input.focused, input.modifiers)
    }

    fn from_context(ctx: &egui::Context) -> Self {
        let (focused, modifiers) =
            ctx.input(|input| (input.viewport().focused.unwrap_or(true), input.modifiers));
        Self::new(ctx.viewport_id(), focused, modifiers)
    }

    fn new(viewport_id: egui::ViewportId, focused: bool, modifiers: egui::Modifiers) -> Self {
        let egui = ModifierLevels::from_egui(modifiers);
        let permit =
            crate::keyboard_input::focused_key_state_permit_for_viewport(viewport_id, focused);
        let os = permit.map(|permit| ModifierLevels {
            ctrl: modifier_held_via_os(permit, ModKind::Ctrl),
            shift: modifier_held_via_os(permit, ModKind::Shift),
            alt: modifier_held_via_os(permit, ModKind::Alt),
        });
        Self {
            focused,
            egui,
            egui_command: modifiers.command,
            os,
        }
    }
}

const fn modifier_levels_diverged(egui: ModifierLevels, os: ModifierLevels) -> bool {
    egui.ctrl != os.ctrl || egui.shift != os.shift || egui.alt != os.alt
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModifierProbeTrigger {
    KeyEvent,
    Wheel,
    Heartbeat,
}

impl ModifierProbeTrigger {
    const fn as_str(self) -> &'static str {
        match self {
            Self::KeyEvent => "key_event",
            Self::Wheel => "wheel",
            Self::Heartbeat => "heartbeat",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ModifierProbeFrame {
    keys: Vec<String>,
    wheel: bool,
}

impl ModifierProbeFrame {
    fn from_raw_input(input: &egui::RawInput) -> Self {
        let mut keys = Vec::new();
        let mut wheel = false;
        for event in &input.events {
            match event {
                egui::Event::Key { key, pressed, .. } if keys.len() < MAX_KEYS_PER_EVENT => {
                    keys.push(format!(
                        "{key:?}:{}",
                        if *pressed { "pressed" } else { "released" }
                    ));
                }
                egui::Event::MouseWheel { .. } => wheel = true,
                _ => {}
            }
        }
        Self { keys, wheel }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ModifierProbeEmission {
    trigger: ModifierProbeTrigger,
    frame: ModifierProbeFrame,
    snapshot: ModifierSnapshot,
}

#[derive(Clone, Copy, Debug)]
struct ViewportProbeTiming {
    last_emit_at: Instant,
    last_emitted_frame_token: Option<u64>,
}

#[derive(Default)]
struct ModifierProbeState {
    viewports: HashMap<egui::ViewportId, ViewportProbeTiming>,
}

impl ModifierProbeState {
    fn next(
        &mut self,
        viewport_id: egui::ViewportId,
        frame_token: Option<u64>,
        now: Instant,
        frame: &ModifierProbeFrame,
    ) -> Option<ModifierProbeTrigger> {
        let timing = self
            .viewports
            .entry(viewport_id)
            .or_insert(ViewportProbeTiming {
                last_emit_at: now,
                last_emitted_frame_token: None,
            });
        if frame_token.is_some() && timing.last_emitted_frame_token == frame_token {
            return None;
        }

        // The trigger is deliberately decided only from input-event presence and elapsed time.
        // The modifier snapshot is payload and cannot suppress the event.
        let trigger = if !frame.keys.is_empty() {
            ModifierProbeTrigger::KeyEvent
        } else if frame.wheel {
            ModifierProbeTrigger::Wheel
        } else if now.saturating_duration_since(timing.last_emit_at) >= HEARTBEAT_INTERVAL {
            ModifierProbeTrigger::Heartbeat
        } else {
            return None;
        };

        timing.last_emit_at = now;
        timing.last_emitted_frame_token = frame_token;
        Some(trigger)
    }
}

#[derive(Default)]
struct ModifierProbePlugin {
    state: ModifierProbeState,
}

impl egui::Plugin for ModifierProbePlugin {
    fn debug_name(&self) -> &'static str {
        "miv_modifier_probe"
    }

    fn input_hook(&mut self, input: &mut egui::RawInput) {
        if !crate::perf::is_enabled() {
            return;
        }
        let frame = ModifierProbeFrame::from_raw_input(input);
        let frame_token = input.time.map(f64::to_bits);
        if let Some(trigger) =
            self.state
                .next(input.viewport_id, frame_token, Instant::now(), &frame)
        {
            let snapshot = ModifierSnapshot::from_raw_input(input);
            let emission = ModifierProbeEmission {
                trigger,
                frame,
                snapshot,
            };
            emit_modifier_probe(input.viewport_id, emission);
        }
    }
}

pub(crate) fn install(ctx: &egui::Context) {
    ctx.add_plugin(ModifierProbePlugin::default());
}

pub(crate) fn record_modified_action(
    ctx: &egui::Context,
    action: &'static str,
    source: &'static str,
) {
    if !crate::perf::is_enabled() {
        return;
    }
    let snapshot = ModifierSnapshot::from_context(ctx);
    let mut fields = snapshot_fields(viewport_label(ctx.viewport_id()), snapshot);
    fields.push(("action", Value::from(action)));
    fields.push(("source", Value::from(source)));
    crate::perf::event("input", "modified_action", None, 0, &fields);
}

fn emit_modifier_probe(viewport_id: egui::ViewportId, emission: ModifierProbeEmission) {
    let mut fields = snapshot_fields(viewport_label(viewport_id), emission.snapshot);
    fields.push((
        "keys",
        Value::Array(emission.frame.keys.into_iter().map(Value::from).collect()),
    ));
    fields.push(("wheel", Value::from(emission.frame.wheel)));
    fields.push(("trigger", Value::from(emission.trigger.as_str())));
    crate::perf::event("input", "modifier_probe", None, 0, &fields);
}

fn snapshot_fields(
    viewport: &'static str,
    snapshot: ModifierSnapshot,
) -> Vec<(&'static str, Value)> {
    let mut fields = vec![
        ("viewport", Value::from(viewport)),
        ("focused", Value::from(snapshot.focused)),
        ("egui_ctrl", Value::from(snapshot.egui.ctrl)),
        ("egui_shift", Value::from(snapshot.egui.shift)),
        ("egui_alt", Value::from(snapshot.egui.alt)),
        ("egui_command", Value::from(snapshot.egui_command)),
        ("permit", Value::from(snapshot.os.is_some())),
    ];
    if let Some(os) = snapshot.os {
        fields.extend([
            ("os_ctrl", Value::from(os.ctrl)),
            ("os_shift", Value::from(os.shift)),
            ("os_alt", Value::from(os.alt)),
            (
                "diverged",
                Value::from(modifier_levels_diverged(snapshot.egui, os)),
            ),
        ]);
    }
    fields
}

fn viewport_label(viewport_id: egui::ViewportId) -> &'static str {
    if viewport_id == egui::ViewportId::ROOT {
        "main"
    } else {
        "fullscreen"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(bits: u8) -> ModifierLevels {
        ModifierLevels {
            ctrl: bits & 0b001 != 0,
            shift: bits & 0b010 != 0,
            alt: bits & 0b100 != 0,
        }
    }

    fn snapshot(value: bool) -> ModifierSnapshot {
        ModifierSnapshot {
            focused: true,
            egui: ModifierLevels {
                ctrl: value,
                shift: value,
                alt: value,
            },
            egui_command: value,
            os: Some(ModifierLevels {
                ctrl: value,
                shift: value,
                alt: value,
            }),
        }
    }

    fn next_emission(
        state: &mut ModifierProbeState,
        viewport: egui::ViewportId,
        frame_token: Option<u64>,
        now: Instant,
        frame: ModifierProbeFrame,
        snapshot: ModifierSnapshot,
    ) -> Option<ModifierProbeEmission> {
        state
            .next(viewport, frame_token, now, &frame)
            .map(|trigger| ModifierProbeEmission {
                trigger,
                frame,
                snapshot,
            })
    }

    #[test]
    fn modifier_divergence_truth_table() {
        for egui_bits in 0_u8..8 {
            for os_bits in 0_u8..8 {
                assert_eq!(
                    modifier_levels_diverged(levels(egui_bits), levels(os_bits)),
                    egui_bits != os_bits
                );
            }
        }
    }

    #[test]
    fn modifier_probe_triggers_ignore_modifier_values() {
        let viewport = egui::ViewportId::ROOT;
        let start = Instant::now();
        for modifier_value in [false, true] {
            let mut state = ModifierProbeState::default();
            let emission = next_emission(
                &mut state,
                viewport,
                Some(1),
                start,
                ModifierProbeFrame {
                    keys: vec![String::new()],
                    wheel: false,
                },
                snapshot(modifier_value),
            );
            assert_eq!(emission.unwrap().trigger, ModifierProbeTrigger::KeyEvent);

            let mut state = ModifierProbeState::default();
            let emission = next_emission(
                &mut state,
                viewport,
                Some(1),
                start,
                ModifierProbeFrame {
                    keys: Vec::new(),
                    wheel: true,
                },
                snapshot(modifier_value),
            );
            assert_eq!(emission.unwrap().trigger, ModifierProbeTrigger::Wheel);

            let mut state = ModifierProbeState::default();
            assert!(
                next_emission(
                    &mut state,
                    viewport,
                    Some(1),
                    start,
                    ModifierProbeFrame::default(),
                    snapshot(modifier_value),
                )
                .is_none()
            );
            let emission = next_emission(
                &mut state,
                viewport,
                Some(2),
                start + HEARTBEAT_INTERVAL,
                ModifierProbeFrame::default(),
                snapshot(modifier_value),
            );
            assert_eq!(emission.unwrap().trigger, ModifierProbeTrigger::Heartbeat);
        }
    }

    #[test]
    fn modifier_probe_heartbeat_does_not_request_repaint() {
        let production_source = include_str!("modifier_probe.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(!production_source.contains("request_repaint"));
        assert!(!production_source.contains("request_repaint_after"));
    }
}
