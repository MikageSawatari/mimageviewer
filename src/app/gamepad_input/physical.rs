use std::time::Instant;

use eframe::egui;

use super::{TRIGGER_THRESHOLD, stick_pair};
use crate::gamepad::{
    GamepadInputState, GamepadRuntime, PadAxis, PadButton, PadEvent, PadEventReceipt,
    foreground_app_hwnd,
};

/// Physical facts, in receipt order. Terminals must not overwrite undelivered
/// edges: a press and release can arrive in different logic calls before UI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PadPhysicalEvent {
    ButtonPressed(PadButton),
    ButtonReleased(PadButton),
    SuppressedRelease(PadButton),
    AxesChanged([f32; 6]),
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
            self.state.suppress_physical_actions();
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
            self.state.clear_physical();
            self.pending.events.push(PadPhysicalEvent::SessionEnded);
            return;
        }
        self.pending.analog_operation |= self.analog_active();
        for receipt in events {
            self.pending.saw_input_event = true;
            match receipt.event {
                PadEvent::ButtonPressed(button) => {
                    let changed = self.state.set_physical_button_down(button, true, now);
                    if !receipt.accepted {
                        self.state.suppress_physical_actions();
                    } else if changed {
                        self.pending
                            .events
                            .push(PadPhysicalEvent::ButtonPressed(button));
                    }
                }
                PadEvent::ButtonReleased(button) => {
                    let admitted = self.state.button_release_admitted(button) && receipt.accepted;
                    if self.state.set_physical_button_down(button, false, now) {
                        self.pending.events.push(if admitted {
                            PadPhysicalEvent::ButtonReleased(button)
                        } else {
                            PadPhysicalEvent::SuppressedRelease(button)
                        });
                    }
                }
                PadEvent::AxisChanged(axis, value) => {
                    self.state.set_physical_axis(axis, value);
                    let values = self.state.physical_axes();
                    if let Some(PadPhysicalEvent::AxesChanged(previous)) =
                        self.pending.events.last_mut()
                    {
                        *previous = values;
                    } else {
                        self.pending
                            .events
                            .push(PadPhysicalEvent::AxesChanged(values));
                    }
                    self.pending.analog_operation |= self.analog_active();
                }
                PadEvent::Connected => {}
                PadEvent::Disconnected => {
                    // Clear here, before any subsequent press in this drain.
                    // UI overlay cleanup and context-owned ring commit are separate.
                    self.state.clear_physical();
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
    fn gamepad_receipt_keeps_short_background_press_rejected_after_focus_returns() {
        let _foreground = crate::test_native_focus::scoped(1, 1, true);
        let ctx = egui::Context::default();
        let mut input = GamepadPhysicalInput::default();
        let now = Instant::now();
        // The producer saw another app. No logic poll happened before mIV returned.
        for event in [
            PadEvent::ButtonPressed(PadButton::DPadRight),
            PadEvent::AxisChanged(PadAxis::LeftX, 0.9),
            PadEvent::ButtonReleased(PadButton::DPadRight),
            PadEvent::ButtonPressed(PadButton::North),
            PadEvent::ButtonReleased(PadButton::North),
            PadEvent::ButtonPressed(PadButton::West),
            PadEvent::ButtonReleased(PadButton::West),
        ] {
            input.push_receipt_for_test(event, None);
        }
        input.push_receipt_for_test(PadEvent::ButtonPressed(PadButton::South), Some(1));
        input.drain(&ctx, true, now);
        assert_eq!(input.state.axis(PadAxis::LeftX), 0.9);
        assert!(!input.state.button_down(PadButton::DPadRight));
        assert!(input.state.button_down(PadButton::South));
        assert!(!input.state.due_button_repeat(
            PadButton::DPadRight,
            now + std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(70),
        ));
        assert_eq!(
            input.take_pending().events,
            [
                PadPhysicalEvent::AxesChanged([0.9, 0.0, 0.0, 0.0, 0.0, 0.0]),
                PadPhysicalEvent::SuppressedRelease(PadButton::DPadRight),
                PadPhysicalEvent::SuppressedRelease(PadButton::North),
                PadPhysicalEvent::SuppressedRelease(PadButton::West),
                PadPhysicalEvent::ButtonPressed(PadButton::South),
            ]
        );
        assert!(input.take_pending().events.is_empty());
    }

    #[test]
    fn gamepad_background_logic_stops_discrete_repeat_but_keeps_the_axis() {
        let ctx = egui::Context::default();
        let mut input = GamepadPhysicalInput::default();
        let now = Instant::now();
        {
            let _foreground = crate::test_native_focus::scoped(1, 1, true);
            input.push_for_test(PadEvent::ButtonPressed(PadButton::DPadRight));
            input.push_for_test(PadEvent::AxisChanged(PadAxis::LeftX, 0.9));
            input.drain(&ctx, true, now);
        }
        {
            let _background = crate::test_native_focus::scoped(2, 2, false);
            input.drain(&ctx, true, now + std::time::Duration::from_secs(1));
            input.drain(&ctx, true, now + std::time::Duration::from_secs(2));
        }
        let _foreground = crate::test_native_focus::scoped(1, 1, true);
        input.drain(&ctx, true, now + std::time::Duration::from_secs(3));
        assert!(input.state.button_down(PadButton::DPadRight));
        assert_eq!(input.state.axis(PadAxis::LeftX), 0.9);
        assert!(!input.state.directional_neutral_required());
        assert!(!input.state.due_button_repeat(
            PadButton::DPadRight,
            now + std::time::Duration::from_secs(3),
            std::time::Duration::from_millis(70)
        ));
        assert!(input.state.left_stick_step_due(
            true,
            now + std::time::Duration::from_secs(3),
            std::time::Duration::from_millis(70)
        ));
        // The earlier admitted edge was never overwritten by the neutral polls.
        assert_eq!(
            input.take_pending().events,
            [
                PadPhysicalEvent::ButtonPressed(PadButton::DPadRight),
                PadPhysicalEvent::AxesChanged([0.9, 0.0, 0.0, 0.0, 0.0, 0.0]),
            ]
        );
    }

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
                PadPhysicalEvent::AxesChanged([0.9, 0.0, 0.0, 0.0, 0.0, 0.0]),
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
        assert_eq!(handoff.events, [PadPhysicalEvent::AxesChanged([0.0; 6])]);
        assert!(handoff.analog_operation);
        assert!(!input.take_pending().analog_operation);
    }
}
