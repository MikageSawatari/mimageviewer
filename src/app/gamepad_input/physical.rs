use std::time::Instant;

use eframe::egui;

use super::{TRIGGER_THRESHOLD, stick_pair};
use crate::gamepad::{
    GamepadInputState, GamepadRuntime, PadAxis, PadButton, PadEvent, PadEventReceipt,
    foreground_app_hwnd,
};

/// Physical facts, in receipt order. Terminals must not overwrite undelivered
/// edges: a press and release can arrive in different logic calls before UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PadPhysicalEvent {
    ButtonPressed(PadButton),
    ButtonReleased(PadButton),
    SuppressedRelease(PadButton),
    SessionEnded,
}

#[derive(Default)]
pub(super) struct PadPhysicalHandoff {
    pub(super) events: Vec<PadPhysicalEvent>,
    pub(super) saw_input_event: bool,
    pub(super) analog_operation: bool,
}

/// One App-global device/held-state owner, independent of the mounted viewer.
/// Pending physical facts are transferred once, separately from UI batch replay.
/// Context is used only by the runtime's existing wake registration, never input.
#[derive(Default)]
pub(crate) struct GamepadPhysicalInput {
    runtime: GamepadRuntime,
    pub(crate) state: GamepadInputState,
    pending: PadPhysicalHandoff,
}

impl GamepadPhysicalInput {
    pub(super) fn drain(&mut self, ctx: &egui::Context, enabled: bool, now: Instant) {
        let events = self.runtime.drain(ctx, enabled);
        self.ingest(enabled, events, now);
        if foreground_app_hwnd().is_none() {
            self.state.suppress_pending_actions();
            let _ = self.state.analog_dt(false, now);
            let _ = self
                .state
                .left_stick_step_due(false, now, super::STICK_STEP_INTERVAL);
            let _ = self
                .state
                .trigger_step_due(false, now, super::STICK_STEP_INTERVAL);
        }
    }

    fn ingest(&mut self, enabled: bool, events: Vec<PadEventReceipt>, now: Instant) {
        if !enabled {
            self.state.clear();
            self.pending.events.push(PadPhysicalEvent::SessionEnded);
            return;
        }
        self.pending.analog_operation |= self.analog_active();
        for receipt in events {
            self.pending.saw_input_event = true;
            match receipt.event {
                PadEvent::ButtonPressed(button) => {
                    let changed = self.state.set_button_down(button, true, now);
                    if !receipt.accepted {
                        self.state.suppress_pending_actions();
                    } else if changed {
                        self.pending
                            .events
                            .push(PadPhysicalEvent::ButtonPressed(button));
                    }
                }
                PadEvent::ButtonReleased(button) => {
                    let admitted = self.state.button_release_admitted(button);
                    if self.state.set_button_down(button, false, now) {
                        self.pending.events.push(if admitted {
                            PadPhysicalEvent::ButtonReleased(button)
                        } else {
                            PadPhysicalEvent::SuppressedRelease(button)
                        });
                    }
                }
                PadEvent::AxisChanged(axis, value) => {
                    self.state.set_axis(axis, value);
                    self.pending.analog_operation |= self.analog_active();
                }
                PadEvent::Connected => {}
                PadEvent::Disconnected => {
                    // Clear here, before any subsequent press in this drain.
                    // UI overlay cleanup and context-owned ring commit are separate.
                    self.state.clear();
                    self.pending.events.push(PadPhysicalEvent::SessionEnded);
                }
            }
        }
    }

    pub(super) fn take_pending(&mut self) -> PadPhysicalHandoff {
        std::mem::take(&mut self.pending)
    }

    pub(super) fn analog_active(&self) -> bool {
        let left = stick_pair(&self.state, PadAxis::LeftX, PadAxis::LeftY);
        let right = stick_pair(&self.state, PadAxis::RightX, PadAxis::RightY);
        left.length_sq() > 0.0
            || right.length_sq() > 0.0
            || self.state.trigger_value(true) > TRIGGER_THRESHOLD
            || self.state.trigger_value(false) > TRIGGER_THRESHOLD
    }

    #[cfg(test)]
    pub(crate) fn push_receipt_for_test(&mut self, event: PadEvent, foreground: Option<u64>) {
        self.runtime.push_receipt_for_test(event, foreground);
    }

    #[cfg(test)]
    pub(crate) fn push_for_test(&mut self, event: PadEvent) {
        self.runtime.push_for_test(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamepad_physical_handoff_keeps_press_release_across_two_drains() {
        let _foreground = crate::test_native_focus::scoped(1, 1, true);
        let ctx = egui::Context::default();
        let mut input = GamepadPhysicalInput::default();
        let now = Instant::now();
        input.push_for_test(PadEvent::ButtonPressed(PadButton::South));
        input.drain(&ctx, true, now);
        input.push_for_test(PadEvent::ButtonReleased(PadButton::South));
        input.drain(&ctx, true, now);

        assert!(!input.state.button_down(PadButton::South));
        let handoff = input.take_pending();
        assert_eq!(
            handoff.events,
            [
                PadPhysicalEvent::ButtonPressed(PadButton::South),
                PadPhysicalEvent::ButtonReleased(PadButton::South),
            ]
        );
        assert!(handoff.saw_input_event);
        let empty = input.take_pending();
        assert!(empty.events.is_empty());
        assert!(!empty.saw_input_event);
        assert!(!empty.analog_operation);
    }

    #[test]
    fn gamepad_physical_handoff_keeps_terminals_and_post_disconnect_press() {
        let _foreground = crate::test_native_focus::scoped(1, 1, true);
        let ctx = egui::Context::default();
        let mut input = GamepadPhysicalInput::default();
        let now = Instant::now();
        input.push_for_test(PadEvent::ButtonPressed(PadButton::West));
        input.drain(&ctx, true, now);
        for event in [
            PadEvent::Disconnected,
            PadEvent::Connected,
            PadEvent::ButtonPressed(PadButton::North),
            PadEvent::Disconnected,
            PadEvent::ButtonPressed(PadButton::South),
        ] {
            input.push_for_test(event);
        }
        input.drain(&ctx, true, now);

        assert!(!input.state.button_down(PadButton::West));
        assert!(!input.state.button_down(PadButton::North));
        assert!(input.state.button_down(PadButton::South));
        assert_eq!(
            input.take_pending().events,
            [
                PadPhysicalEvent::ButtonPressed(PadButton::West),
                PadPhysicalEvent::SessionEnded,
                PadPhysicalEvent::ButtonPressed(PadButton::North),
                PadPhysicalEvent::SessionEnded,
                PadPhysicalEvent::ButtonPressed(PadButton::South),
            ]
        );
    }

    #[test]
    fn gamepad_physical_disable_clears_holds_without_overwriting_pending_edge() {
        let _foreground = crate::test_native_focus::scoped(1, 1, true);
        let ctx = egui::Context::default();
        let mut input = GamepadPhysicalInput::default();
        let now = Instant::now();
        input.push_for_test(PadEvent::ButtonPressed(PadButton::DPadRight));
        input.push_for_test(PadEvent::AxisChanged(PadAxis::LeftX, 0.9));
        input.drain(&ctx, true, now);
        input.drain(&ctx, false, now);

        assert!(input.state.is_idle());
        let handoff = input.take_pending();
        assert_eq!(
            handoff.events,
            [
                PadPhysicalEvent::ButtonPressed(PadButton::DPadRight),
                PadPhysicalEvent::SessionEnded,
            ]
        );
        assert!(handoff.saw_input_event);
        assert!(handoff.analog_operation);
    }

    #[test]
    fn gamepad_physical_handoff_retains_analog_fact_after_later_neutral() {
        let _foreground = crate::test_native_focus::scoped(1, 1, true);
        let ctx = egui::Context::default();
        let mut input = GamepadPhysicalInput::default();
        let now = Instant::now();
        input.push_for_test(PadEvent::AxisChanged(PadAxis::LeftX, 0.9));
        input.drain(&ctx, true, now);
        input.push_for_test(PadEvent::AxisChanged(PadAxis::LeftX, 0.0));
        input.drain(&ctx, true, now);

        assert!(!input.analog_active());
        let handoff = input.take_pending();
        assert!(handoff.events.is_empty());
        assert!(handoff.analog_operation);
        assert!(!input.take_pending().analog_operation);
    }
}
