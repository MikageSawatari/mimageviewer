//! Ordered egui touch/pointer correlation for Phase 1 touch support.
//!
//! This adapter observes (but never consumes or mutates) the raw egui event
//! queue. It treats `Event::Touch` as the gesture source of truth and only
//! labels primary pointer events as touch-derived when the exact
//! egui-winit 0.33.3 synthetic-pointer signature is present.

use crate::touch_input::{
    TapZoneGeometry, TouchCommand, TouchOwner, TouchPhase, TouchRecognizer, TouchSample,
};
use egui::{Event, PointerButton, Pos2, Response, TouchDeviceId, TouchId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TouchSurface {
    MainGrid,
    StillFullscreen,
}

/// Read-only classification produced for one egui input pass.
///
/// Step 2 deliberately does not apply either the commands or the suppression
/// decisions. Step 3 can consume `commands` and consult suppression only for a
/// `Response` or primary event that this value has positively correlated.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TouchFrame {
    commands: Vec<TouchCommand>,
    primary_events: Vec<CorrelatedPrimary>,
    owner: TouchOwner,
    active: bool,
    touch_cancelled: bool,
}

impl TouchFrame {
    pub(crate) fn commands(&self) -> &[TouchCommand] {
        &self.commands
    }

    pub(crate) fn into_commands(self) -> Vec<TouchCommand> {
        self.commands
    }

    pub(crate) fn owner(&self) -> TouchOwner {
        self.owner
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn touch_cancelled(&self) -> bool {
        self.touch_cancelled
    }

    /// Returns true only after the primary event has first been correlated to
    /// an exact touch signature and the recognizer then requested suppression.
    pub(crate) fn should_suppress_primary(&self, pos: Pos2, pressed: bool) -> bool {
        self.primary_events
            .iter()
            .any(|event| event.pos == pos && event.pressed == pressed && event.should_suppress)
    }

    /// Convenience query for an egui response's primary release.
    ///
    /// The position equality is intentional: the egui-winit signature copies
    /// the same logical-point position into the touch and synthetic pointer
    /// events. Ambiguous or merely nearby pointer input remains on the existing
    /// mouse path.
    pub(crate) fn should_suppress_response(&self, response: &Response) -> bool {
        response
            .interact_pointer_pos()
            .is_some_and(|pos| self.should_suppress_primary(pos, false))
    }
}

impl TouchCorrelationState {
    fn advance_pending(&mut self, event: &Event, frame: &mut FrameBuilder) -> bool {
        let pending = self.pending.take().unwrap();
        self.advance_pending_tail(pending, event, frame)
    }

    fn advance_pending_tail(
        &mut self,
        pending: PendingSignature,
        event: &Event,
        frame: &mut FrameBuilder,
    ) -> bool {
        match pending {
            PendingSignature::StartMoved(touch) => {
                if matches!(event, Event::PointerMoved(pos) if *pos == touch.pos) {
                    self.pending = Some(PendingSignature::StartButton(touch));
                    true
                } else {
                    false
                }
            }
            PendingSignature::StartButton(touch) => {
                if !primary_button_matches(event, touch.pos, true) {
                    return false;
                }
                frame.primary_events.push(CorrelatedPrimary {
                    pos: touch.pos,
                    pressed: true,
                    should_suppress: touch.should_suppress,
                });
                frame.commands.extend(touch.commands);
                true
            }
            PendingSignature::MoveMoved(touch) => {
                if matches!(event, Event::PointerMoved(pos) if *pos == touch.pos) {
                    frame.commands.extend(touch.commands);
                    true
                } else {
                    false
                }
            }
            PendingSignature::EndButton(touch) => {
                if !primary_button_matches(event, touch.pos, false) {
                    return false;
                }
                frame.primary_events.push(CorrelatedPrimary {
                    pos: touch.pos,
                    pressed: false,
                    should_suppress: touch.should_suppress,
                });
                frame.commands.extend(touch.commands);
                self.pending = Some(PendingSignature::EndGone);
                true
            }
            PendingSignature::EndGone => matches!(event, Event::PointerGone),
        }
    }

    fn cancel_stream(&mut self, geometry: &TapZoneGeometry, now_ms: u64) {
        self.pointer_touch = None;
        self.pending = None;
        let _ = self.recognizer.handle_sample(
            geometry,
            TouchSample {
                id: 0,
                pos: Pos2::ZERO,
                phase: TouchPhase::Cancel,
                now_ms,
            },
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CorrelatedPrimary {
    pos: Pos2,
    pressed: bool,
    should_suppress: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TouchKey {
    device_id: TouchDeviceId,
    id: TouchId,
}

#[derive(Clone, Debug)]
struct PendingTouch {
    pos: Pos2,
    commands: Vec<TouchCommand>,
    should_suppress: bool,
}

#[derive(Clone, Debug)]
enum PendingSignature {
    StartMoved(PendingTouch),
    StartButton(PendingTouch),
    MoveMoved(PendingTouch),
    EndButton(PendingTouch),
    /// The release already supplies the positive evidence needed to classify
    /// that primary event. Keep this tail state across passes so a leading
    /// `PointerGone` in the next pass is still consumed as part of the same
    /// egui-winit sequence.
    EndGone,
}

#[derive(Clone, Debug)]
struct TouchCorrelationState {
    recognizer: TouchRecognizer,
    /// Mirrors egui-winit's `pointer_touch_id`: only this contact receives the
    /// synthetic primary stream while it is active.
    pointer_touch: Option<TouchKey>,
    pending: Option<PendingSignature>,
    last_frame: TouchFrame,
}

impl Default for TouchCorrelationState {
    fn default() -> Self {
        Self {
            recognizer: TouchRecognizer::new(),
            pointer_touch: None,
            pending: None,
            last_frame: TouchFrame::default(),
        }
    }
}

#[derive(Default)]
struct FrameBuilder {
    commands: Vec<TouchCommand>,
    primary_events: Vec<CorrelatedPrimary>,
    ambiguous: bool,
    touch_cancelled: bool,
}

impl FrameBuilder {
    fn finish(mut self, owner: TouchOwner, active: bool) -> TouchFrame {
        if self.ambiguous {
            // Suppression is useful only together with a replacement command.
            // A mixed or malformed primary stream therefore falls back in
            // full: no touch command and no suppression recommendation.
            self.commands.clear();
            self.primary_events.clear();
        }
        TouchFrame {
            commands: self.commands,
            primary_events: self.primary_events,
            owner,
            active,
            touch_cancelled: self.touch_cancelled,
        }
    }
}

impl TouchCorrelationState {
    fn process_frame(
        &mut self,
        geometry: &TapZoneGeometry,
        events: &[Event],
        now_ms: u64,
        disabled: bool,
    ) -> TouchFrame {
        if disabled {
            let touch_cancelled = self.recognizer.is_active();
            *self = Self::default();
            return TouchFrame {
                touch_cancelled,
                ..TouchFrame::default()
            };
        }

        let mut frame = FrameBuilder::default();
        for event in events {
            self.process_event(geometry, event, now_ms, &mut frame);
        }
        frame.finish(self.recognizer.owner(), self.recognizer.is_active())
    }

    fn process_event(
        &mut self,
        geometry: &TapZoneGeometry,
        event: &Event,
        now_ms: u64,
        frame: &mut FrameBuilder,
    ) {
        if self.pending.is_some() {
            if self.advance_pending(event, frame) {
                return;
            }

            // Exact adjacency is part of the correlation contract. Reprocess
            // the mismatching event after abandoning the ambiguous touch
            // stream so a later, independent exact sequence can still start.
            frame.ambiguous = true;
            self.cancel_stream(geometry, now_ms);
        }

        match event {
            Event::Touch {
                device_id,
                id,
                phase,
                pos,
                ..
            } => {
                frame.touch_cancelled |= *phase == egui::TouchPhase::Cancel;
                self.handle_touch(
                    geometry,
                    TouchKey {
                        device_id: *device_id,
                        id: *id,
                    },
                    *phase,
                    *pos,
                    now_ms,
                    frame,
                )
            }
            Event::PointerButton {
                button: PointerButton::Primary,
                ..
            } => {
                // A primary button without a pending exact touch signature is
                // existing mouse input. If touch input is also present in this
                // pass, fail closed and cancel all touch-side action.
                frame.ambiguous = true;
                self.cancel_stream(geometry, now_ms);
            }
            _ => {}
        }
    }

    fn handle_touch(
        &mut self,
        geometry: &TapZoneGeometry,
        key: TouchKey,
        phase: egui::TouchPhase,
        pos: Pos2,
        now_ms: u64,
        frame: &mut FrameBuilder,
    ) {
        let phase = match phase {
            egui::TouchPhase::Start => TouchPhase::Start,
            egui::TouchPhase::Move => TouchPhase::Move,
            egui::TouchPhase::End => TouchPhase::End,
            egui::TouchPhase::Cancel => TouchPhase::Cancel,
        };
        if matches!(phase, TouchPhase::Move | TouchPhase::End) && !self.recognizer.is_active() {
            // Never combine a correlated stray tail with suppression retained
            // by an already completed or cancelled recognizer stream.
            self.pointer_touch = None;
            self.pending = None;
            return;
        }
        let sample = TouchSample {
            id: key.id.0,
            pos,
            phase,
            now_ms,
        };

        if phase == TouchPhase::Cancel {
            // Cancel is unverified on real hardware. Discard both correlation
            // and recognizer ownership defensively; the following PointerGone,
            // if any, is harmless and intentionally carries no ownership.
            self.pointer_touch = None;
            self.pending = None;
            let _ = self.recognizer.handle_sample(geometry, sample);
            return;
        }

        let receives_synthetic_pointer = match phase {
            TouchPhase::Start => self.pointer_touch.is_none(),
            TouchPhase::Move | TouchPhase::End => {
                self.pointer_touch.is_some_and(|active| active == key)
            }
            TouchPhase::Cancel => unreachable!(),
        };
        let commands = self.recognizer.handle_sample(geometry, sample);
        let pending = PendingTouch {
            pos,
            commands,
            // This is sampled only after the raw touch event has updated the
            // recognizer. It is never consulted for an uncorrelated primary.
            should_suppress: self.recognizer.should_suppress_primary(),
        };

        if !receives_synthetic_pointer {
            // Additional contacts have no synthetic pointer events while the
            // first egui-winit pointer touch remains active.
            frame.commands.extend(pending.commands);
            return;
        }

        match phase {
            TouchPhase::Start => {
                self.pointer_touch = Some(key);
                self.pending = Some(PendingSignature::StartMoved(pending));
            }
            TouchPhase::Move => {
                self.pending = Some(PendingSignature::MoveMoved(pending));
            }
            TouchPhase::End => {
                self.pointer_touch = None;
                self.pending = Some(PendingSignature::EndButton(pending));
            }
            TouchPhase::Cancel => unreachable!(),
        }
    }
}

fn primary_button_matches(event: &Event, pos: Pos2, pressed: bool) -> bool {
    matches!(
        event,
        Event::PointerButton {
            pos: event_pos,
            button: PointerButton::Primary,
            pressed: event_pressed,
            ..
        } if *event_pos == pos && *event_pressed == pressed
    )
}

fn touch_gestures_disabled() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| std::env::var_os("MIV_DISABLE_TOUCH_GESTURES").is_some())
}

fn state_id(viewport: egui::ViewportId, surface: TouchSurface) -> egui::Id {
    egui::Id::new(("miv_touch_correlation", viewport, surface))
}

fn driven_surface_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::new(("miv_touch_correlation_driven_surface", viewport))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DrivenSurface {
    frame: u64,
    surface: TouchSurface,
}

/// Drive correlation once for the active surface and app frame.
///
/// `MainGrid` is called only from the grid `CentralPanel`; `StillFullscreen`
/// only from the non-video/non-music fullscreen `CentralPanel`. Embedded still
/// fullscreen returns before grid rendering, and a separate fullscreen window
/// has a different `ViewportId`. The frame marker is a fail-closed backstop,
/// including when egui performs more than one pass for the same app frame.
///
/// This only clones events and writes `ctx.data_temp`: it never consumes input,
/// mutates pointer state, requests repaint, executes commands, or suppresses a
/// response.
pub(crate) fn drive_egui_touch_input(
    ctx: &egui::Context,
    surface: TouchSurface,
    geometry: TapZoneGeometry,
    frame: u64,
    enabled: bool,
) -> TouchFrame {
    drive_egui_touch_input_inner(
        ctx,
        surface,
        geometry,
        frame,
        touch_gestures_disabled() || !enabled,
    )
}

fn drive_egui_touch_input_inner(
    ctx: &egui::Context,
    surface: TouchSurface,
    geometry: TapZoneGeometry,
    frame: u64,
    disabled: bool,
) -> TouchFrame {
    let viewport = ctx.viewport_id();
    let (events, now_ms) = ctx.input(|input| {
        let millis = if input.time.is_finite() && input.time > 0.0 {
            (input.time * 1000.0).min(u64::MAX as f64) as u64
        } else {
            0
        };
        (input.events.clone(), millis)
    });
    process_in_temp_data(
        ctx, viewport, frame, surface, geometry, events, now_ms, disabled,
    )
}

#[allow(clippy::too_many_arguments)]
fn process_in_temp_data(
    ctx: &egui::Context,
    viewport: egui::ViewportId,
    frame: u64,
    surface: TouchSurface,
    geometry: TapZoneGeometry,
    events: Vec<Event>,
    now_ms: u64,
    disabled: bool,
) -> TouchFrame {
    ctx.data_mut(|data| {
        let marker_id = driven_surface_id(viewport);
        if let Some(marker) = data.get_temp::<DrivenSurface>(marker_id)
            && marker.frame == frame
        {
            return repeated_drive_result(data, viewport, surface, marker);
        }
        data.insert_temp(marker_id, DrivenSurface { frame, surface });

        process_surface_state(
            data, viewport, surface, &geometry, &events, now_ms, disabled,
        )
    })
}

fn repeated_drive_result(
    data: &mut egui::util::IdTypeMap,
    viewport: egui::ViewportId,
    surface: TouchSurface,
    marker: DrivenSurface,
) -> TouchFrame {
    if marker.surface != surface {
        return TouchFrame::default();
    }
    data.get_temp::<TouchCorrelationState>(state_id(viewport, surface))
        .map_or_else(TouchFrame::default, |state| state.last_frame)
}

#[allow(clippy::too_many_arguments)]
fn process_surface_state(
    data: &mut egui::util::IdTypeMap,
    viewport: egui::ViewportId,
    surface: TouchSurface,
    geometry: &TapZoneGeometry,
    events: &[Event],
    now_ms: u64,
    disabled: bool,
) -> TouchFrame {
    let id = state_id(viewport, surface);
    let mut state = data
        .get_temp::<TouchCorrelationState>(id)
        .unwrap_or_default();
    let frame = state.process_frame(geometry, events, now_ms, disabled);
    state.last_frame = frame.clone();
    data.insert_temp(id, state);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::touch_input::TouchOwner;
    use egui::{Modifiers, Rect, pos2};

    fn geometry() -> TapZoneGeometry {
        TapZoneGeometry {
            surface: Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 800.0)),
            excluded: Vec::new(),
        }
    }

    fn touch(id: u64, phase: egui::TouchPhase, pos: Pos2) -> Event {
        Event::Touch {
            device_id: TouchDeviceId(7),
            id: TouchId(id),
            phase,
            pos,
            force: None,
        }
    }

    fn primary(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn mouse_click(pos: Pos2) -> Vec<Event> {
        vec![
            Event::PointerMoved(pos),
            primary(pos, true),
            primary(pos, false),
        ]
    }

    fn tap_events(id: u64, pos: Pos2) -> Vec<Event> {
        vec![
            touch(id, egui::TouchPhase::Start, pos),
            Event::PointerMoved(pos),
            primary(pos, true),
            touch(id, egui::TouchPhase::End, pos),
            primary(pos, false),
            Event::PointerGone,
        ]
    }

    fn process(state: &mut TouchCorrelationState, events: &[Event], now_ms: u64) -> TouchFrame {
        state.process_frame(&geometry(), events, now_ms, false)
    }

    #[test]
    fn mouse_only_never_classifies_or_suppresses_primary_input() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let frame = process(&mut state, &mouse_click(pos), 100);

        assert!(frame.commands().is_empty());
        assert!(frame.primary_events.is_empty());
        assert!(!frame.should_suppress_primary(pos, true));
        assert!(!frame.should_suppress_primary(pos, false));
    }

    #[test]
    fn existing_mouse_button_and_ctrl_wheel_paths_never_emit_touch_actions() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let mut events = Vec::new();
        for button in [
            PointerButton::Secondary,
            PointerButton::Middle,
            PointerButton::Extra1,
            PointerButton::Extra2,
        ] {
            events.push(Event::PointerButton {
                pos,
                button,
                pressed: true,
                modifiers: Modifiers::NONE,
            });
            events.push(Event::PointerButton {
                pos,
                button,
                pressed: false,
                modifiers: Modifiers::NONE,
            });
        }
        events.push(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 1.0),
            modifiers: Modifiers::CTRL,
        });

        let frame = process(&mut state, &events, 100);
        assert!(frame.commands().is_empty());
        assert!(frame.primary_events.is_empty());
        assert!(!frame.should_suppress_primary(pos, true));
        assert!(!frame.should_suppress_primary(pos, false));
    }

    #[test]
    fn mouse_click_after_completed_touch_is_not_suppressed() {
        let mut state = TouchCorrelationState::default();
        let touch_pos = pos2(500.0, 400.0);
        let touch_frame = process(&mut state, &tap_events(1, touch_pos), 100);
        assert_eq!(touch_frame.commands(), &[TouchCommand::ToggleChrome]);
        assert!(touch_frame.should_suppress_primary(touch_pos, false));
        assert!(state.recognizer.should_suppress_primary());

        // The recognizer retains true after End, but this adapter never asks
        // it about a primary event until correlation succeeds first.
        let mouse_pos = pos2(100.0, 100.0);
        let mouse_frame = process(&mut state, &mouse_click(mouse_pos), 200);
        assert!(mouse_frame.commands().is_empty());
        assert!(mouse_frame.primary_events.is_empty());
        assert!(!mouse_frame.should_suppress_primary(mouse_pos, false));
    }

    #[test]
    fn stray_touch_tail_cannot_reuse_completed_suppression() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let _ = process(&mut state, &tap_events(1, pos), 100);
        assert!(state.recognizer.should_suppress_primary());

        let stray = [
            touch(2, egui::TouchPhase::End, pos),
            primary(pos, false),
            Event::PointerGone,
        ];
        let frame = process(&mut state, &stray, 200);
        assert!(frame.commands().is_empty());
        assert!(frame.primary_events.is_empty());
        assert!(!frame.should_suppress_primary(pos, false));
    }

    /// Dependency guardian for `egui-winit-0.33.3/src/lib.rs:676-738`.
    /// If this premise changes, touch commands can disappear; loosening the
    /// check could instead misclassify and suppress existing mouse input.
    #[test]
    fn egui_winit_0_33_3_signature_is_exact_and_ordered() {
        let mut state = TouchCorrelationState::default();
        let start = pos2(100.0, 400.0);
        let moved = pos2(105.0, 400.0);
        // Start: Touch -> moved -> pressed; Move: Touch -> moved;
        // End: Touch -> released -> gone.
        let accepted = vec![
            touch(1, egui::TouchPhase::Start, start),
            Event::PointerMoved(start),
            primary(start, true),
            touch(1, egui::TouchPhase::Move, moved),
            Event::PointerMoved(moved),
            touch(1, egui::TouchPhase::End, moved),
            primary(moved, false),
            Event::PointerGone,
        ];
        let frame = process(&mut state, &accepted, 100);
        assert_eq!(frame.commands(), &[TouchCommand::PageSide { left: true }]);
        assert!(frame.should_suppress_primary(moved, false));
    }

    #[test]
    fn changed_signature_order_fails_closed() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(100.0, 400.0);
        let events = vec![
            touch(2, egui::TouchPhase::Start, pos),
            primary(pos, true),
            Event::PointerMoved(pos),
            touch(2, egui::TouchPhase::End, pos),
            primary(pos, false),
            Event::PointerGone,
        ];
        let frame = process(&mut state, &events, 100);
        assert!(frame.commands().is_empty());
        assert!(frame.primary_events.is_empty());
    }

    #[test]
    fn position_mismatch_fails_closed() {
        let mut state = TouchCorrelationState::default();
        let touch_pos = pos2(500.0, 400.0);
        let pointer_pos = pos2(500.5, 400.0);
        let events = vec![
            touch(1, egui::TouchPhase::Start, touch_pos),
            Event::PointerMoved(pointer_pos),
            primary(pointer_pos, true),
            touch(1, egui::TouchPhase::End, touch_pos),
            primary(touch_pos, false),
            Event::PointerGone,
        ];
        let frame = process(&mut state, &events, 100);
        assert!(frame.commands().is_empty());
        assert!(!frame.should_suppress_primary(touch_pos, false));
        assert!(!frame.should_suppress_primary(pointer_pos, false));
    }

    #[test]
    fn ten_complete_taps_in_one_frame_are_all_processed() {
        let mut state = TouchCorrelationState::default();
        let mut events = Vec::new();
        for id in 48..=57 {
            events.extend(tap_events(id, pos2(100.0, 400.0)));
        }
        let frame = process(&mut state, &events, 100);
        assert_eq!(frame.commands().len(), 10);
        assert!(
            frame
                .commands()
                .iter()
                .all(|command| { *command == TouchCommand::PageSide { left: true } })
        );
        let suppressed_releases = frame
            .primary_events
            .iter()
            .filter(|event| !event.pressed && event.should_suppress)
            .count();
        assert_eq!(suppressed_releases, 10);
    }

    #[test]
    fn state_survives_a_frame_boundary_and_leading_pointer_gone() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let first = vec![
            touch(1, egui::TouchPhase::Start, pos),
            Event::PointerMoved(pos),
            primary(pos, true),
            touch(1, egui::TouchPhase::End, pos),
            primary(pos, false),
        ];
        let first_frame = process(&mut state, &first, 100);
        assert_eq!(first_frame.commands(), &[TouchCommand::ToggleChrome]);
        assert!(matches!(state.pending, Some(PendingSignature::EndGone)));

        let mut second = vec![Event::PointerGone];
        second.extend(tap_events(2, pos));
        let second_frame = process(&mut state, &second, 200);
        assert_eq!(second_frame.commands(), &[TouchCommand::ToggleChrome]);
        assert!(state.pending.is_none());
    }

    #[test]
    fn touch_end_and_release_can_complete_in_a_later_frame() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let start = vec![
            touch(1, egui::TouchPhase::Start, pos),
            Event::PointerMoved(pos),
            primary(pos, true),
        ];
        assert!(process(&mut state, &start, 100).commands().is_empty());
        let end = [touch(1, egui::TouchPhase::End, pos)];
        assert!(process(&mut state, &end, 200).commands().is_empty());

        let tail = [primary(pos, false), Event::PointerGone];
        let completed = process(&mut state, &tail, 300);
        assert_eq!(completed.commands(), &[TouchCommand::ToggleChrome]);
        assert!(completed.should_suppress_primary(pos, false));
    }

    #[test]
    fn mixed_mouse_primary_cancels_touch_action_and_suppression() {
        let mut state = TouchCorrelationState::default();
        let touch_pos = pos2(500.0, 400.0);
        let mouse_pos = pos2(50.0, 50.0);
        let mut events = tap_events(1, touch_pos);
        events.extend(mouse_click(mouse_pos));

        let frame = process(&mut state, &events, 100);
        assert!(frame.commands().is_empty());
        assert!(!frame.should_suppress_primary(touch_pos, false));
        assert!(!frame.should_suppress_primary(mouse_pos, false));
        assert!(!state.recognizer.is_active());
        assert_eq!(state.recognizer.owner(), TouchOwner::Cancelled);
    }

    #[test]
    fn disabled_gate_returns_nothing_and_discards_existing_state() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let start = vec![
            touch(1, egui::TouchPhase::Start, pos),
            Event::PointerMoved(pos),
            primary(pos, true),
        ];
        let _ = process(&mut state, &start, 100);
        assert!(state.recognizer.is_active());

        // true is the production gate value when the environment variable is set.
        let frame = state.process_frame(&geometry(), &tap_events(2, pos), 200, true);
        assert!(frame.commands().is_empty());
        assert!(frame.primary_events.is_empty());
        assert!(frame.touch_cancelled());
        assert!(!state.recognizer.is_active());
        assert!(state.pointer_touch.is_none());
        assert!(state.pending.is_none());
    }

    #[test]
    fn cancel_discards_correlation_and_recognizer_ownership() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let start = vec![
            touch(1, egui::TouchPhase::Start, pos),
            Event::PointerMoved(pos),
            primary(pos, true),
        ];
        let _ = process(&mut state, &start, 100);
        assert!(state.recognizer.is_active());
        assert!(state.pointer_touch.is_some());

        let cancel = [touch(1, egui::TouchPhase::Cancel, pos), Event::PointerGone];
        let frame = process(&mut state, &cancel, 200);
        assert!(frame.commands().is_empty());
        assert!(frame.primary_events.is_empty());
        assert!(!state.recognizer.is_active());
        assert_eq!(state.recognizer.owner(), TouchOwner::Cancelled);
        assert!(frame.touch_cancelled());
        assert!(state.pointer_touch.is_none());
        assert!(state.pending.is_none());
    }

    #[test]
    fn context_temp_state_is_surface_keyed_and_one_surface_drives_per_frame() {
        let ctx = egui::Context::default();
        let geometry = geometry();
        let raw = egui::RawInput {
            screen_rect: Some(geometry.surface),
            time: Some(0.1),
            events: tap_events(1, pos2(500.0, 400.0)),
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let frame = drive_egui_touch_input_inner(
                    ctx,
                    TouchSurface::MainGrid,
                    geometry.clone(),
                    9,
                    false,
                );
                let mut response = ui.allocate_rect(ui.max_rect(), egui::Sense::click());
                response.interact_pointer_pos = Some(pos2(500.0, 400.0));
                assert!(frame.should_suppress_response(&response));
                assert_eq!(frame.clone().into_commands().len(), 1);

                let repeated = drive_egui_touch_input_inner(
                    ctx,
                    TouchSurface::MainGrid,
                    geometry.clone(),
                    9,
                    false,
                );
                assert_eq!(repeated.commands().len(), 1);
                let conflicting = drive_egui_touch_input_inner(
                    ctx,
                    TouchSurface::StillFullscreen,
                    geometry.clone(),
                    9,
                    false,
                );
                assert!(conflicting.commands().is_empty());
            });
        });
    }

    #[test]
    fn secondary_contacts_need_no_synthetic_pointer_signature() {
        let mut state = TouchCorrelationState::default();
        let first = pos2(200.0, 400.0);
        let second = pos2(800.0, 400.0);
        let moved = pos2(900.0, 400.0);
        let events = vec![
            touch(1, egui::TouchPhase::Start, first),
            Event::PointerMoved(first),
            primary(first, true),
            touch(2, egui::TouchPhase::Start, second),
            touch(2, egui::TouchPhase::Move, moved),
            touch(2, egui::TouchPhase::End, moved),
            touch(1, egui::TouchPhase::End, first),
            primary(first, false),
            Event::PointerGone,
        ];
        let frame = process(&mut state, &events, 100);
        assert_eq!(frame.commands().len(), 3);
        assert_eq!(frame.commands().last(), Some(&TouchCommand::PinchEnd));
        assert!(frame.should_suppress_primary(first, false));
    }

    #[test]
    fn secondary_contact_stays_direct_after_synthetic_primary_ends() {
        let mut state = TouchCorrelationState::default();
        let first = pos2(200.0, 400.0);
        let second = pos2(800.0, 400.0);
        let events = vec![
            touch(1, egui::TouchPhase::Start, first),
            Event::PointerMoved(first),
            primary(first, true),
            touch(2, egui::TouchPhase::Start, second),
            touch(1, egui::TouchPhase::End, first),
            primary(first, false),
            Event::PointerGone,
            touch(2, egui::TouchPhase::End, second),
        ];

        let frame = process(&mut state, &events, 100);
        assert_eq!(frame.commands().last(), Some(&TouchCommand::PinchEnd));
        assert!(frame.should_suppress_primary(first, false));
        assert!(state.pending.is_none());
        assert!(state.pointer_touch.is_none());
    }
}
