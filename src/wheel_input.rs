//! Wheel reads and consumption at the existing UI input owner.
//!
//! This boundary preserves egui 0.33's unit conversion, modifier-axis mapping,
//! and pass-wide aggregation. Read it only where the consumer already runs,
//! after any front popup has had the opportunity to consume its input.

use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WheelInput {
    pub(crate) delta: egui::Vec2,
    pub(crate) modifiers: egui::Modifiers,
}

/// Read the remaining unsmoothed wheel delta, never the immutable raw events.
///
/// The pass-wide modifiers are intentional here: changing to per-event routing
/// belongs to the later egui migration, rather than this owner extraction.
pub(crate) fn remaining_wheel_input(input: &egui::InputState) -> WheelInput {
    WheelInput {
        delta: input.raw_scroll_delta,
        modifiers: input.modifiers,
    }
}

/// Consume all public wheel representations together for this viewport/pass.
///
/// Call after the accepting owner has read/applied the input. This does not
/// terminate egui's internal smoothing tail at a popup lifecycle boundary.
pub(crate) fn consume_wheel_input(ctx: &egui::Context) {
    ctx.input_mut(|input| {
        input.raw_scroll_delta = egui::Vec2::ZERO;
        input.smooth_scroll_delta = egui::Vec2::ZERO;
        input
            .events
            .retain(|event| !matches!(event, egui::Event::MouseWheel { .. }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(
        unit: egui::MouseWheelUnit,
        delta: egui::Vec2,
        modifiers: egui::Modifiers,
    ) -> egui::Event {
        egui::Event::MouseWheel {
            unit,
            delta,
            modifiers,
        }
    }

    fn input(events: Vec<egui::Event>, modifiers: egui::Modifiers) -> egui::InputState {
        egui::InputState::default().begin_pass(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500.0, 300.0),
                )),
                events,
                modifiers,
                ..Default::default()
            },
            false,
            1.0,
            egui::InputOptions {
                line_scroll_speed: 13.0,
                ..Default::default()
            },
        )
    }

    #[test]
    fn egui037_wheel_extraction_preserves_legacy_unit_conversion() {
        for (unit, scale) in [
            (egui::MouseWheelUnit::Point, 1.0),
            (egui::MouseWheelUnit::Line, 13.0),
            (egui::MouseWheelUnit::Page, 300.0),
        ] {
            let delta = egui::vec2(2.0, -3.0);
            let state = input(
                vec![wheel(unit, delta, egui::Modifiers::NONE)],
                egui::Modifiers::NONE,
            );
            assert_eq!(remaining_wheel_input(&state).delta, scale * delta);
        }
    }

    #[test]
    fn egui037_wheel_extraction_preserves_event_modifier_axis_mapping() {
        for (event_modifiers, expected) in [
            (egui::Modifiers::NONE, egui::vec2(2.0, 3.0)),
            (egui::Modifiers::SHIFT, egui::vec2(5.0, 0.0)),
            (egui::Modifiers::ALT, egui::vec2(0.0, 5.0)),
            (
                egui::Modifiers::SHIFT | egui::Modifiers::ALT,
                egui::vec2(2.0, 3.0),
            ),
        ] {
            let state = input(
                vec![wheel(
                    egui::MouseWheelUnit::Point,
                    egui::vec2(2.0, 3.0),
                    event_modifiers,
                )],
                egui::Modifiers::CTRL,
            );
            let remaining = remaining_wheel_input(&state);
            assert_eq!(remaining.delta, expected);
            assert_eq!(remaining.modifiers, egui::Modifiers::CTRL);
        }
    }

    #[test]
    fn egui037_wheel_extraction_preserves_pass_aggregation_and_ctrl() {
        let state = input(
            vec![
                wheel(
                    egui::MouseWheelUnit::Point,
                    egui::vec2(0.0, 10.0),
                    egui::Modifiers::NONE,
                ),
                wheel(
                    egui::MouseWheelUnit::Point,
                    egui::vec2(0.0, -4.0),
                    egui::Modifiers::CTRL,
                ),
            ],
            egui::Modifiers::CTRL,
        );
        assert_eq!(
            remaining_wheel_input(&state),
            WheelInput {
                delta: egui::vec2(0.0, 6.0),
                modifiers: egui::Modifiers::CTRL,
            }
        );
    }

    #[test]
    fn egui037_wheel_consumption_cannot_resurrect_from_raw_events() {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput {
            modifiers: egui::Modifiers::CTRL,
            events: vec![
                egui::Event::Copy,
                wheel(
                    egui::MouseWheelUnit::Point,
                    egui::vec2(0.0, 3.0),
                    egui::Modifiers::NONE,
                ),
            ],
            ..Default::default()
        });
        assert_eq!(ctx.input(remaining_wheel_input).delta.y, 3.0);
        assert!(ctx.input(|input| input.smooth_scroll_delta.y > 0.0));

        consume_wheel_input(&ctx);
        consume_wheel_input(&ctx);

        ctx.input(|input| {
            assert_eq!(remaining_wheel_input(input).delta, egui::Vec2::ZERO);
            assert_eq!(input.smooth_scroll_delta, egui::Vec2::ZERO);
            assert!(matches!(input.events.as_slice(), [egui::Event::Copy]));
            assert!(
                input
                    .raw
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::MouseWheel { .. }))
            );
            assert_eq!(input.modifiers, egui::Modifiers::CTRL);
        });
        crate::egui_output::discard_headless_output(ctx.end_pass());
    }
}
