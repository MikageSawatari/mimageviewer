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
    pub(crate) phase: egui::TouchPhase,
    pub(crate) source: egui::MouseWheelSource,
}

/// Project only unconsumed events at the existing consumer boundary.
/// Adjacent events share one legacy aggregation unit only while their modifier,
/// source and phase agree. Gesture boundaries carry no movement.
pub(crate) fn remaining_wheel_inputs(
    input: &egui::InputState,
    options: egui::InputOptions,
) -> Vec<WheelInput> {
    let mut segments: Vec<WheelInput> = Vec::new();
    for event in &input.events {
        let egui::Event::MouseWheel {
            unit,
            delta,
            modifiers,
            phase,
            source,
        } = event
        else {
            continue;
        };
        let mut delta = if *phase == egui::TouchPhase::Move {
            match unit {
                egui::MouseWheelUnit::Point => *delta,
                egui::MouseWheelUnit::Line => options.line_scroll_speed * *delta,
                egui::MouseWheelUnit::Page => input.viewport_rect().height() * *delta,
            }
        } else {
            egui::Vec2::ZERO
        };
        let horizontal = modifiers.matches_any(options.horizontal_scroll_modifier);
        let vertical = modifiers.matches_any(options.vertical_scroll_modifier);
        if horizontal && !vertical {
            delta = egui::vec2(delta.x + delta.y, 0.0);
        } else if vertical && !horizontal {
            delta = egui::vec2(0.0, delta.x + delta.y);
        }
        if let Some(previous) = segments.last_mut()
            && previous.modifiers == *modifiers
            && previous.phase == *phase
            && previous.source == *source
            && *phase == egui::TouchPhase::Move
        {
            previous.delta += delta;
        } else {
            segments.push(WheelInput {
                delta,
                modifiers: *modifiers,
                phase: *phase,
                source: *source,
            });
        }
    }
    segments
}

/// Sum the current event projection for diagnostics and hover guards only.
/// Consumers that dispatch by modifiers must use the ordered segments above.
pub(crate) fn remaining_wheel_delta(
    input: &egui::InputState,
    options: egui::InputOptions,
) -> egui::Vec2 {
    remaining_wheel_inputs(input, options)
        .into_iter()
        .fold(egui::Vec2::ZERO, |sum, segment| sum + segment.delta)
}

/// Consume accepted event projections without removing a sibling wheel route.
/// The predicate is evaluated on each event, before same-condition aggregation.
pub(crate) fn consume_remaining_wheel_inputs(
    ctx: &egui::Context,
    mut accepts: impl FnMut(&WheelInput) -> bool,
) {
    let options = ctx.options(|o| o.input_options);
    ctx.input_mut(|input| {
        let height = input.viewport_rect().height();
        let mut consumed = false;
        input.events.retain(|event| {
            let egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
                phase,
                source,
            } = event
            else {
                return true;
            };
            let mut delta = if *phase == egui::TouchPhase::Move {
                match unit {
                    egui::MouseWheelUnit::Point => *delta,
                    egui::MouseWheelUnit::Line => options.line_scroll_speed * *delta,
                    egui::MouseWheelUnit::Page => height * *delta,
                }
            } else {
                egui::Vec2::ZERO
            };
            let horizontal = modifiers.matches_any(options.horizontal_scroll_modifier);
            let vertical = modifiers.matches_any(options.vertical_scroll_modifier);
            if horizontal && !vertical {
                delta = egui::vec2(delta.x + delta.y, 0.0);
            } else if vertical && !horizontal {
                delta = egui::vec2(0.0, delta.x + delta.y);
            }
            let accepted = accepts(&WheelInput {
                delta,
                modifiers: *modifiers,
                phase: *phase,
                source: *source,
            });
            consumed |= accepted;
            !accepted
        });
        if consumed {
            input.smooth_scroll_delta = egui::Vec2::ZERO;
        }
    });
}

/// Consume all public wheel representations together for this viewport/pass.
///
/// Call after the accepting owner has read/applied the input. This does not
/// terminate egui's internal smoothing tail at a popup lifecycle boundary.
pub(crate) fn consume_wheel_input(ctx: &egui::Context) {
    ctx.input_mut(|input| {
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
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        }
    }

    fn input(events: Vec<egui::Event>, modifiers: egui::Modifiers) -> egui::InputState {
        egui::InputState::default().begin_pass(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500.0, 300.0),
                )),
                events: std::iter::once(egui::Event::ModifiersChanged(modifiers))
                    .chain(events)
                    .collect(),
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
            assert_eq!(
                remaining_wheel_inputs(&state, options())[0].delta,
                scale * delta
            );
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
            let remaining = remaining_wheel_inputs(&state, options())[0];
            assert_eq!(remaining.delta, expected);
            assert_eq!(remaining.modifiers, event_modifiers);
        }
    }

    fn options() -> egui::InputOptions {
        egui::InputOptions {
            line_scroll_speed: 13.0,
            ..Default::default()
        }
    }

    #[test]
    fn egui037_wheel_preserves_contiguous_aggregation_and_event_modifiers() {
        let state = input(
            vec![
                wheel(
                    egui::MouseWheelUnit::Point,
                    egui::vec2(0.0, 10.0),
                    egui::Modifiers::NONE,
                ),
                wheel(
                    egui::MouseWheelUnit::Point,
                    egui::vec2(0.0, 2.0),
                    egui::Modifiers::NONE,
                ),
                wheel(
                    egui::MouseWheelUnit::Point,
                    egui::vec2(0.0, -4.0),
                    egui::Modifiers::CTRL,
                ),
                wheel(
                    egui::MouseWheelUnit::Point,
                    egui::vec2(0.0, 1.0),
                    egui::Modifiers::NONE,
                ),
            ],
            egui::Modifiers::ALT,
        );
        let segments = remaining_wheel_inputs(&state, options());
        assert_eq!(segments.len(), 3);
        assert_eq!(
            segments.iter().map(|s| s.delta.y).collect::<Vec<_>>(),
            vec![12.0, -4.0, 1.0]
        );
        assert_eq!(
            segments.iter().map(|s| s.modifiers).collect::<Vec<_>>(),
            vec![
                egui::Modifiers::NONE,
                egui::Modifiers::CTRL,
                egui::Modifiers::NONE
            ]
        );
    }

    #[test]
    fn egui037_wheel_boundaries_and_sources_do_not_become_motion() {
        let events = [
            egui::TouchPhase::Start,
            egui::TouchPhase::Move,
            egui::TouchPhase::End,
            egui::TouchPhase::Cancel,
        ]
        .into_iter()
        .map(|phase| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 2.0),
            modifiers: egui::Modifiers::NONE,
            phase,
            source: egui::MouseWheelSource::Unknown,
        })
        .collect();
        let segments = remaining_wheel_inputs(&input(events, egui::Modifiers::NONE), options());
        assert_eq!(segments.len(), 4);
        assert_eq!(
            segments.iter().map(|s| s.delta.y).collect::<Vec<_>>(),
            vec![0.0, 26.0, 0.0, 0.0]
        );
    }

    #[test]
    fn egui037_wheel_consumption_cannot_resurrect_from_raw_events() {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput {
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
        assert_eq!(
            ctx.input(|i| remaining_wheel_inputs(i, options()))[0]
                .delta
                .y,
            3.0
        );
        assert!(ctx.input(|input| input.smooth_scroll_delta.y > 0.0));

        consume_wheel_input(&ctx);
        consume_wheel_input(&ctx);

        ctx.input(|input| {
            assert!(remaining_wheel_inputs(input, options()).is_empty());
            assert_eq!(input.smooth_scroll_delta, egui::Vec2::ZERO);
            assert!(matches!(input.events.as_slice(), [egui::Event::Copy]));
            assert!(
                input
                    .raw
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::MouseWheel { .. }))
            );
            assert_eq!(input.modifiers, egui::Modifiers::NONE);
        });
        crate::egui_output::discard_headless_output(ctx.end_pass());
    }
}
