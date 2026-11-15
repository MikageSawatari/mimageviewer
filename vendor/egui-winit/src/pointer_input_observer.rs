//! Observation at the boundary where a native event becomes egui pointer input.
//!
//! This does not replace input delivery or interpret native touch history. A
//! shared observer can associate a gesture with its owner before a later egui
//! pass drains the already queued input.

use std::sync::Arc;

use egui::{Context, Event, PointerButton, TouchPhase, ViewportId};
use winit::event::{ElementState, MouseButton, WindowEvent};

/// Physical origin, even when egui also promotes touch to a primary pointer or
/// simulates touch from a mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerInputSource {
    Mouse,
    Touch(u64),
}

/// A phase proved by the input appended for one native event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NormalizedPointerInput {
    Pressed(PointerInputSource),
    Released(PointerInputSource),
    /// A native cancellation or an unmatched mouse release (no forwarded hold).
    /// A forwarded physical hold can end without a current pointer position.
    Cancelled(PointerInputSource),
}

/// Called synchronously after conversion, once at most per native event, and
/// outside the Context data lock. Observers should enqueue into their shared
/// input owner only; they must not reenter the application or eframe.
///
/// Return an opaque provenance token when accepting a Pressed input. It is
/// inserted immediately before that input's first raw start event, so queued
/// input retains its owner association across take/append and deferred passes.
/// Other phases ignore the return value. Returning None leaves input unchanged.
pub type PointerInputObserver =
    Arc<dyn Fn(&Context, ViewportId, NormalizedPointerInput) -> Option<egui::Id> + Send + Sync>;

/// A physical primary press which could not reach the normalized observer.
/// Diagnostics only: no input is synthesized or retried for these failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerInputDiscardReason {
    ObserverNotInstalled,
    PointerPositionUnavailable,
}

pub(crate) fn log_pressed_discard(
    ctx: &Context,
    viewport: ViewportId,
    reason: PointerInputDiscardReason,
    input_source: PointerInputSource,
) {
    let source = match input_source {
        PointerInputSource::Mouse => "ClientClick",
        PointerInputSource::Touch(_) => "TouchTap",
    };
    log::info!(target: "egui_winit::pointer_input_observer::diagnostic",
        "[detached-activation] boundary=normalized_input source={source} decision={reason:?} input=Pressed({input_source:?}) viewport={viewport:?} registration={:?}",
        ctx.viewport_registration(viewport),
    );
}

fn observer_id() -> egui::Id {
    egui::Id::new("egui_winit::pointer_input_observer")
}

/// Install or remove the observer shared by all window States of this Context.
/// Existing queued input is never replayed to a newly installed observer.
pub fn set_observer(ctx: &Context, observer: Option<PointerInputObserver>) {
    ctx.data_mut(|data| {
        if let Some(observer) = observer {
            data.insert_temp(observer_id(), observer);
        } else {
            data.remove::<PointerInputObserver>(observer_id());
        }
    });
}

pub(crate) fn observe(
    ctx: &Context,
    viewport: ViewportId,
    event: &WindowEvent,
    events: &mut Vec<Event>,
    events_before: usize,
) {
    let Some(input) = normalized_input(event, &events[events_before..]) else {
        return;
    };
    let observer = ctx.data(|data| data.get_temp::<PointerInputObserver>(observer_id()));
    if let Some(observer) = observer {
        let provenance = observer(ctx, viewport, input);
        if let NormalizedPointerInput::Pressed(_) = input
            && let Some(provenance) = provenance
            && let Some(start) = events[events_before..].iter().position(|event| {
                matches!(
                    event,
                    Event::Touch {
                        phase: TouchPhase::Start,
                        ..
                    } | Event::PointerButton {
                        button: PointerButton::Primary,
                        pressed: true,
                        ..
                    }
                )
            })
        {
            events.insert(
                events_before + start,
                Event::PointerInputProvenance(provenance),
            );
        }
    } else if let NormalizedPointerInput::Pressed(source) = input {
        log_pressed_discard(
            ctx,
            viewport,
            PointerInputDiscardReason::ObserverNotInstalled,
            source,
        );
    }
}

fn normalized_input(event: &WindowEvent, appended: &[Event]) -> Option<NormalizedPointerInput> {
    match event {
        WindowEvent::MouseInput {
            state,
            button: MouseButton::Left,
            ..
        } => {
            let pressed = *state == ElementState::Pressed;
            let emitted = appended.iter().any(|event| {
                matches!(event, Event::PointerButton {
                    button: PointerButton::Primary,
                    pressed: emitted_pressed,
                    ..
                } if *emitted_pressed == pressed)
                    || (!pressed
                        && matches!(
                            event,
                            Event::PointerButtonReleased {
                                button: PointerButton::Primary,
                                ..
                            }
                        ))
            });
            match (pressed, emitted) {
                (true, true) => Some(NormalizedPointerInput::Pressed(PointerInputSource::Mouse)),
                (true, false) => None,
                (false, true) => Some(NormalizedPointerInput::Released(PointerInputSource::Mouse)),
                (false, false) => {
                    Some(NormalizedPointerInput::Cancelled(PointerInputSource::Mouse))
                }
            }
        }
        WindowEvent::Touch(touch) => {
            let phase = crate::to_egui_touch_phase(touch.phase);
            // Use the raw touch, not the optional primary-pointer promotion.
            // A second finger still has an independent physical gesture.
            let emitted = appended.iter().any(|event| {
                matches!(event, Event::Touch { id, phase: emitted_phase, .. }
                    if id.0 == touch.id && *emitted_phase == phase)
            });
            if !emitted {
                return None;
            }
            let source = PointerInputSource::Touch(touch.id);
            match phase {
                TouchPhase::Start => Some(NormalizedPointerInput::Pressed(source)),
                TouchPhase::End => Some(NormalizedPointerInput::Released(source)),
                TouchPhase::Cancel => Some(NormalizedPointerInput::Cancelled(source)),
                TouchPhase::Move => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HasDisplayHandle, State};
    use std::sync::Mutex;
    use winit::{dpi::PhysicalPosition, event::DeviceId};

    struct NoDisplay;

    impl HasDisplayHandle for NoDisplay {
        fn display_handle(
            &self,
        ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
            Err(raw_window_handle::HandleError::Unavailable)
        }
    }

    type Observed = Arc<Mutex<Vec<(ViewportId, NormalizedPointerInput)>>>;

    fn fixture() -> (State, Observed) {
        let ctx = Context::default();
        let observed: Observed = Default::default();
        let sink = observed.clone();
        set_observer(
            &ctx,
            Some(Arc::new(move |_, viewport, input| {
                sink.lock().unwrap().push((viewport, input));
                None
            })),
        );
        (
            State::new(ctx, ViewportId::ROOT, &NoDisplay, None, None, None),
            observed,
        )
    }

    fn mouse(pressed: bool, button: MouseButton) -> WindowEvent {
        WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            state: if pressed {
                ElementState::Pressed
            } else {
                ElementState::Released
            },
            button,
        }
    }

    fn touch(id: u64, phase: winit::event::TouchPhase) -> WindowEvent {
        WindowEvent::Touch(winit::event::Touch {
            device_id: DeviceId::dummy(),
            phase,
            location: PhysicalPosition::new(20.0, 40.0),
            force: None,
            id,
        })
    }

    // Window/EventLoop creation is deliberately excluded. Exercise the same
    // real State producers and observer boundary, supplying the window's scale
    // directly. on_window_event supplies this scale from the actual Window.
    fn dispatch(state: &mut State, event: &WindowEvent) -> Vec<Event> {
        let before = state.egui_input.events.len();
        state.on_pointer_event(event, 2.0);
        state.egui_input.events[before..].to_vec()
    }

    fn moved(x: f64, y: f64) -> WindowEvent {
        WindowEvent::CursorMoved {
            device_id: DeviceId::dummy(),
            position: PhysicalPosition::new(x, y),
        }
    }

    fn left() -> WindowEvent {
        WindowEvent::CursorLeft {
            device_id: DeviceId::dummy(),
        }
    }

    fn entered() -> WindowEvent {
        WindowEvent::CursorEntered {
            device_id: DeviceId::dummy(),
        }
    }

    fn primary_position(events: &[Event]) -> Option<egui::Pos2> {
        events.iter().find_map(|event| match event {
            Event::PointerButton {
                pos,
                pressed: true,
                button: PointerButton::Primary,
                ..
            } => Some(*pos),
            _ => None,
        })
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn realmachine55_cursor_reentry_at_same_position_restores_press_and_observer() {
        let (mut state, observed) = fixture();
        dispatch(&mut state, &moved(40.0, 60.0));
        assert_eq!(dispatch(&mut state, &left()), vec![Event::PointerGone]);
        assert_eq!(state.pointer_pos_in_points, None);
        assert_eq!(
            dispatch(&mut state, &entered()),
            vec![Event::PointerMoved(egui::pos2(20.0, 30.0))]
        );
        assert_eq!(
            primary_position(&dispatch(&mut state, &mouse(true, MouseButton::Left))),
            Some(egui::pos2(20.0, 30.0))
        );
        assert_eq!(
            phases(&observed),
            vec![NormalizedPointerInput::Pressed(PointerInputSource::Mouse)]
        );
    }

    #[test]
    fn realmachine55_cursor_reentry_new_moved_overwrites_restored_position() {
        let (mut state, _) = fixture();
        dispatch(&mut state, &moved(40.0, 60.0));
        dispatch(&mut state, &left());
        dispatch(&mut state, &entered());
        dispatch(&mut state, &moved(80.0, 100.0));
        assert_eq!(
            primary_position(&dispatch(&mut state, &mouse(true, MouseButton::Left))),
            Some(egui::pos2(40.0, 50.0))
        );
    }

    #[test]
    fn realmachine55_captured_outside_release_is_terminal_without_position() {
        let (mut state, observed) = fixture();
        dispatch(&mut state, &moved(40.0, 60.0));
        dispatch(&mut state, &mouse(true, MouseButton::Left));
        dispatch(&mut state, &moved(-40.0, 60.0));
        dispatch(&mut state, &left());
        dispatch(&mut state, &mouse(false, MouseButton::Left));
        assert_eq!(
            phases(&observed),
            vec![
                NormalizedPointerInput::Pressed(PointerInputSource::Mouse),
                NormalizedPointerInput::Released(PointerInputSource::Mouse),
            ]
        );
        let events = state.egui_input_mut().take().events;
        let ctx = state.egui_ctx.clone();
        ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |_| {},
        )
        .drop_without_applying_deltas();
        assert!(!ctx.input(|input| input.pointer.primary_down()));
        assert_eq!(ctx.input(|input| input.pointer.hover_pos()), None);
        assert!(!ctx.input(|input| input.pointer.any_click()));
    }

    #[test]
    fn realmachine55_positionless_mouse_terminal_is_owned_once_per_button() {
        for button in [
            MouseButton::Left,
            MouseButton::Right,
            MouseButton::Middle,
            MouseButton::Back,
            MouseButton::Forward,
        ] {
            let (mut state, _) = fixture();
            dispatch(&mut state, &moved(40.0, 60.0));
            dispatch(&mut state, &mouse(true, button));
            dispatch(&mut state, &left());
            let terminal = dispatch(&mut state, &mouse(false, button));
            assert!(
                matches!(terminal.as_slice(), [Event::PointerButtonReleased { button: released, .. }] if Some(*released) == crate::translate_mouse_button(button))
            );
            assert!(dispatch(&mut state, &mouse(false, button)).is_empty());
        }
    }

    #[test]
    fn realmachine55_touch_promotion_does_not_own_a_physical_mouse_terminal() {
        for phase in [
            winit::event::TouchPhase::Ended,
            winit::event::TouchPhase::Cancelled,
        ] {
            let (mut state, observed) = fixture();
            dispatch(&mut state, &touch(1, winit::event::TouchPhase::Started));
            dispatch(&mut state, &touch(1, phase));
            assert!(dispatch(&mut state, &mouse(false, MouseButton::Left)).is_empty());
            assert_eq!(
                phases(&observed).last(),
                Some(&NormalizedPointerInput::Cancelled(
                    PointerInputSource::Mouse
                ))
            );
        }
    }

    #[test]
    fn realmachine55_simulated_touch_and_physical_mouse_both_end_without_cursor() {
        let (mut state, _) = fixture();
        state.simulate_touch_screen = true;
        dispatch(&mut state, &moved(40.0, 60.0));
        dispatch(&mut state, &mouse(true, MouseButton::Left));
        dispatch(&mut state, &moved(-40.0, 80.0));
        dispatch(&mut state, &left());
        let terminal = dispatch(&mut state, &mouse(false, MouseButton::Left));
        assert!(
            matches!(terminal.as_slice(), [Event::PointerButtonReleased { .. }, Event::Touch { phase: TouchPhase::Cancel, pos, .. }] if *pos == egui::pos2(-20.0,40.0))
        );
        assert!(!state.any_pointer_button_down);
    }

    #[test]
    fn realmachine55_cursor_reentry_without_history_does_not_invent_position() {
        let (mut state, observed) = fixture();
        assert!(dispatch(&mut state, &entered()).is_empty());
        assert!(dispatch(&mut state, &mouse(true, MouseButton::Left)).is_empty());
        assert!(phases(&observed).is_empty());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn realmachine55_cursor_reentry_uses_current_effective_pixels_per_point() {
        let (mut state, observed) = fixture();
        dispatch(&mut state, &moved(40.0, 60.0));
        dispatch(&mut state, &left());
        state.on_pointer_event(&entered(), 4.0);
        assert_eq!(
            primary_position(&dispatch(&mut state, &mouse(true, MouseButton::Left))),
            Some(egui::pos2(10.0, 15.0))
        );
        assert_eq!(phases(&observed).len(), 1);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn realmachine55_cursor_history_survives_touch_end_and_cancel() {
        for phase in [
            winit::event::TouchPhase::Ended,
            winit::event::TouchPhase::Cancelled,
        ] {
            let (mut state, _) = fixture();
            dispatch(&mut state, &moved(40.0, 60.0));
            dispatch(&mut state, &touch(1, winit::event::TouchPhase::Started));
            dispatch(&mut state, &touch(1, phase));
            assert_eq!(state.pointer_pos_in_points, None);
            dispatch(&mut state, &entered());
            assert_eq!(
                primary_position(&dispatch(&mut state, &mouse(true, MouseButton::Left))),
                Some(egui::pos2(20.0, 30.0))
            );
        }
    }

    #[test]
    fn realmachine55_recreated_state_does_not_inherit_cursor_history() {
        let (mut old, _) = fixture();
        dispatch(&mut old, &moved(40.0, 60.0));
        let mut new = State::new(
            old.egui_ctx.clone(),
            old.viewport_id,
            &NoDisplay,
            None,
            None,
            None,
        );
        assert!(dispatch(&mut new, &entered()).is_empty());
        assert!(dispatch(&mut new, &mouse(true, MouseButton::Left)).is_empty());
    }

    #[test]
    fn realmachine55_pointer_adapter_ignores_other_events_without_mutation() {
        let (mut state, observed) = fixture();
        assert!(
            state
                .on_pointer_event(&WindowEvent::Focused(true), 2.0)
                .is_none()
        );
        assert!(state.egui_input.events.is_empty());
        assert!(phases(&observed).is_empty());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn realmachine55_cursor_history_is_owned_by_each_root_and_child_state() {
        let (mut root, observed) = fixture();
        let child_id = ViewportId::from_hash_of("rm55-cursor-child");
        let mut child = State::new(
            root.egui_ctx.clone(),
            child_id,
            &NoDisplay,
            None,
            None,
            None,
        );
        dispatch(&mut root, &moved(40.0, 60.0));
        dispatch(&mut child, &moved(80.0, 100.0));
        for (state, expected) in [
            (&mut root, egui::pos2(20.0, 30.0)),
            (&mut child, egui::pos2(40.0, 50.0)),
        ] {
            dispatch(state, &left());
            dispatch(state, &entered());
            assert_eq!(
                primary_position(&dispatch(state, &mouse(true, MouseButton::Left))),
                Some(expected)
            );
        }
        assert_eq!(
            observed
                .lock()
                .unwrap()
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            vec![ViewportId::ROOT, child_id]
        );
    }

    fn phases(observed: &Observed) -> Vec<NormalizedPointerInput> {
        observed
            .lock()
            .unwrap()
            .iter()
            .map(|(_, phase)| *phase)
            .collect()
    }

    fn return_provenance(state: &State, provenance: egui::Id) {
        set_observer(
            &state.egui_ctx,
            Some(Arc::new(move |_, _, _| Some(provenance))),
        );
    }

    fn assert_provenance_before_start(events: &[Event], provenance: egui::Id) {
        let markers: Vec<_> = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| {
                matches!(event, Event::PointerInputProvenance(id) if *id == provenance)
                    .then_some(index)
            })
            .collect();
        assert_eq!(markers.len(), 1);
        assert!(matches!(
            events.get(markers[0] + 1),
            Some(
                Event::Touch {
                    phase: TouchPhase::Start,
                    ..
                } | Event::PointerButton {
                    button: PointerButton::Primary,
                    pressed: true,
                    ..
                }
            )
        ));
    }

    #[test]
    fn pointer_input_observer_provenance_some_tags_only_fresh_start_after_older_queued_input() {
        let (mut state, _) = fixture();
        state.pointer_pos_in_points = Some(egui::Pos2::ZERO);
        dispatch(&mut state, &mouse(true, MouseButton::Left));
        dispatch(&mut state, &mouse(false, MouseButton::Left));
        let old = state.egui_input.events.clone();
        let provenance = egui::Id::new("fresh-owner");
        return_provenance(&state, provenance);
        let fresh = dispatch(&mut state, &mouse(true, MouseButton::Left));
        assert_eq!(&state.egui_input.events[..old.len()], old);
        assert!(
            matches!(fresh.first(), Some(Event::PointerInputProvenance(id)) if *id == provenance)
        );
        assert_provenance_before_start(&state.egui_input.events, provenance);
        dispatch(&mut state, &mouse(false, MouseButton::Left));
        assert_provenance_before_start(&state.egui_input.events, provenance);
    }

    #[test]
    fn pointer_input_observer_provenance_tags_raw_touch_start_once_with_or_without_promotion() {
        for promoted in [false, true] {
            let (mut state, _) = fixture();
            let provenance = egui::Id::new(("touch-owner", promoted));
            return_provenance(&state, provenance);
            if !promoted {
                state.pointer_touch_id = Some(1);
            }
            let appended = dispatch(&mut state, &touch(2, winit::event::TouchPhase::Started));
            assert_provenance_before_start(&appended, provenance);
            assert!(matches!(
                appended.get(1),
                Some(Event::Touch {
                    phase: TouchPhase::Start,
                    ..
                })
            ));
            assert_eq!(
                appended
                    .iter()
                    .filter(|event| matches!(event, Event::PointerButton { pressed: true, .. }))
                    .count(),
                usize::from(promoted)
            );
        }
    }

    #[test]
    fn pointer_input_observer_provenance_some_without_raw_press_creates_no_marker() {
        let (mut state, _) = fixture();
        return_provenance(&state, egui::Id::new("no-raw-press"));
        assert!(dispatch(&mut state, &mouse(true, MouseButton::Left)).is_empty());
        assert!(state.egui_input.events.is_empty());
    }

    #[test]
    fn pointer_input_observer_provenance_return_value_for_release_or_cancel_is_ignored() {
        let (mut state, _) = fixture();
        return_provenance(&state, egui::Id::new("terminal-return"));
        for positioned in [true, false] {
            state.pointer_pos_in_points = positioned.then_some(egui::Pos2::ZERO);
            let events = dispatch(&mut state, &mouse(false, MouseButton::Left));
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Event::PointerInputProvenance(_)))
            );
        }
        for phase in [
            winit::event::TouchPhase::Ended,
            winit::event::TouchPhase::Cancelled,
        ] {
            let events = dispatch(&mut state, &touch(2, phase));
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Event::PointerInputProvenance(_)))
            );
        }
    }

    #[test]
    fn pointer_input_observer_provenance_survives_raw_take_append_and_prepended_key_release() {
        let (mut state, _) = fixture();
        let provenance = egui::Id::new("take-append-owner");
        return_provenance(&state, provenance);
        state.pointer_pos_in_points = Some(egui::Pos2::ZERO);
        dispatch(&mut state, &mouse(true, MouseButton::Left));
        dispatch(&mut state, &mouse(false, MouseButton::Left));
        let produced = state.egui_input.events.clone();
        let drained = state.egui_input.take();
        assert!(state.egui_input.events.is_empty());
        assert_eq!(drained.events, produced);
        let (mut old_state, _) = fixture();
        old_state.pointer_pos_in_points = Some(egui::Pos2::ZERO);
        dispatch(&mut old_state, &mouse(true, MouseButton::Left));
        dispatch(&mut old_state, &mouse(false, MouseButton::Left));
        let mut retained = old_state.egui_input.take();
        let old_len = retained.events.len();
        retained.append(drained);
        retained.events.splice(
            0..0,
            [Event::Key {
                key: egui::Key::F12,
                physical_key: Some(egui::Key::F12),
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(&retained.events[old_len + 1..], produced);
        assert_provenance_before_start(&retained.events, provenance);
        let replay = retained.clone();
        assert_eq!(replay.events, retained.events);
        assert_provenance_before_start(&replay.events, provenance);
    }

    #[test]
    fn pointer_input_observer_provenance_and_start_are_not_replayed_on_egui_discard_repass() {
        let (mut state, _) = fixture();
        let provenance = egui::Id::new("discard-repass-owner");
        return_provenance(&state, provenance);
        state.pointer_pos_in_points = Some(egui::Pos2::ZERO);
        dispatch(&mut state, &mouse(true, MouseButton::Left));
        let mut passes = Vec::new();
        let output = state.egui_ctx.run_ui(state.egui_input.take(), |ui| {
            passes.push(ui.input(|input| input.events.clone()));
            if passes.len() == 1 {
                ui.ctx()
                    .request_discard("pointer provenance repass regression");
            }
        });
        output.drop_without_applying_deltas();
        assert_eq!(passes.len(), 2);
        assert_provenance_before_start(&passes[0], provenance);
        // Context::run_dyn takes volatile raw events on its first pass. A
        // discard pass reuses neither the start nor its provenance marker.
        assert!(!passes[1].iter().any(|event| matches!(
            event,
            Event::PointerInputProvenance(_)
                | Event::Touch {
                    phase: TouchPhase::Start,
                    ..
                }
                | Event::PointerButton {
                    button: PointerButton::Primary,
                    pressed: true,
                    ..
                }
        )));
    }

    #[test]
    fn pointer_input_observer_mouse_matches_real_primary_conversion() {
        let (mut state, observed) = fixture();
        state.pointer_pos_in_points = Some(egui::pos2(10.0, 20.0));
        for pressed in [true, false] {
            let appended = dispatch(&mut state, &mouse(pressed, MouseButton::Left));
            assert!(matches!(appended.as_slice(), [Event::PointerButton {
                button: PointerButton::Primary, pressed: actual, ..
            }] if *actual == pressed));
        }
        assert_eq!(
            phases(&observed),
            [
                NormalizedPointerInput::Pressed(PointerInputSource::Mouse),
                NormalizedPointerInput::Released(PointerInputSource::Mouse),
            ]
        );
    }

    #[test]
    fn pointer_input_observer_positionless_press_has_no_proof_but_owned_release_completes() {
        let (mut state, observed) = fixture();
        assert!(dispatch(&mut state, &mouse(true, MouseButton::Left)).is_empty());
        assert!(phases(&observed).is_empty());
        assert!(dispatch(&mut state, &mouse(false, MouseButton::Left)).is_empty());
        assert_eq!(
            phases(&observed),
            [NormalizedPointerInput::Cancelled(PointerInputSource::Mouse)]
        );
        state.pointer_pos_in_points = Some(egui::Pos2::ZERO);
        dispatch(&mut state, &mouse(true, MouseButton::Left));
        dispatch(&mut state, &left());
        assert!(matches!(
            dispatch(&mut state, &mouse(false, MouseButton::Left)).as_slice(),
            [Event::PointerButtonReleased {
                button: PointerButton::Primary,
                ..
            }]
        ));
        assert_eq!(
            phases(&observed),
            [
                NormalizedPointerInput::Cancelled(PointerInputSource::Mouse),
                NormalizedPointerInput::Pressed(PointerInputSource::Mouse),
                NormalizedPointerInput::Released(PointerInputSource::Mouse),
            ]
        );
    }

    #[test]
    fn pointer_input_observer_queued_old_events_cannot_prove_new_press() {
        let (mut state, observed) = fixture();
        state.pointer_pos_in_points = Some(egui::Pos2::ZERO);
        dispatch(&mut state, &mouse(true, MouseButton::Left));
        dispatch(&mut state, &mouse(false, MouseButton::Left));
        let old = state.egui_input.events.clone();
        observed.lock().unwrap().clear();
        state.pointer_pos_in_points = None;
        dispatch(&mut state, &mouse(true, MouseButton::Left));
        assert!(phases(&observed).is_empty());
        assert_eq!(state.egui_input.events, old);
    }

    #[test]
    fn pointer_input_observer_simulated_touch_remains_one_mouse_source() {
        let (mut state, observed) = fixture();
        state.simulate_touch_screen = true;
        state.pointer_pos_in_points = Some(egui::Pos2::ZERO);
        for pressed in [true, false] {
            let appended = dispatch(&mut state, &mouse(pressed, MouseButton::Left));
            assert!(
                appended
                    .iter()
                    .any(|event| matches!(event, Event::Touch { .. }))
            );
        }
        assert_eq!(
            phases(&observed),
            [
                NormalizedPointerInput::Pressed(PointerInputSource::Mouse),
                NormalizedPointerInput::Released(PointerInputSource::Mouse),
            ]
        );
    }

    #[test]
    fn pointer_input_observer_touch_promotion_does_not_duplicate_notification() {
        let (mut state, observed) = fixture();
        let start = dispatch(&mut state, &touch(7, winit::event::TouchPhase::Started));
        assert_eq!(
            start
                .iter()
                .filter(|event| matches!(event, Event::Touch { .. }))
                .count(),
            1
        );
        assert_eq!(
            start
                .iter()
                .filter(|event| matches!(event, Event::PointerButton { pressed: true, .. }))
                .count(),
            1
        );
        assert!(
            matches!(start.first(), Some(Event::Touch { pos, .. }) if *pos == egui::pos2(10.0, 20.0))
        );
        dispatch(&mut state, &touch(7, winit::event::TouchPhase::Moved));
        dispatch(&mut state, &touch(7, winit::event::TouchPhase::Ended));
        assert_eq!(
            phases(&observed),
            [
                NormalizedPointerInput::Pressed(PointerInputSource::Touch(7)),
                NormalizedPointerInput::Released(PointerInputSource::Touch(7)),
            ]
        );
    }

    #[test]
    fn pointer_input_observer_unpromoted_touch_has_its_own_phases() {
        let (mut state, observed) = fixture();
        state.pointer_touch_id = Some(1);
        for phase in [
            winit::event::TouchPhase::Started,
            winit::event::TouchPhase::Ended,
        ] {
            let appended = dispatch(&mut state, &touch(2, phase));
            assert!(matches!(appended.as_slice(), [Event::Touch { .. }]));
            assert_eq!(state.pointer_touch_id, Some(1));
        }
        assert_eq!(
            phases(&observed),
            [
                NormalizedPointerInput::Pressed(PointerInputSource::Touch(2)),
                NormalizedPointerInput::Released(PointerInputSource::Touch(2)),
            ]
        );
    }

    #[test]
    fn pointer_input_observer_touch_cancel_is_terminal_with_or_without_promotion() {
        for promoted in [false, true] {
            let (mut state, observed) = fixture();
            if !promoted {
                state.pointer_touch_id = Some(1);
            }
            dispatch(&mut state, &touch(2, winit::event::TouchPhase::Started));
            let appended = dispatch(&mut state, &touch(2, winit::event::TouchPhase::Cancelled));
            assert!(appended.iter().any(|event| matches!(
                event,
                Event::Touch {
                    phase: TouchPhase::Cancel,
                    ..
                }
            )));
            assert_eq!(
                phases(&observed),
                [
                    NormalizedPointerInput::Pressed(PointerInputSource::Touch(2)),
                    NormalizedPointerInput::Cancelled(PointerInputSource::Touch(2)),
                ]
            );
        }
    }

    #[test]
    fn pointer_input_observer_nonprimary_focus_and_motion_are_not_producers() {
        let (mut state, observed) = fixture();
        state.pointer_pos_in_points = Some(egui::Pos2::ZERO);
        for event in [
            mouse(true, MouseButton::Right),
            mouse(false, MouseButton::Right),
            WindowEvent::Focused(true),
            WindowEvent::Focused(false),
            WindowEvent::CursorMoved {
                device_id: DeviceId::dummy(),
                position: PhysicalPosition::new(10.0, 20.0),
            },
            WindowEvent::Ime(winit::event::Ime::Disabled),
        ] {
            dispatch(&mut state, &event);
        }
        assert!(phases(&observed).is_empty());
    }

    #[test]
    fn pointer_input_observer_shared_context_retains_viewport_and_other_context_is_untouched() {
        let (mut root, observed) = fixture();
        let child_id = ViewportId::from_hash_of("pointer-observer-child");
        let mut child = State::new(
            root.egui_ctx.clone(),
            child_id,
            &NoDisplay,
            None,
            None,
            None,
        );
        let mut other = State::new(Context::default(), child_id, &NoDisplay, None, None, None);
        for state in [&mut root, &mut child, &mut other] {
            state.pointer_pos_in_points = Some(egui::Pos2::ZERO);
            dispatch(state, &mouse(true, MouseButton::Left));
        }
        assert_eq!(
            *observed.lock().unwrap(),
            [
                (
                    ViewportId::ROOT,
                    NormalizedPointerInput::Pressed(PointerInputSource::Mouse)
                ),
                (
                    child_id,
                    NormalizedPointerInput::Pressed(PointerInputSource::Mouse)
                ),
            ]
        );
    }

    #[test]
    fn pointer_input_observer_repeated_main_retirement_keeps_child_conversion() {
        let (mut root, observed) = fixture();
        let ctx = root.egui_ctx.clone();
        ctx.set_embed_viewports(false);
        let child_id = ViewportId::from_hash_of("rm55-surviving-client");
        let main_id = ViewportId::from_hash_of("rm55-main-viewer");
        ctx.run_ui(Default::default(), |ui| {
            ui.ctx()
                .show_viewport_deferred(child_id, Default::default(), |_, _| {});
        })
        .drop_without_applying_deltas();
        let registration = ctx.viewport_registration(child_id).unwrap();
        let mut child = State::new(ctx.clone(), child_id, &NoDisplay, None, None, None);
        child.on_cursor_moved_in_points(egui::pos2(50.0, 60.0));
        for _ in 0..3 {
            ctx.run_ui(Default::default(), |ui| {
                ui.ctx()
                    .show_viewport_deferred(child_id, Default::default(), |_, _| {});
                ui.ctx()
                    .show_viewport_deferred(main_id, Default::default(), |_, _| {});
            })
            .drop_without_applying_deltas();
            let _ = ctx.retire_viewport(main_id);
            assert_eq!(ctx.viewport_registration(child_id), Some(registration));
            for pressed in [true, false] {
                let events = dispatch(&mut child, &mouse(pressed, MouseButton::Left));
                assert!(events.iter().any(|event| matches!(event, Event::PointerButton { pressed: actual, .. } if *actual == pressed)));
            }
            // Normal RawInput consumption does not discard State cursor ownership.
            child.egui_input.take();
        }
        assert_eq!(phases(&observed).len(), 6);
        assert!(
            observed
                .lock()
                .unwrap()
                .iter()
                .all(|(id, _)| *id == child_id)
        );
        root.pointer_pos_in_points = Some(egui::Pos2::ZERO);
        set_observer(&ctx, None);
        // Missing observer is diagnostic only: the physical input is preserved.
        assert_eq!(
            dispatch(&mut root, &mouse(true, MouseButton::Left)).len(),
            1
        );
        assert_eq!(phases(&observed).len(), 6);
    }

    #[test]
    fn pointer_input_observer_callback_can_access_context_and_unregister_itself() {
        let (mut state, observed) = fixture();
        let sink = observed.clone();
        set_observer(
            &state.egui_ctx,
            Some(Arc::new(move |ctx, viewport, phase| {
                ctx.data_mut(|data| data.insert_temp(egui::Id::new("observer-reentry"), 42_u32));
                set_observer(ctx, None);
                sink.lock().unwrap().push((viewport, phase));
                None
            })),
        );
        state.pointer_pos_in_points = Some(egui::Pos2::ZERO);
        dispatch(&mut state, &mouse(true, MouseButton::Left));
        dispatch(&mut state, &mouse(false, MouseButton::Left));
        assert_eq!(
            phases(&observed),
            [NormalizedPointerInput::Pressed(PointerInputSource::Mouse)]
        );
        assert_eq!(
            state
                .egui_ctx
                .data(|data| data.get_temp::<u32>(egui::Id::new("observer-reentry"))),
            Some(42)
        );
    }
}
