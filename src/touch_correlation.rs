//! Ordered egui touch/pointer correlation for Phase 1 touch support.
//!
//! This adapter observes (but never consumes or mutates) the raw egui event
//! queue. It treats `Event::Touch` as the gesture source of truth and only
//! labels primary pointer events as touch-derived when the exact
//! egui-winit 0.33.3 synthetic-pointer signature is present.

use std::fmt::Write as _;

use crate::touch_input::{
    TapZoneGeometry, TouchCommand, TouchOwner, TouchPhase, TouchRecognizer, TouchSample,
};
use egui::{Event, PointerButton, Pos2, Response, TouchId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TouchSurface {
    MainGrid,
    StillFullscreen,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CommandScope {
    #[default]
    Unscoped,
    MainGrid(egui::Id),
    Retired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrimaryOwnership {
    Current(CommandScope),
    Retired,
    Mouse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrimaryPhase {
    Button(bool),
    Moved,
}

/// Read-only classification produced for one app-frame drive.
///
/// Commands are returned only by the first drive for an app frame because
/// executing them mutates application state. Later egui passes receive the
/// same correlation and suppression answers with an empty command list.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TouchFrame {
    scope: CommandScope,
    commands: Vec<TouchCommand>,
    primary_events: Vec<CorrelatedPrimary>,
    owner: TouchOwner,
    active: bool,
    touch_cancelled: bool,
}

impl TouchFrame {
    fn replay_for_later_passes(&self) -> Self {
        Self {
            scope: self.scope,
            commands: Vec::new(),
            primary_events: self.primary_events.clone(),
            owner: self.owner,
            active: self.active,
            touch_cancelled: self.touch_cancelled,
        }
    }

    /// Keep read-only observation results while removing everything that may
    /// authorize a caller-side gesture action or primary-pointer suppression.
    fn suppress_execution(&mut self) {
        self.commands.clear();
        for event in &mut self.primary_events {
            event.should_suppress = false;
        }
    }

    pub(crate) fn commands(&self) -> &[TouchCommand] {
        &self.commands
    }

    /// A batch generated before an adoption cannot act on the new list owner.
    pub(crate) fn grid_commands(&self, scope: egui::Id) -> &[TouchCommand] {
        if self.scope == CommandScope::MainGrid(scope) {
            &self.commands
        } else {
            &[]
        }
    }

    /// The current grid recognizer reserves Y even while deciding a tap. Its
    /// exact terminal edge also remains App-owned, without suppressing widget
    /// clicks. Retired transport alone cannot take a new mouse gesture's Y.
    pub(crate) fn grid_owns_native_y(&self, scope: egui::Id) -> bool {
        if self.scope != CommandScope::MainGrid(scope) {
            return false;
        }
        match self.primary_events.last().map(|event| event.ownership) {
            Some(PrimaryOwnership::Mouse) => false,
            Some(PrimaryOwnership::Current(owner)) if owner == self.scope => true,
            _ => self.active || !self.commands.is_empty(),
        }
    }

    fn retire(&mut self) {
        self.commands.clear();
        self.scope = CommandScope::Retired;
        self.owner = TouchOwner::Undecided;
        self.active = false;
        self.touch_cancelled = false;
        for event in &mut self.primary_events {
            if event.ownership != PrimaryOwnership::Mouse {
                event.ownership = PrimaryOwnership::Retired;
                event.accepted_grid_tap = false;
            }
        }
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

    /// Whether this drive contains an exactly correlated touch-derived
    /// primary stream. Grid cells use this provenance-only answer to disable
    /// native file D&D without changing mouse-only input.
    pub(crate) fn has_touch_derived_pointer_activity(&self) -> bool {
        self.primary_events
            .last()
            .map_or(self.active || self.touch_cancelled, |event| {
                event.ownership != PrimaryOwnership::Mouse
            })
    }

    /// Positions of primary releases whose exact egui-winit synthetic-pointer
    /// signature was correlated to raw touch input in this pass.
    ///
    /// Callers must still apply their normal click/tap classification. This
    /// method proves provenance only; it never turns an arbitrary touch release
    /// into a click.
    pub(crate) fn correlated_primary_release_positions(&self) -> impl Iterator<Item = Pos2> + '_ {
        self.primary_events
            .iter()
            .filter(|event| {
                event.phase == PrimaryPhase::Button(false)
                    && event.ownership != PrimaryOwnership::Mouse
                    && self.is_touch_primary(event.pos, false)
            })
            .map(|event| event.pos)
    }

    pub(crate) fn is_touch_primary(&self, pos: Pos2, pressed: bool) -> bool {
        self.primary_event(pos, pressed)
            .is_some_and(|event| event.ownership != PrimaryOwnership::Mouse)
    }

    /// Positive evidence that the current edge is an ordinary pointer event,
    /// even if a previous touch edge occupied the same position in this frame.
    pub(crate) fn is_mouse_primary(&self, pos: Pos2, pressed: bool) -> bool {
        self.primary_event(pos, pressed)
            .is_some_and(|event| event.ownership == PrimaryOwnership::Mouse)
    }

    fn primary_event(&self, pos: Pos2, pressed: bool) -> Option<&CorrelatedPrimary> {
        self.primary_events
            .iter()
            .rev()
            .find(|event| event.pos == pos && event.phase == PrimaryPhase::Button(pressed))
    }

    pub(crate) fn accepts_grid_tap_release(&self, pos: Pos2) -> bool {
        self.primary_event(pos, false)
            .is_some_and(|event| event.accepted_grid_tap)
    }

    /// Returns true only after the primary event has first been correlated to
    /// an exact touch signature and the recognizer then requested suppression.
    pub(crate) fn should_suppress_primary(&self, pos: Pos2, pressed: bool) -> bool {
        self.primary_event(pos, pressed).is_some_and(|event| {
            event.should_suppress || event.ownership == PrimaryOwnership::Retired
        })
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
                    phase: PrimaryPhase::Button(true),
                    ownership: touch.ownership,
                    should_suppress: touch.should_suppress,
                    accepted_grid_tap: touch.accepted_grid_tap,
                });
                frame.commands.extend(touch.commands);
                true
            }
            PendingSignature::MoveMoved(touch) => {
                if matches!(event, Event::PointerMoved(pos) if *pos == touch.pos) {
                    frame.primary_events.push(CorrelatedPrimary {
                        pos: touch.pos,
                        phase: PrimaryPhase::Moved,
                        ownership: touch.ownership,
                        should_suppress: touch.should_suppress,
                        accepted_grid_tap: false,
                    });
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
                    phase: PrimaryPhase::Button(false),
                    ownership: touch.ownership,
                    should_suppress: touch.should_suppress,
                    accepted_grid_tap: touch.accepted_grid_tap,
                });
                frame.commands.extend(touch.commands);
                self.pending = Some(PendingSignature::EndGone);
                true
            }
            PendingSignature::EndGone => matches!(event, Event::PointerGone),
            PendingSignature::CancelGone => matches!(event, Event::PointerGone),
        }
    }

    fn cancel_stream(&mut self, geometry: &TapZoneGeometry, now_ms: u64) {
        // A real mouse edge can cancel gesture execution without changing
        // egui-winit's independent touch gate. MainGrid keeps that transport
        // mirror until the backend's matching End/Cancel actually releases it.
        if self.recognition.scope() == CommandScope::Unscoped {
            self.pointer_touch = None;
        }
        self.pending = None;
        let _ = self.recognition.recognizer_mut().handle_sample(
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
    phase: PrimaryPhase,
    ownership: PrimaryOwnership,
    should_suppress: bool,
    accepted_grid_tap: bool,
}

#[derive(Clone, Debug)]
struct PendingTouch {
    pos: Pos2,
    ownership: PrimaryOwnership,
    commands: Vec<TouchCommand>,
    should_suppress: bool,
    accepted_grid_tap: bool,
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
    /// egui-winit emits only `PointerGone` after a gated touch cancellation;
    /// unlike `End`, there is no synthetic primary release.
    CancelGone,
}

fn pending_signature_name(pending: Option<&PendingSignature>) -> &'static str {
    match pending {
        Some(PendingSignature::StartMoved(_)) => "StartMoved",
        Some(PendingSignature::StartButton(_)) => "StartButton",
        Some(PendingSignature::MoveMoved(_)) => "MoveMoved",
        Some(PendingSignature::EndButton(_)) => "EndButton",
        Some(PendingSignature::EndGone) => "EndGone",
        Some(PendingSignature::CancelGone) => "CancelGone",
        None => "None",
    }
}

#[derive(Clone, Copy, Debug)]
struct CorrelationStateSnapshot {
    pending: &'static str,
    pointer_touch: bool,
    owner: TouchOwner,
    contacts: usize,
}

impl CorrelationStateSnapshot {
    fn capture(state: &TouchCorrelationState) -> Self {
        Self {
            pending: pending_signature_name(state.pending.as_ref()),
            pointer_touch: state.pointer_touch.is_some(),
            owner: state.recognizer().owner(),
            contacts: state.recognizer().contact_count(),
        }
    }
}

#[derive(Clone, Debug)]
enum AmbiguityCauseKind {
    PendingMismatch { pending: &'static str },
    UnmatchedPrimary,
}

#[derive(Clone, Debug)]
struct AmbiguityCause {
    kind: AmbiguityCauseKind,
    event: Event,
}

#[derive(Clone, Debug)]
struct CorrelationDiagnostics {
    before: CorrelationStateSnapshot,
    after: CorrelationStateSnapshot,
    ambiguity_causes: Vec<AmbiguityCause>,
    cancel_stream_calls: usize,
}

impl CorrelationDiagnostics {
    fn new(state: &TouchCorrelationState) -> Self {
        let snapshot = CorrelationStateSnapshot::capture(state);
        Self {
            before: snapshot,
            after: snapshot,
            ambiguity_causes: Vec::new(),
            cancel_stream_calls: 0,
        }
    }

    fn replay_for_later_passes(&self) -> Self {
        Self {
            before: self.after,
            after: self.after,
            ambiguity_causes: self.ambiguity_causes.clone(),
            cancel_stream_calls: 0,
        }
    }

    fn ambiguous(&self) -> bool {
        !self.ambiguity_causes.is_empty()
    }
}

#[derive(Clone, Debug)]
enum Recognition {
    Unscoped(TouchRecognizer),
    MainGrid {
        scope: egui::Id,
        recognizer: TouchRecognizer,
    },
    Retired(TouchRecognizer),
}

impl Recognition {
    fn scope(&self) -> CommandScope {
        match self {
            Self::Unscoped(_) => CommandScope::Unscoped,
            Self::MainGrid { scope, .. } => CommandScope::MainGrid(*scope),
            Self::Retired(_) => CommandScope::Retired,
        }
    }

    fn recognizer(&self) -> &TouchRecognizer {
        match self {
            Self::Unscoped(recognizer)
            | Self::MainGrid { recognizer, .. }
            | Self::Retired(recognizer) => recognizer,
        }
    }

    fn recognizer_mut(&mut self) -> &mut TouchRecognizer {
        match self {
            Self::Unscoped(recognizer)
            | Self::MainGrid { recognizer, .. }
            | Self::Retired(recognizer) => recognizer,
        }
    }
}

#[derive(Clone, Debug)]
struct TouchCorrelationState {
    recognition: Recognition,
    /// Mirrors egui-winit's `pointer_touch_id`. Its gate is open when this is
    /// `None` or when the incoming touch id matches the stored id.
    pointer_touch: Option<TouchId>,
    pending: Option<PendingSignature>,
    /// Replay-safe result for later egui passes in the same app frame.
    /// Commands are deliberately removed before this value is stored.
    repeatable_frame: TouchFrame,
    /// Diagnostics for the replay result. This is populated only under the
    /// existing `MIV_TOUCH_DEBUG` gate.
    repeatable_diagnostics: Option<CorrelationDiagnostics>,
}

impl Default for TouchCorrelationState {
    fn default() -> Self {
        Self {
            recognition: Recognition::Unscoped(TouchRecognizer::new()),
            pointer_touch: None,
            pending: None,
            repeatable_frame: TouchFrame::default(),
            repeatable_diagnostics: None,
        }
    }
}

#[derive(Default)]
struct FrameBuilder {
    commands: Vec<TouchCommand>,
    primary_events: Vec<CorrelatedPrimary>,
    ambiguous: bool,
    touch_cancelled: bool,
    diagnostics: Option<CorrelationDiagnostics>,
}

impl FrameBuilder {
    fn new(state: &TouchCorrelationState, diagnostics_enabled: bool) -> Self {
        Self {
            diagnostics: diagnostics_enabled.then(|| CorrelationDiagnostics::new(state)),
            ..Self::default()
        }
    }

    fn record_pending_mismatch(&mut self, pending: Option<&'static str>, event: &Event) {
        if let (Some(diagnostics), Some(pending)) = (&mut self.diagnostics, pending) {
            diagnostics.ambiguity_causes.push(AmbiguityCause {
                kind: AmbiguityCauseKind::PendingMismatch { pending },
                event: event.clone(),
            });
        }
    }

    fn record_unmatched_primary(&mut self, event: &Event) {
        if let Some(diagnostics) = &mut self.diagnostics {
            diagnostics.ambiguity_causes.push(AmbiguityCause {
                kind: AmbiguityCauseKind::UnmatchedPrimary,
                event: event.clone(),
            });
        }
    }

    fn record_cancel_stream_call(&mut self) {
        if let Some(diagnostics) = &mut self.diagnostics {
            diagnostics.cancel_stream_calls += 1;
        }
    }

    fn finish(
        mut self,
        scope: CommandScope,
        owner: TouchOwner,
        active: bool,
        after: CorrelationStateSnapshot,
    ) -> (TouchFrame, Option<CorrelationDiagnostics>) {
        if self.ambiguous {
            // Ambiguous input cannot authorize a current gesture action or
            // suppress its ordinary pointer fallback.
            self.commands.clear();
            // Exact retired provenance remains valid even when a separate
            // real mouse event makes current gesture execution ambiguous.
            self.primary_events
                .retain(|event| !matches!(event.ownership, PrimaryOwnership::Current(_)));
        }
        if let Some(diagnostics) = &mut self.diagnostics {
            diagnostics.after = after;
        }
        (
            TouchFrame {
                scope,
                commands: self.commands,
                primary_events: self.primary_events,
                owner,
                active,
                touch_cancelled: self.touch_cancelled,
            },
            self.diagnostics,
        )
    }
}

impl TouchCorrelationState {
    fn recognizer(&self) -> &TouchRecognizer {
        self.recognition.recognizer()
    }

    fn select_scope(&mut self, scope: CommandScope) {
        if self.recognition.scope() == scope {
            return;
        }
        self.recognition = match scope {
            CommandScope::Unscoped => Recognition::Unscoped(TouchRecognizer::new()),
            CommandScope::MainGrid(scope) => Recognition::MainGrid {
                scope,
                recognizer: TouchRecognizer::new(),
            },
            CommandScope::Retired => Recognition::Retired(TouchRecognizer::new()),
        };
        if let Some(pending) = &mut self.pending {
            match pending {
                PendingSignature::StartMoved(touch)
                | PendingSignature::StartButton(touch)
                | PendingSignature::MoveMoved(touch)
                | PendingSignature::EndButton(touch) => {
                    touch.commands.clear();
                    touch.ownership = PrimaryOwnership::Retired;
                    touch.accepted_grid_tap = false;
                }
                PendingSignature::EndGone | PendingSignature::CancelGone => {}
            }
        }
        self.repeatable_frame.retire();
    }
    #[cfg(test)]
    fn process_frame(
        &mut self,
        geometry: &TapZoneGeometry,
        events: &[Event],
        now_ms: u64,
        execution_enabled: bool,
    ) -> TouchFrame {
        self.process_frame_with_diagnostics(geometry, events, now_ms, execution_enabled, false)
            .0
    }

    fn process_frame_with_diagnostics(
        &mut self,
        geometry: &TapZoneGeometry,
        events: &[Event],
        now_ms: u64,
        execution_enabled: bool,
        diagnostics_enabled: bool,
    ) -> (TouchFrame, Option<CorrelationDiagnostics>) {
        let mut frame = FrameBuilder::new(self, diagnostics_enabled);
        for event in events {
            self.process_event(geometry, event, now_ms, &mut frame);
        }
        let (mut touch_frame, diagnostics) = frame.finish(
            self.recognition.scope(),
            self.recognizer().owner(),
            self.recognizer().is_active(),
            CorrelationStateSnapshot::capture(self),
        );
        if !execution_enabled {
            touch_frame.suppress_execution();
        }
        (touch_frame, diagnostics)
    }

    fn reset_for_global_disable(
        &mut self,
        diagnostics_enabled: bool,
    ) -> (TouchFrame, Option<CorrelationDiagnostics>) {
        let mut frame = FrameBuilder::new(self, diagnostics_enabled);
        frame.touch_cancelled = self.recognizer().is_active();
        *self = Self::default();
        frame.finish(
            self.recognition.scope(),
            self.recognizer().owner(),
            self.recognizer().is_active(),
            CorrelationStateSnapshot::capture(self),
        )
    }

    fn process_event(
        &mut self,
        geometry: &TapZoneGeometry,
        event: &Event,
        now_ms: u64,
        frame: &mut FrameBuilder,
    ) {
        if self.pending.is_some() {
            let pending = frame
                .diagnostics
                .as_ref()
                .map(|_| pending_signature_name(self.pending.as_ref()));
            if self.advance_pending(event, frame) {
                return;
            }

            // Exact adjacency is part of the correlation contract. Reprocess
            // the mismatching event after abandoning the ambiguous touch
            // stream so a later, independent exact sequence can still start.
            frame.ambiguous = true;
            frame.record_pending_mismatch(pending, event);
            frame.record_cancel_stream_call();
            self.cancel_stream(geometry, now_ms);
        }

        match event {
            Event::Touch { id, phase, pos, .. } => {
                self.handle_touch(geometry, *id, *phase, *pos, now_ms, frame)
            }
            Event::PointerButton {
                button: PointerButton::Primary,
                pos,
                pressed,
                ..
            } => {
                // A primary button without a pending exact touch signature is
                // existing mouse input. If touch input is also present in this
                // pass, fail closed and cancel all touch-side action.
                frame.ambiguous = true;
                frame.record_unmatched_primary(event);
                frame.record_cancel_stream_call();
                self.cancel_stream(geometry, now_ms);
                frame.primary_events.push(CorrelatedPrimary {
                    pos: *pos,
                    phase: PrimaryPhase::Button(*pressed),
                    ownership: PrimaryOwnership::Mouse,
                    should_suppress: false,
                    accepted_grid_tap: false,
                });
            }
            Event::PointerMoved(pos) => frame.primary_events.push(CorrelatedPrimary {
                pos: *pos,
                phase: PrimaryPhase::Moved,
                ownership: PrimaryOwnership::Mouse,
                should_suppress: false,
                accepted_grid_tap: false,
            }),
            _ => {}
        }
    }

    fn handle_touch(
        &mut self,
        geometry: &TapZoneGeometry,
        id: TouchId,
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
        let sample = TouchSample {
            id: id.0,
            pos,
            phase,
            now_ms,
        };

        // Mirrors egui-winit 0.33.3's `on_touch` gate one-for-one:
        // `pointer_touch_id.is_none() || pointer_touch_id == Some(id)`.
        // In particular, after the stored contact ends the gate reopens for
        // Move/End events from another contact that is still down.
        let receives_synthetic_pointer =
            self.pointer_touch.is_none() || self.pointer_touch == Some(id);
        let scope = self.recognition.scope();
        let accepted = match scope {
            CommandScope::Unscoped => true,
            CommandScope::MainGrid(_) => {
                phase == TouchPhase::Start || self.recognizer().has_contact(id.0)
            }
            CommandScope::Retired => false,
        };
        frame.touch_cancelled |= phase == TouchPhase::Cancel && accepted;
        let accepted_grid_tap = geometry.behavior == crate::touch_input::TouchSurfaceBehavior::Grid
            && accepted
            && self.recognizer().accepts_grid_tap_end(sample);
        let recognizer_was_active = self.recognizer().is_active();
        let commands = if !accepted
            || (matches!(phase, TouchPhase::Move | TouchPhase::End) && !recognizer_was_active)
        {
            // Keep the dependency mirror exact for a stray tail, but never
            // reuse suppression retained by an already completed stream.
            Vec::new()
        } else {
            self.recognition
                .recognizer_mut()
                .handle_sample(geometry, sample)
        };
        let pending = PendingTouch {
            pos,
            ownership: if accepted {
                PrimaryOwnership::Current(scope)
            } else {
                PrimaryOwnership::Retired
            },
            accepted_grid_tap,
            commands,
            // This is sampled only after the raw touch event has updated the
            // recognizer. It is never consulted for an uncorrelated primary.
            should_suppress: recognizer_was_active && self.recognizer().should_suppress_primary(),
        };

        if !receives_synthetic_pointer {
            // A different contact has no synthetic pointer events while the
            // egui-winit pointer touch id remains occupied.
            frame.commands.extend(pending.commands);
            return;
        }

        match phase {
            TouchPhase::Start => {
                self.pointer_touch = Some(id);
                self.pending = Some(PendingSignature::StartMoved(pending));
            }
            TouchPhase::Move => {
                self.pending = Some(PendingSignature::MoveMoved(pending));
            }
            TouchPhase::End => {
                self.pointer_touch = None;
                self.pending = Some(PendingSignature::EndButton(pending));
            }
            TouchPhase::Cancel => {
                self.pointer_touch = None;
                self.pending = Some(PendingSignature::CancelGone);
            }
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

pub(crate) fn touch_gestures_disabled() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| std::env::var_os("MIV_DISABLE_TOUCH_GESTURES").is_some())
}

fn state_id(viewport: egui::ViewportId, surface: TouchSurface) -> egui::Id {
    egui::Id::unique(("miv_touch_correlation", viewport, surface))
}

fn driven_surface_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("miv_touch_correlation_driven_surface", viewport))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DrivenSurface {
    frame: u64,
    outcome: FrameDrive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameDrive {
    GridDriven(CommandScope),
    StillDriven,
    StillDrivenAndGridTransport,
    GridTransportOnly,
}

impl FrameDrive {
    fn surface(self) -> Option<TouchSurface> {
        match self {
            Self::GridDriven(_) => Some(TouchSurface::MainGrid),
            Self::StillDriven | Self::StillDrivenAndGridTransport => {
                Some(TouchSurface::StillFullscreen)
            }
            Self::GridTransportOnly => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiagnosticDriveKind {
    Commands,
    Replay,
}

impl DiagnosticDriveKind {
    fn label(self) -> &'static str {
        match self {
            Self::Commands => "commands",
            Self::Replay => "replay",
        }
    }
}

/// Drive correlation once for the active surface and app frame.
///
/// `MainGrid` is called only from the grid `CentralPanel`; `StillFullscreen`
/// only from the non-video/non-music fullscreen `CentralPanel`. Embedded still
/// fullscreen returns before grid rendering, and a separate fullscreen window
/// has a different `ViewportId`. The frame marker is a fail-closed backstop,
/// including when egui performs more than one pass for the same app frame.
/// The first drive may return commands; repeated drives replay correlation and
/// suppression state with an empty command list.
/// `enabled` gates only caller-side command execution and suppression advice.
/// Raw touch events always advance the contact tracker, pending signature, and
/// egui-winit `pointer_touch_id` mirror so a disabled interval cannot corrupt
/// the next enabled stream. The process-wide diagnostic escape hatch
/// `MIV_DISABLE_TOUCH_GESTURES` remains a separate hard-disable path.
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
    drive_egui_touch_input_with_policy(
        ctx,
        surface,
        geometry,
        frame,
        enabled,
        touch_gestures_disabled(),
        CommandScope::Unscoped,
    )
}

/// MainGrid recognition is owned by the visible viewer context and list session;
/// transport correlation stays viewport-scoped across their replacement.
pub(crate) fn drive_egui_grid_touch_input(
    ctx: &egui::Context,
    geometry: TapZoneGeometry,
    scope: egui::Id,
    enabled: bool,
) -> TouchFrame {
    drive_egui_touch_input_with_policy(
        ctx,
        TouchSurface::MainGrid,
        geometry,
        ctx.cumulative_frame_nr(),
        enabled,
        touch_gestures_disabled(),
        CommandScope::MainGrid(scope),
    )
}

/// Register inside the existing context-input installation boundary. The callback
/// runs in the actual viewport, including passes that never draw MainGrid.
pub(crate) fn install_main_grid_touch_transport_observer(ctx: &egui::Context) {
    ctx.on_end_pass(
        "miv_main_grid_touch_transport",
        std::sync::Arc::new(|ui| {
            observe_main_grid_transport(ui.ctx());
        }),
    );
}

fn observe_main_grid_transport(ctx: &egui::Context) {
    let viewport = ctx.viewport_id();
    let frame = ctx.cumulative_frame_nr();
    let (events, now_ms) = input_events_and_time(ctx);
    ctx.data_mut(|data| {
        let marker_id = driven_surface_id(viewport);
        let previous = data
            .get_temp::<DrivenSurface>(marker_id)
            .filter(|marker| marker.frame == frame);
        let outcome = match previous.map(|marker| marker.outcome) {
            Some(
                FrameDrive::GridDriven(_)
                | FrameDrive::GridTransportOnly
                | FrameDrive::StillDrivenAndGridTransport,
            ) => return,
            Some(FrameDrive::StillDriven) => FrameDrive::StillDrivenAndGridTransport,
            None => FrameDrive::GridTransportOnly,
        };
        let geometry = TapZoneGeometry {
            surface: egui::Rect::NOTHING,
            excluded: Vec::new(),
            behavior: crate::touch_input::TouchSurfaceBehavior::Grid,
        };
        let _ = process_surface_state(
            data,
            viewport,
            TouchSurface::MainGrid,
            CommandScope::Retired,
            &geometry,
            &events,
            now_ms,
            false,
            touch_gestures_disabled(),
            false,
        );
        data.insert_temp(marker_id, DrivenSurface { frame, outcome });
    });
}

#[cfg(test)]
fn drive_egui_touch_input_inner(
    ctx: &egui::Context,
    surface: TouchSurface,
    geometry: TapZoneGeometry,
    frame: u64,
    execution_enabled: bool,
) -> TouchFrame {
    drive_egui_touch_input_with_policy(
        ctx,
        surface,
        geometry,
        frame,
        execution_enabled,
        false,
        CommandScope::Unscoped,
    )
}

#[allow(clippy::too_many_arguments)]
fn drive_egui_touch_input_with_policy(
    ctx: &egui::Context,
    surface: TouchSurface,
    geometry: TapZoneGeometry,
    frame: u64,
    execution_enabled: bool,
    gestures_disabled: bool,
    scope: CommandScope,
) -> TouchFrame {
    let viewport = ctx.viewport_id();
    let (events, now_ms) = input_events_and_time(ctx);
    process_in_temp_data(
        ctx,
        viewport,
        frame,
        surface,
        scope,
        geometry,
        events,
        now_ms,
        execution_enabled,
        gestures_disabled,
    )
}

fn input_events_and_time(ctx: &egui::Context) -> (Vec<Event>, u64) {
    ctx.input(|input| {
        let millis = if input.time.is_finite() && input.time > 0.0 {
            (input.time * 1000.0).min(u64::MAX as f64) as u64
        } else {
            0
        };
        (input.raw.events.clone(), millis)
    })
}

#[allow(clippy::too_many_arguments)]
fn process_in_temp_data(
    ctx: &egui::Context,
    viewport: egui::ViewportId,
    frame: u64,
    surface: TouchSurface,
    scope: CommandScope,
    geometry: TapZoneGeometry,
    events: Vec<Event>,
    now_ms: u64,
    execution_enabled: bool,
    gestures_disabled: bool,
) -> TouchFrame {
    let diagnostics_enabled = crate::touch_debug::touch_debug_enabled();
    let pass = diagnostics_enabled.then(|| (ctx.cumulative_pass_nr(), ctx.current_pass_index()));
    let contains_touch = diagnostics_enabled
        && events
            .iter()
            .any(|event| matches!(event, Event::Touch { .. }));
    let (touch_frame, diagnostics, drive_kind, surface_match) = ctx.data_mut(|data| {
        let marker_id = driven_surface_id(viewport);
        if let Some(marker) = data.get_temp::<DrivenSurface>(marker_id)
            && marker.frame == frame
        {
            let (touch_frame, diagnostics, surface_match) =
                repeated_drive_result(data, viewport, surface, scope, marker, diagnostics_enabled);
            return (
                touch_frame,
                diagnostics,
                DiagnosticDriveKind::Replay,
                surface_match,
            );
        }
        let outcome = match surface {
            TouchSurface::MainGrid => FrameDrive::GridDriven(scope),
            TouchSurface::StillFullscreen => FrameDrive::StillDriven,
        };
        data.insert_temp(marker_id, DrivenSurface { frame, outcome });

        let (touch_frame, diagnostics) = process_surface_state(
            data,
            viewport,
            surface,
            scope,
            &geometry,
            &events,
            now_ms,
            execution_enabled,
            gestures_disabled,
            diagnostics_enabled,
        );
        (
            touch_frame,
            diagnostics,
            DiagnosticDriveKind::Commands,
            true,
        )
    });

    if let (Some((egui_pass, pass_index)), Some(diagnostics)) = (pass, diagnostics)
        && (contains_touch || diagnostics.ambiguous())
    {
        log_correlation_diagnostics(
            viewport,
            surface,
            frame,
            egui_pass,
            pass_index,
            drive_kind,
            surface_match,
            &touch_frame,
            &diagnostics,
        );
    }
    touch_frame
}

fn repeated_drive_result(
    data: &mut egui::util::IdTypeMap,
    viewport: egui::ViewportId,
    surface: TouchSurface,
    scope: CommandScope,
    marker: DrivenSurface,
    diagnostics_enabled: bool,
) -> (TouchFrame, Option<CorrelationDiagnostics>, bool) {
    // A later pass may adopt another list, but the raw frame is already
    // processed. Retire its authority without re-running Start or commands.
    if surface == TouchSurface::MainGrid && scope != CommandScope::Unscoped {
        let id = state_id(viewport, surface);
        if let Some(mut state) = data.get_temp::<TouchCorrelationState>(id) {
            state.select_scope(scope);
            data.insert_temp(id, state);
        }
    }
    if marker.outcome.surface() != Some(surface)
        && !(surface == TouchSurface::MainGrid
            && matches!(
                marker.outcome,
                FrameDrive::GridTransportOnly | FrameDrive::StillDrivenAndGridTransport
            ))
    {
        let diagnostics = diagnostics_enabled.then(|| {
            let state = data
                .get_temp::<TouchCorrelationState>(state_id(viewport, surface))
                .unwrap_or_default();
            CorrelationDiagnostics::new(&state)
        });
        return (TouchFrame::default(), diagnostics, false);
    }
    data.get_temp::<TouchCorrelationState>(state_id(viewport, surface))
        .map_or_else(
            || {
                let diagnostics = diagnostics_enabled
                    .then(|| CorrelationDiagnostics::new(&TouchCorrelationState::default()));
                (TouchFrame::default(), diagnostics, true)
            },
            |state| (state.repeatable_frame, state.repeatable_diagnostics, true),
        )
}

#[allow(clippy::too_many_arguments)]
fn process_surface_state(
    data: &mut egui::util::IdTypeMap,
    viewport: egui::ViewportId,
    surface: TouchSurface,
    scope: CommandScope,
    geometry: &TapZoneGeometry,
    events: &[Event],
    now_ms: u64,
    execution_enabled: bool,
    gestures_disabled: bool,
    diagnostics_enabled: bool,
) -> (TouchFrame, Option<CorrelationDiagnostics>) {
    let id = state_id(viewport, surface);
    let mut state = data
        .get_temp::<TouchCorrelationState>(id)
        .unwrap_or_default();
    state.select_scope(scope);
    let (frame, diagnostics) = if gestures_disabled {
        state.reset_for_global_disable(diagnostics_enabled)
    } else {
        state.process_frame_with_diagnostics(
            geometry,
            events,
            now_ms,
            execution_enabled,
            diagnostics_enabled,
        )
    };
    state.repeatable_frame = frame.replay_for_later_passes();
    state.repeatable_diagnostics = diagnostics
        .as_ref()
        .map(CorrelationDiagnostics::replay_for_later_passes);
    data.insert_temp(id, state);
    (frame, diagnostics)
}

#[allow(clippy::too_many_arguments)]
fn log_correlation_diagnostics(
    viewport: egui::ViewportId,
    surface: TouchSurface,
    app_frame: u64,
    egui_pass: u64,
    pass_index: usize,
    drive_kind: DiagnosticDriveKind,
    surface_match: bool,
    frame: &TouchFrame,
    diagnostics: &CorrelationDiagnostics,
) {
    if !crate::touch_debug::touch_debug_enabled() {
        return;
    }
    crate::logger::log(format_correlation_diagnostics(
        viewport,
        surface,
        app_frame,
        egui_pass,
        pass_index,
        drive_kind,
        surface_match,
        frame,
        diagnostics,
    ));
}

#[allow(clippy::too_many_arguments)]
fn format_correlation_diagnostics(
    viewport: egui::ViewportId,
    surface: TouchSurface,
    app_frame: u64,
    egui_pass: u64,
    pass_index: usize,
    drive_kind: DiagnosticDriveKind,
    surface_match: bool,
    frame: &TouchFrame,
    diagnostics: &CorrelationDiagnostics,
) -> String {
    let mut ambiguity = String::new();
    for (index, cause) in diagnostics.ambiguity_causes.iter().enumerate() {
        if index != 0 {
            ambiguity.push('|');
        }
        match cause.kind {
            AmbiguityCauseKind::PendingMismatch { pending } => {
                let _ = write!(ambiguity, "pending_mismatch(pending={pending},event=");
            }
            AmbiguityCauseKind::UnmatchedPrimary => {
                ambiguity.push_str("unmatched_primary(event=");
            }
        }
        write_diagnostic_event(&mut ambiguity, &cause.event);
        ambiguity.push(')');
    }
    if ambiguity.is_empty() {
        ambiguity.push_str("none");
    }

    let mut command_kinds = String::new();
    for (index, command) in frame.commands().iter().enumerate() {
        if index != 0 {
            command_kinds.push(',');
        }
        command_kinds.push_str(touch_command_name(command));
    }

    let mut out = String::new();
    let _ = write!(
        out,
        "[TOUCH-DEBUG] correlation viewport={viewport:?} surface={surface:?} app_frame={app_frame} egui_pass={egui_pass} pass_index={pass_index}"
    );
    let _ = write!(
        out,
        " drive={} surface_match={surface_match} ambiguous={} ambiguity=[{ambiguity}] cancel_stream_calls={}",
        drive_kind.label(),
        diagnostics.ambiguous(),
        diagnostics.cancel_stream_calls,
    );
    let _ = write!(
        out,
        " pending={}->{} pointer_touch={}->{} owner={:?}->{:?}",
        diagnostics.before.pending,
        diagnostics.after.pending,
        presence_name(diagnostics.before.pointer_touch),
        presence_name(diagnostics.after.pointer_touch),
        diagnostics.before.owner,
        diagnostics.after.owner,
    );
    let _ = write!(
        out,
        " commands={}[{}] contacts={}->{}",
        frame.commands().len(),
        command_kinds,
        diagnostics.before.contacts,
        diagnostics.after.contacts,
    );
    out
}

fn presence_name(present: bool) -> &'static str {
    if present { "present" } else { "absent" }
}

fn touch_command_name(command: &TouchCommand) -> &'static str {
    match command {
        TouchCommand::ToggleChrome => "ToggleChrome",
        TouchCommand::PageSide { .. } => "PageSide",
        TouchCommand::Zoom { .. } => "Zoom",
        TouchCommand::Pan { .. } => "Pan",
        TouchCommand::ScrollGrid { .. } => "ScrollGrid",
        TouchCommand::ScrollGridEnd => "ScrollGridEnd",
        TouchCommand::PinchEnd => "PinchEnd",
    }
}

fn write_diagnostic_event(out: &mut String, event: &Event) {
    match event {
        Event::Touch {
            device_id,
            id,
            phase,
            pos,
            ..
        } => {
            let _ = write!(
                out,
                "Touch(device_id={} id={} phase={phase:?} pos=({:.1},{:.1}))",
                device_id.0, id.0, pos.x, pos.y
            );
        }
        Event::PointerMoved(pos) => {
            let _ = write!(out, "PointerMoved(pos=({:.1},{:.1}))", pos.x, pos.y);
        }
        Event::PointerButton {
            pos,
            button,
            pressed,
            ..
        } => {
            let _ = write!(
                out,
                "PointerButton(pos=({:.1},{:.1}) button={button:?} pressed={pressed})",
                pos.x, pos.y
            );
        }
        Event::PointerGone => out.push_str("PointerGone"),
        _ => {
            let event = format!("{event:?}")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::touch_input::TouchOwner;
    use egui::{Modifiers, Rect, TouchDeviceId, pos2};

    fn geometry() -> TapZoneGeometry {
        TapZoneGeometry {
            surface: Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 800.0)),
            excluded: Vec::new(),
            behavior: crate::touch_input::TouchSurfaceBehavior::Viewer {
                accepts_pinch: true,
                tap_zones: true,
            },
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
        state.process_frame(&geometry(), events, now_ms, true)
    }

    fn drive_twice_in_app_frame(
        ctx: &egui::Context,
        geometry: &TapZoneGeometry,
        frame: u64,
        time: f64,
        events: Vec<Event>,
    ) -> (TouchFrame, TouchFrame) {
        let raw = egui::RawInput {
            screen_rect: Some(geometry.surface),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut frames = None;
        ctx.run_pass(raw, |ctx| {
            frames = Some((
                drive_egui_touch_input_inner(
                    ctx,
                    TouchSurface::StillFullscreen,
                    geometry.clone(),
                    frame,
                    true,
                ),
                drive_egui_touch_input_inner(
                    ctx,
                    TouchSurface::StillFullscreen,
                    geometry.clone(),
                    frame,
                    true,
                ),
            ));
        })
        .drop_without_applying_deltas();
        frames.unwrap()
    }

    #[test]
    fn correlation_diagnostic_line_carries_frame_state_and_command_kinds() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let (frame, diagnostics) =
            state.process_frame_with_diagnostics(&geometry(), &tap_events(1, pos), 100, true, true);
        let diagnostics = diagnostics.unwrap();
        let line = format_correlation_diagnostics(
            egui::ViewportId::ROOT,
            TouchSurface::StillFullscreen,
            42,
            77,
            1,
            DiagnosticDriveKind::Commands,
            true,
            &frame,
            &diagnostics,
        );

        assert!(line.contains("surface=StillFullscreen app_frame=42 egui_pass=77 pass_index=1"));
        assert!(line.contains("drive=commands surface_match=true ambiguous=false"));
        assert!(line.contains("ambiguity=[none] cancel_stream_calls=0"));
        assert!(line.contains("pending=None->None pointer_touch=absent->absent"));
        assert!(line.contains("owner=Undecided->ViewerTapZone"));
        assert!(line.contains("commands=1[ToggleChrome] contacts=0->0"));
        assert!(!line.contains('\r') && !line.contains('\n'));

        let replay_frame = frame.replay_for_later_passes();
        let replay_diagnostics = diagnostics.replay_for_later_passes();
        let replay_line = format_correlation_diagnostics(
            egui::ViewportId::ROOT,
            TouchSurface::StillFullscreen,
            42,
            78,
            2,
            DiagnosticDriveKind::Replay,
            true,
            &replay_frame,
            &replay_diagnostics,
        );
        assert!(replay_line.contains("drive=replay"));
        assert!(replay_line.contains("commands=0[]"));
        assert!(replay_line.contains("cancel_stream_calls=0"));
    }

    #[test]
    fn correlation_diagnostic_names_both_fail_closed_causes() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let events = [touch(1, egui::TouchPhase::Start, pos), primary(pos, true)];
        let (frame, diagnostics) =
            state.process_frame_with_diagnostics(&geometry(), &events, 100, true, true);
        let line = format_correlation_diagnostics(
            egui::ViewportId::ROOT,
            TouchSurface::StillFullscreen,
            43,
            78,
            0,
            DiagnosticDriveKind::Commands,
            true,
            &frame,
            &diagnostics.unwrap(),
        );

        assert!(frame.commands().is_empty());
        assert!(line.contains("ambiguous=true"));
        assert!(line.contains(
            "pending_mismatch(pending=StartMoved,event=PointerButton(pos=(500.0,400.0) button=Primary pressed=true))"
        ));
        assert!(line.contains(
            "unmatched_primary(event=PointerButton(pos=(500.0,400.0) button=Primary pressed=true))"
        ));
        assert!(line.contains("cancel_stream_calls=2"));
    }

    #[test]
    fn mouse_only_never_classifies_or_suppresses_primary_input() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let frame = process(&mut state, &mouse_click(pos), 100);

        assert!(frame.commands().is_empty());
        assert!(
            frame
                .primary_events
                .iter()
                .all(|event| event.ownership == PrimaryOwnership::Mouse)
        );
        assert_eq!(frame.correlated_primary_release_positions().count(), 0);
        assert!(!frame.should_suppress_primary(pos, true));
        assert!(!frame.should_suppress_primary(pos, false));
        assert!(!frame.has_touch_derived_pointer_activity());
    }

    #[test]
    fn unmatched_real_mouse_release_still_fails_closed_as_ambiguous() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let (frame, diagnostics) = state.process_frame_with_diagnostics(
            &geometry(),
            &[primary(pos, false)],
            100,
            true,
            true,
        );

        assert!(frame.commands().is_empty());
        assert!(
            frame
                .primary_events
                .iter()
                .all(|event| event.ownership == PrimaryOwnership::Mouse)
        );
        assert!(diagnostics.unwrap().ambiguous());
        assert_eq!(state.recognizer().owner(), TouchOwner::Cancelled);
    }

    #[test]
    fn correlated_touch_release_exposes_its_exact_position() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let frame = process(&mut state, &tap_events(1, pos), 100);

        assert_eq!(
            frame
                .correlated_primary_release_positions()
                .collect::<Vec<_>>(),
            vec![pos]
        );
        assert!(frame.has_touch_derived_pointer_activity());
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
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        });

        let frame = process(&mut state, &events, 100);
        assert!(frame.commands().is_empty());
        assert!(
            frame
                .primary_events
                .iter()
                .all(|event| event.ownership == PrimaryOwnership::Mouse)
        );
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
        assert!(state.recognizer().should_suppress_primary());

        // The recognizer retains true after End, but this adapter never asks
        // it about a primary event until correlation succeeds first.
        let mouse_pos = pos2(100.0, 100.0);
        let mouse_frame = process(&mut state, &mouse_click(mouse_pos), 200);
        assert!(mouse_frame.commands().is_empty());
        assert!(
            mouse_frame
                .primary_events
                .iter()
                .all(|event| event.ownership == PrimaryOwnership::Mouse)
        );
        assert!(!mouse_frame.should_suppress_primary(mouse_pos, false));
    }

    #[test]
    fn stray_touch_tail_tracks_the_dependency_gate_without_reusing_suppression() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let _ = process(&mut state, &tap_events(1, pos), 100);
        assert!(state.recognizer().should_suppress_primary());

        let stray = [
            touch(2, egui::TouchPhase::End, pos),
            primary(pos, false),
            Event::PointerGone,
        ];
        let frame = process(&mut state, &stray, 200);
        assert!(frame.commands().is_empty());
        assert_eq!(
            frame
                .correlated_primary_release_positions()
                .collect::<Vec<_>>(),
            vec![pos]
        );
        assert!(!frame.should_suppress_primary(pos, false));
    }

    /// Dependency guardian for `egui-winit-0.33.3/src/lib.rs` `on_touch`.
    /// Its gate is exactly
    /// `pointer_touch_id.is_none() || pointer_touch_id == Some(id)`.
    /// If this premise changes, touch commands can disappear; loosening the
    /// check could instead misclassify and suppress existing mouse input.
    #[test]
    fn egui_winit_0_33_3_signature_is_exact_and_ordered() {
        let mut state = TouchCorrelationState::default();
        let first = pos2(200.0, 400.0);
        let second = pos2(800.0, 400.0);
        let pinched = pos2(850.0, 400.0);
        let remaining = pos2(900.0, 400.0);
        // Start: Touch -> moved -> pressed; Move: Touch -> moved;
        // End: Touch -> released -> gone.
        // The second Start has no pointer tail while the first id holds the
        // gate. Once the first id ends, the remaining contact's Move and End
        // pass the reopened gate even though that contact was never pressed.
        let accepted = vec![
            touch(1, egui::TouchPhase::Start, first),
            Event::PointerMoved(first),
            primary(first, true),
            touch(2, egui::TouchPhase::Start, second),
            touch(2, egui::TouchPhase::Move, pinched),
            touch(1, egui::TouchPhase::End, first),
            primary(first, false),
            Event::PointerGone,
            touch(2, egui::TouchPhase::Move, remaining),
            Event::PointerMoved(remaining),
            touch(2, egui::TouchPhase::End, remaining),
            primary(remaining, false),
            Event::PointerGone,
        ];
        let frame = process(&mut state, &accepted, 100);
        assert!(
            frame
                .commands()
                .iter()
                .any(|command| matches!(command, TouchCommand::Zoom { .. }))
        );
        assert!(
            frame
                .commands()
                .iter()
                .any(|command| matches!(command, TouchCommand::Pan { .. }))
        );
        assert_eq!(frame.commands().last(), Some(&TouchCommand::PinchEnd));
        assert!(frame.should_suppress_primary(first, false));
        assert!(frame.should_suppress_primary(remaining, false));

        // Cancel uses the same gate, clears the mirrored id, and has only a
        // PointerGone tail. A synthetic release here must fail closed.
        let mut cancelled = TouchCorrelationState::default();
        let cancel_start = vec![
            touch(3, egui::TouchPhase::Start, first),
            Event::PointerMoved(first),
            primary(first, true),
        ];
        let _ = process(&mut cancelled, &cancel_start, 200);
        let cancel = [
            touch(3, egui::TouchPhase::Cancel, first),
            Event::PointerGone,
        ];
        let cancel_frame = process(&mut cancelled, &cancel, 201);
        assert!(
            cancel_frame
                .primary_events
                .iter()
                .all(|event| event.ownership == PrimaryOwnership::Mouse)
        );
        assert!(cancelled.pointer_touch.is_none());
        assert!(cancelled.pending.is_none());

        let mut invalid_cancel = TouchCorrelationState::default();
        let _ = process(&mut invalid_cancel, &cancel_start, 300);
        let invalid = [
            touch(3, egui::TouchPhase::Cancel, first),
            primary(first, false),
            Event::PointerGone,
        ];
        let (_, diagnostics) =
            invalid_cancel.process_frame_with_diagnostics(&geometry(), &invalid, 301, true, true);
        assert!(diagnostics.unwrap().ambiguous());
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
        assert!(
            frame
                .primary_events
                .iter()
                .all(|event| event.ownership == PrimaryOwnership::Mouse)
        );
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
            .filter(|event| event.phase == PrimaryPhase::Button(false) && event.should_suppress)
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
        assert!(!state.recognizer().is_active());
        assert_eq!(state.recognizer().owner(), TouchOwner::Cancelled);
    }

    #[test]
    fn execution_disabled_hides_actions_but_keeps_observation_state() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let start = vec![
            touch(1, egui::TouchPhase::Start, pos),
            Event::PointerMoved(pos),
            primary(pos, true),
        ];
        let disabled_start = state.process_frame(&geometry(), &start, 100, false);
        assert!(disabled_start.commands().is_empty());
        assert!(!disabled_start.should_suppress_primary(pos, true));
        assert!(disabled_start.has_touch_derived_pointer_activity());
        assert!(state.recognizer().is_active());
        assert_eq!(state.recognizer().contact_count(), 1);
        assert_eq!(state.pointer_touch, Some(TouchId(1)));
        assert!(state.pending.is_none());

        // Re-enable after the Start was observed while execution was blocked.
        // The matching End must close the same mirrored stream, not a reset or
        // a stray tail, and the following gesture must still work normally.
        let end = vec![
            touch(1, egui::TouchPhase::End, pos),
            primary(pos, false),
            Event::PointerGone,
        ];
        let reenabled_end = state.process_frame(&geometry(), &end, 200, true);
        assert_eq!(reenabled_end.commands(), &[TouchCommand::ToggleChrome]);
        assert!(reenabled_end.should_suppress_primary(pos, false));
        assert!(!state.recognizer().is_active());
        assert_eq!(state.recognizer().contact_count(), 0);
        assert!(state.pointer_touch.is_none());
        assert!(state.pending.is_none());

        let next = state.process_frame(&geometry(), &tap_events(2, pos), 300, true);
        assert_eq!(next.commands(), &[TouchCommand::ToggleChrome]);
        assert!(next.should_suppress_primary(pos, false));
    }

    #[test]
    fn global_gesture_disable_retains_the_legacy_hard_reset() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let start = vec![
            touch(1, egui::TouchPhase::Start, pos),
            Event::PointerMoved(pos),
            primary(pos, true),
        ];
        let _ = process(&mut state, &start, 100);
        assert!(state.recognizer().is_active());

        let (frame, _) = state.reset_for_global_disable(false);
        assert!(frame.commands().is_empty());
        assert!(
            frame
                .primary_events
                .iter()
                .all(|event| event.ownership == PrimaryOwnership::Mouse)
        );
        assert!(frame.touch_cancelled());
        assert!(!state.recognizer().is_active());
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
        assert!(state.recognizer().is_active());
        assert!(state.pointer_touch.is_some());

        let cancel = [touch(1, egui::TouchPhase::Cancel, pos), Event::PointerGone];
        let frame = process(&mut state, &cancel, 200);
        assert!(frame.commands().is_empty());
        assert!(
            frame
                .primary_events
                .iter()
                .all(|event| event.ownership == PrimaryOwnership::Mouse)
        );
        assert!(!state.recognizer().is_active());
        assert_eq!(state.recognizer().owner(), TouchOwner::Cancelled);
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
        ctx.run_pass(raw, |ctx| {
            ctx.root_ui(|ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let frame = drive_egui_touch_input_inner(
                        ctx,
                        TouchSurface::MainGrid,
                        geometry.clone(),
                        9,
                        true,
                    );
                    let mut response = ui.allocate_rect(ui.max_rect(), egui::Sense::click());
                    response.interact_pointer_pos_or_nan = pos2(500.0, 400.0);
                    assert!(frame.should_suppress_response(&response));
                    assert_eq!(frame.clone().into_commands().len(), 1);

                    let repeated = drive_egui_touch_input_inner(
                        ctx,
                        TouchSurface::MainGrid,
                        geometry.clone(),
                        9,
                        true,
                    );
                    assert!(repeated.commands().is_empty());
                    assert!(repeated.should_suppress_response(&response));
                    assert_eq!(
                        repeated
                            .correlated_primary_release_positions()
                            .collect::<Vec<_>>(),
                        vec![pos2(500.0, 400.0)]
                    );
                    let conflicting = drive_egui_touch_input_inner(
                        ctx,
                        TouchSurface::StillFullscreen,
                        geometry.clone(),
                        9,
                        true,
                    );
                    assert!(conflicting.commands().is_empty());
                })
            });
        })
        .drop_without_applying_deltas();
    }

    #[test]
    fn context_driver_observes_disabled_start_and_enabled_end() {
        let ctx = egui::Context::default();
        let geometry = geometry();
        let pos = pos2(500.0, 400.0);
        let mut disabled_frame = None;
        ctx.run_pass(
            egui::RawInput {
                screen_rect: Some(geometry.surface),
                time: Some(0.1),
                events: vec![
                    touch(1, egui::TouchPhase::Start, pos),
                    Event::PointerMoved(pos),
                    primary(pos, true),
                ],
                ..Default::default()
            },
            |ctx| {
                disabled_frame = Some(drive_egui_touch_input_inner(
                    ctx,
                    TouchSurface::StillFullscreen,
                    geometry.clone(),
                    20,
                    false,
                ));
            },
        )
        .drop_without_applying_deltas();
        let disabled_frame = disabled_frame.unwrap();
        assert!(disabled_frame.commands().is_empty());
        assert!(disabled_frame.has_touch_derived_pointer_activity());
        assert!(!disabled_frame.should_suppress_primary(pos, true));
        let disabled_state = ctx.data(|data| {
            data.get_temp::<TouchCorrelationState>(state_id(
                egui::ViewportId::ROOT,
                TouchSurface::StillFullscreen,
            ))
            .unwrap()
        });
        assert_eq!(disabled_state.pointer_touch, Some(TouchId(1)));
        assert_eq!(disabled_state.recognizer().contact_count(), 1);

        let mut enabled_frame = None;
        ctx.run_pass(
            egui::RawInput {
                screen_rect: Some(geometry.surface),
                time: Some(0.2),
                events: vec![
                    touch(1, egui::TouchPhase::End, pos),
                    primary(pos, false),
                    Event::PointerGone,
                ],
                ..Default::default()
            },
            |ctx| {
                enabled_frame = Some(drive_egui_touch_input_inner(
                    ctx,
                    TouchSurface::StillFullscreen,
                    geometry.clone(),
                    21,
                    true,
                ));
            },
        )
        .drop_without_applying_deltas();
        let enabled_frame = enabled_frame.unwrap();
        assert_eq!(enabled_frame.commands(), &[TouchCommand::ToggleChrome]);
        assert!(enabled_frame.should_suppress_primary(pos, false));
        let enabled_state = ctx.data(|data| {
            data.get_temp::<TouchCorrelationState>(state_id(
                egui::ViewportId::ROOT,
                TouchSurface::StillFullscreen,
            ))
            .unwrap()
        });
        assert!(enabled_state.pointer_touch.is_none());
        assert_eq!(enabled_state.recognizer().contact_count(), 0);
        assert!(enabled_state.pending.is_none());
    }

    #[test]
    fn commands_are_delivered_once_per_app_frame_and_resume_next_frame() {
        let ctx = egui::Context::default();
        let geometry = geometry();
        let center = pos2(500.0, 400.0);
        let (first, repeated) =
            drive_twice_in_app_frame(&ctx, &geometry, 20, 0.1, tap_events(1, center));
        let toggle_count = first
            .commands()
            .iter()
            .chain(repeated.commands())
            .filter(|command| **command == TouchCommand::ToggleChrome)
            .count();
        let mut chrome_visible = false;
        for _ in 0..toggle_count {
            chrome_visible = !chrome_visible;
        }
        assert_eq!(toggle_count, 1);
        assert!(chrome_visible);
        assert!(repeated.commands().is_empty());
        assert!(repeated.should_suppress_primary(center, false));

        let left = pos2(100.0, 400.0);
        let (next, next_repeated) =
            drive_twice_in_app_frame(&ctx, &geometry, 21, 0.2, tap_events(2, left));
        let page_moves = next
            .commands()
            .iter()
            .chain(next_repeated.commands())
            .filter(|command| **command == TouchCommand::PageSide { left: true })
            .count();
        assert_eq!(page_moves, 1);
        assert!(next_repeated.commands().is_empty());
        assert!(next_repeated.should_suppress_primary(left, false));
    }

    #[test]
    fn pinch_is_normal_when_secondary_contact_ends_first() {
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
        assert!(
            frame
                .commands()
                .iter()
                .any(|command| matches!(command, TouchCommand::Zoom { .. }))
        );
        assert!(
            frame
                .commands()
                .iter()
                .any(|command| matches!(command, TouchCommand::Pan { .. }))
        );
        assert_eq!(frame.commands().last(), Some(&TouchCommand::PinchEnd));
        assert!(frame.should_suppress_primary(first, false));
        assert!(!state.recognizer().is_active());
        assert!(state.pending.is_none());
        assert!(state.pointer_touch.is_none());
    }

    #[test]
    fn pinch_is_normal_when_pointer_contact_ends_first() {
        let mut state = TouchCorrelationState::default();
        let first = pos2(200.0, 400.0);
        let second = pos2(800.0, 400.0);
        let pinched = pos2(850.0, 400.0);
        let remaining = pos2(900.0, 400.0);
        let events = vec![
            touch(1, egui::TouchPhase::Start, first),
            Event::PointerMoved(first),
            primary(first, true),
            touch(2, egui::TouchPhase::Start, second),
            touch(2, egui::TouchPhase::Move, pinched),
            touch(1, egui::TouchPhase::End, first),
            primary(first, false),
            Event::PointerGone,
            touch(2, egui::TouchPhase::Move, remaining),
            Event::PointerMoved(remaining),
            touch(2, egui::TouchPhase::End, remaining),
            primary(remaining, false),
            Event::PointerGone,
        ];

        let frame = process(&mut state, &events, 100);
        assert!(
            frame
                .commands()
                .iter()
                .any(|command| matches!(command, TouchCommand::Zoom { .. }))
        );
        assert!(
            frame
                .commands()
                .iter()
                .any(|command| matches!(command, TouchCommand::Pan { .. }))
        );
        assert_eq!(frame.commands().last(), Some(&TouchCommand::PinchEnd));
        assert!(frame.should_suppress_primary(first, false));
        assert!(frame.should_suppress_primary(remaining, false));
        assert!(!state.recognizer().is_active());
        assert!(state.pending.is_none());
        assert!(state.pointer_touch.is_none());
    }

    #[test]
    fn previous_pinch_tail_and_next_pinch_start_can_share_one_frame() {
        let mut state = TouchCorrelationState::default();
        let first = pos2(200.0, 400.0);
        let second = pos2(800.0, 400.0);
        let pinched = pos2(850.0, 400.0);
        let previous = vec![
            touch(1, egui::TouchPhase::Start, first),
            Event::PointerMoved(first),
            primary(first, true),
            touch(2, egui::TouchPhase::Start, second),
            touch(2, egui::TouchPhase::Move, pinched),
            touch(1, egui::TouchPhase::End, first),
            primary(first, false),
            Event::PointerGone,
        ];
        let previous_frame = process(&mut state, &previous, 100);
        assert!(
            previous_frame
                .commands()
                .iter()
                .any(|command| matches!(command, TouchCommand::Zoom { .. }))
        );
        assert!(state.recognizer().is_active());
        assert!(state.pointer_touch.is_none());

        let remaining = pos2(900.0, 400.0);
        let next_first = pos2(250.0, 400.0);
        let next_second = pos2(750.0, 400.0);
        let next_pinched = pos2(800.0, 400.0);
        let shared_frame = vec![
            touch(2, egui::TouchPhase::Move, remaining),
            Event::PointerMoved(remaining),
            touch(2, egui::TouchPhase::End, remaining),
            primary(remaining, false),
            Event::PointerGone,
            touch(3, egui::TouchPhase::Start, next_first),
            Event::PointerMoved(next_first),
            primary(next_first, true),
            touch(4, egui::TouchPhase::Start, next_second),
            touch(4, egui::TouchPhase::Move, next_pinched),
        ];
        let (frame, diagnostics) =
            state.process_frame_with_diagnostics(&geometry(), &shared_frame, 200, true, true);

        assert!(!diagnostics.unwrap().ambiguous());
        assert!(!frame.touch_cancelled());
        assert!(frame.commands().contains(&TouchCommand::PinchEnd));
        assert!(
            frame
                .commands()
                .iter()
                .any(|command| matches!(command, TouchCommand::Zoom { .. }))
        );
        assert!(
            frame
                .commands()
                .iter()
                .any(|command| matches!(command, TouchCommand::Pan { .. }))
        );
        assert_eq!(state.recognizer().owner(), TouchOwner::Pinch);
        assert!(state.recognizer().is_active());
        assert_eq!(state.pointer_touch, Some(TouchId(3)));
        assert!(state.pending.is_none());
    }

    fn grid_geometry() -> TapZoneGeometry {
        TapZoneGeometry {
            behavior: crate::touch_input::TouchSurfaceBehavior::Grid,
            ..geometry()
        }
    }

    fn scoped_grid_frame(
        ctx: &egui::Context,
        scope: egui::Id,
        events: Vec<Event>,
        time: f64,
    ) -> TouchFrame {
        let mut result = None;
        ctx.run_pass(
            egui::RawInput {
                screen_rect: Some(grid_geometry().surface),
                events,
                time: Some(time),
                ..Default::default()
            },
            |ctx| {
                result = Some(drive_egui_grid_touch_input(
                    ctx,
                    grid_geometry(),
                    scope,
                    true,
                ));
            },
        )
        .drop_without_applying_deltas();
        result.unwrap()
    }

    fn grid_start(id: u64, pos: Pos2) -> Vec<Event> {
        vec![
            touch(id, egui::TouchPhase::Start, pos),
            Event::PointerMoved(pos),
            primary(pos, true),
        ]
    }

    #[test]
    fn grid_y_owner_covers_current_tap_lifetime_but_not_retired_or_mouse_input() {
        let ctx = egui::Context::default();
        let scope = egui::Id::unique("tap-owner");
        let other = scope.with("other");
        let start = pos2(500.0, 400.0);
        let first = scoped_grid_frame(&ctx, scope, grid_start(1, start), 0.1);
        assert!(first.grid_owns_native_y(scope));
        assert!(!first.grid_owns_native_y(other));
        let held = scoped_grid_frame(&ctx, scope, vec![], 0.2);
        assert!(held.grid_owns_native_y(scope));
        let moved = start + egui::vec2(0.0, 3.0);
        let candidate = scoped_grid_frame(
            &ctx,
            scope,
            vec![
                touch(1, egui::TouchPhase::Move, moved),
                Event::PointerMoved(moved),
            ],
            0.3,
        );
        assert_eq!(candidate.owner(), TouchOwner::Undecided);
        assert!(candidate.commands().is_empty());
        assert!(candidate.grid_owns_native_y(scope));
        let end = scoped_grid_frame(
            &ctx,
            scope,
            vec![
                touch(1, egui::TouchPhase::End, moved),
                primary(moved, false),
                Event::PointerGone,
            ],
            0.31,
        );
        assert!(end.accepts_grid_tap_release(moved));
        assert!(end.grid_owns_native_y(scope));
        assert!(!scoped_grid_frame(&ctx, scope, vec![], 0.4).grid_owns_native_y(scope));

        let _ = scoped_grid_frame(&ctx, scope, grid_start(2, start), 0.5);
        let retired = scoped_grid_frame(
            &ctx,
            other,
            vec![
                touch(2, egui::TouchPhase::Move, moved),
                Event::PointerMoved(moved),
            ],
            0.6,
        );
        assert!(retired.has_touch_derived_pointer_activity());
        assert!(!retired.grid_owns_native_y(other));
        let new = pos2(700.0, 400.0);
        let current = scoped_grid_frame(
            &ctx,
            other,
            vec![touch(3, egui::TouchPhase::Start, new)],
            0.7,
        );
        assert!(
            current.grid_owns_native_y(other),
            "new raw contact can own Y while old pointer gate remains occupied"
        );
        let old_tail = scoped_grid_frame(
            &ctx,
            other,
            vec![
                touch(2, egui::TouchPhase::Move, start),
                Event::PointerMoved(start),
            ],
            0.8,
        );
        assert!(
            old_tail.grid_owns_native_y(other),
            "the current contact still owns Y despite retired pointer provenance"
        );
        let mouse = scoped_grid_frame(&ctx, other, mouse_click(start), 0.9);
        assert!(mouse.is_mouse_primary(start, false));
        assert!(!mouse.grid_owns_native_y(other));
    }

    #[test]
    fn grid_session_accepts_new_contact_while_old_held_and_unknown_cancel_cannot_end_it() {
        let ctx = egui::Context::default();
        let old_scope = egui::Id::unique("old-list");
        let scope = egui::Id::unique("new-list");
        let old = pos2(200.0, 400.0);
        let new = pos2(600.0, 400.0);
        let _ = scoped_grid_frame(&ctx, old_scope, grid_start(1, old), 0.1);
        let scrolled = scoped_grid_frame(
            &ctx,
            old_scope,
            vec![
                touch(1, egui::TouchPhase::Move, pos2(200.0, 370.0)),
                Event::PointerMoved(pos2(200.0, 370.0)),
            ],
            0.2,
        );
        assert!(
            scrolled
                .grid_commands(old_scope)
                .iter()
                .any(|command| matches!(command, TouchCommand::ScrollGrid { .. }))
        );
        // egui-winit's pointer gate is still occupied by contact 1, so neither
        // event for the new contact has a synthetic pointer tail.
        let new_scroll = scoped_grid_frame(
            &ctx,
            scope,
            vec![
                touch(2, egui::TouchPhase::Start, new),
                touch(2, egui::TouchPhase::Move, pos2(600.0, 375.0)),
            ],
            0.3,
        );
        assert!(
            new_scroll
                .grid_commands(scope)
                .iter()
                .any(|command| matches!(command, TouchCommand::ScrollGrid { .. }))
        );
        assert_eq!(new_scroll.owner(), TouchOwner::GridScroll);
        assert!(new_scroll.grid_commands(old_scope).is_empty());
        let after_cancel = scoped_grid_frame(
            &ctx,
            scope,
            vec![
                touch(1, egui::TouchPhase::Cancel, old),
                Event::PointerGone,
                touch(2, egui::TouchPhase::Move, pos2(600.0, 350.0)),
                Event::PointerMoved(pos2(600.0, 350.0)),
            ],
            0.4,
        );
        assert!(!after_cancel.touch_cancelled());
        assert!(after_cancel.is_active());
        assert_eq!(
            after_cancel.grid_commands(scope),
            &[TouchCommand::ScrollGrid { delta_y: -25.0 }]
        );
        let retired_end = scoped_grid_frame(
            &ctx,
            scope,
            vec![
                touch(1, egui::TouchPhase::End, old),
                primary(old, false),
                Event::PointerGone,
            ],
            0.5,
        );
        assert!(retired_end.grid_commands(scope).is_empty());
        assert!(retired_end.should_suppress_primary(old, false));
        assert!(!retired_end.accepts_grid_tap_release(old));
        assert!(retired_end.is_active());
    }

    #[test]
    fn grid_session_retires_pinch_tail_without_zoom_end_or_tap_commands() {
        let ctx = egui::Context::default();
        let old_scope = egui::Id::unique("old-pinch");
        let scope = egui::Id::unique("new-pinch");
        let first = pos2(200.0, 400.0);
        let second = pos2(800.0, 400.0);
        let mut events = grid_start(1, first);
        events.extend([
            touch(2, egui::TouchPhase::Start, second),
            touch(2, egui::TouchPhase::Move, pos2(900.0, 400.0)),
        ]);
        let pinch = scoped_grid_frame(&ctx, old_scope, events, 0.1);
        assert!(
            pinch
                .grid_commands(old_scope)
                .iter()
                .any(|command| matches!(command, TouchCommand::Zoom { .. }))
        );
        assert!(pinch.grid_commands(scope).is_empty());
        let retired = scoped_grid_frame(
            &ctx,
            scope,
            vec![
                touch(2, egui::TouchPhase::Move, pos2(950.0, 400.0)),
                touch(1, egui::TouchPhase::End, first),
                primary(first, false),
                Event::PointerGone,
                touch(2, egui::TouchPhase::End, second),
                primary(second, false),
                Event::PointerGone,
            ],
            0.2,
        );
        assert!(retired.commands().is_empty());
        assert!(!retired.is_active());
        assert!(retired.should_suppress_primary(first, false));
        assert!(retired.should_suppress_primary(second, false));
    }

    #[test]
    fn grid_scope_retires_pending_signature_without_resetting_transport_gate() {
        let mut state = TouchCorrelationState::default();
        let old_scope = egui::Id::unique("pending-old");
        let scope = egui::Id::unique("pending-new");
        let pos = pos2(500.0, 400.0);
        state.select_scope(CommandScope::MainGrid(old_scope));
        let _ = state.process_frame(&grid_geometry(), &grid_start(1, pos), 100, true);
        let _ = state.process_frame(
            &grid_geometry(),
            &[touch(1, egui::TouchPhase::End, pos)],
            200,
            true,
        );
        state.select_scope(CommandScope::MainGrid(scope));
        let tail = state.process_frame(
            &grid_geometry(),
            &[primary(pos, false), Event::PointerGone],
            201,
            true,
        );
        assert!(tail.grid_commands(scope).is_empty());
        assert!(tail.should_suppress_primary(pos, false));
        assert!(!tail.accepts_grid_tap_release(pos));
        assert!(state.pending.is_none());
        assert!(state.pointer_touch.is_none());
    }

    #[test]
    fn grid_retired_move_keeps_touch_provenance_and_real_mouse_remains_unsuppressed() {
        let ctx = egui::Context::default();
        let old_scope = egui::Id::unique("move-old");
        let scope = egui::Id::unique("move-new");
        let pos = pos2(500.0, 400.0);
        let _ = scoped_grid_frame(&ctx, old_scope, grid_start(1, pos), 0.1);
        let moved = pos2(500.0, 380.0);
        let retired = scoped_grid_frame(
            &ctx,
            scope,
            vec![
                touch(1, egui::TouchPhase::Move, moved),
                Event::PointerMoved(moved),
            ],
            0.2,
        );
        assert!(retired.has_touch_derived_pointer_activity());
        assert!(retired.commands().is_empty());
        assert_eq!(retired.correlated_primary_release_positions().count(), 0);
        let mouse = pos2(50.0, 50.0);
        let mut events = vec![
            touch(1, egui::TouchPhase::End, moved),
            primary(moved, false),
            Event::PointerGone,
        ];
        events.extend(mouse_click(mouse));
        let mixed = scoped_grid_frame(&ctx, scope, events, 0.3);
        assert!(mixed.should_suppress_primary(moved, false));
        assert!(!mixed.should_suppress_primary(mouse, false));
    }

    #[test]
    fn grid_same_viewport_frame_scope_change_replays_retired_input_without_restart() {
        let ctx = egui::Context::default();
        let old_scope = egui::Id::unique("replay-old");
        let scope = egui::Id::unique("replay-new");
        let pos = pos2(500.0, 400.0);
        ctx.run_pass(
            egui::RawInput {
                events: grid_start(1, pos),
                ..Default::default()
            },
            |ctx| {
                let original = drive_egui_grid_touch_input(ctx, grid_geometry(), old_scope, true);
                assert!(original.is_active());
                let retired = drive_egui_grid_touch_input(ctx, grid_geometry(), scope, true);
                assert!(!retired.is_active());
                assert!(retired.grid_commands(scope).is_empty());
                assert!(retired.should_suppress_primary(pos, true));
                let back = drive_egui_grid_touch_input(ctx, grid_geometry(), old_scope, true);
                assert!(!back.is_active());
            },
        )
        .drop_without_applying_deltas();
        let next = scoped_grid_frame(
            &ctx,
            scope,
            vec![
                touch(2, egui::TouchPhase::Start, pos),
                touch(2, egui::TouchPhase::Move, pos2(500.0, 370.0)),
            ],
            0.2,
        );
        assert!(!next.grid_commands(scope).is_empty());
    }

    #[test]
    fn grid_transport_only_end_pass_observes_parked_tail_and_resume_accepts_new_start() {
        let ctx = egui::Context::default();
        install_main_grid_touch_transport_observer(&ctx);
        let old_scope = egui::Id::unique("park-old");
        let scope = egui::Id::unique("park-new");
        let pos = pos2(500.0, 400.0);
        let _ = scoped_grid_frame(&ctx, old_scope, grid_start(1, pos), 0.1);
        // No MainGrid drive: the actual pass-end callback owns this observation.
        ctx.run_pass(
            egui::RawInput {
                events: vec![
                    touch(1, egui::TouchPhase::End, pos),
                    primary(pos, false),
                    Event::PointerGone,
                ],
                ..Default::default()
            },
            |_| {},
        )
        .drop_without_applying_deltas();
        let state = ctx.data(|data| {
            data.get_temp::<TouchCorrelationState>(state_id(
                egui::ViewportId::ROOT,
                TouchSurface::MainGrid,
            ))
            .unwrap()
        });
        assert_eq!(state.recognition.scope(), CommandScope::Retired);
        assert!(state.pointer_touch.is_none());
        assert!(state.pending.is_none());
        let mut events = grid_start(2, pos);
        events.extend([
            touch(2, egui::TouchPhase::Move, pos2(500.0, 370.0)),
            Event::PointerMoved(pos2(500.0, 370.0)),
        ]);
        assert!(
            !scoped_grid_frame(&ctx, scope, events, 0.3)
                .grid_commands(scope)
                .is_empty()
        );
    }

    #[test]
    fn grid_transport_only_replay_does_not_restart_and_keeps_still_command_marker() {
        let ctx = egui::Context::default();
        let pos = pos2(500.0, 400.0);
        let scope = egui::Id::unique("transport-replay");
        ctx.run_pass(
            egui::RawInput {
                events: tap_events(1, pos),
                ..Default::default()
            },
            |ctx| {
                observe_main_grid_transport(ctx);
                let replay = drive_egui_grid_touch_input(ctx, grid_geometry(), scope, true);
                assert!(replay.commands().is_empty());
                assert!(replay.should_suppress_primary(pos, false));
                assert!(!replay.accepts_grid_tap_release(pos));
            },
        )
        .drop_without_applying_deltas();
        ctx.run_pass(
            egui::RawInput {
                events: tap_events(2, pos),
                ..Default::default()
            },
            |ctx| {
                let first = drive_egui_touch_input_inner(
                    ctx,
                    TouchSurface::StillFullscreen,
                    geometry(),
                    ctx.cumulative_frame_nr(),
                    true,
                );
                assert_eq!(first.commands(), &[TouchCommand::ToggleChrome]);
                observe_main_grid_transport(ctx);
                let replay = drive_egui_touch_input_inner(
                    ctx,
                    TouchSurface::StillFullscreen,
                    geometry(),
                    ctx.cumulative_frame_nr(),
                    true,
                );
                assert!(replay.commands().is_empty());
                assert!(replay.should_suppress_primary(pos, false));
                assert_eq!(replay.owner(), first.owner());
            },
        )
        .drop_without_applying_deltas();
    }

    #[test]
    fn grid_retired_release_then_same_position_mouse_click_uses_the_later_mouse_edge() {
        let ctx = egui::Context::default();
        let old_scope = egui::Id::unique("same-position-old");
        let scope = egui::Id::unique("same-position-new");
        let pos = pos2(500.0, 400.0);
        let _ = scoped_grid_frame(&ctx, old_scope, grid_start(1, pos), 0.1);
        let mut events = vec![
            touch(1, egui::TouchPhase::End, pos),
            primary(pos, false),
            Event::PointerGone,
        ];
        events.extend(mouse_click(pos));
        let frame = scoped_grid_frame(&ctx, scope, events, 0.2);
        assert!(frame.commands().is_empty());
        assert!(!frame.is_touch_primary(pos, true));
        assert!(!frame.is_touch_primary(pos, false));
        assert!(frame.is_mouse_primary(pos, true));
        assert!(frame.is_mouse_primary(pos, false));
        assert!(!frame.should_suppress_primary(pos, false));
        assert!(!frame.has_touch_derived_pointer_activity());
        assert_eq!(frame.correlated_primary_release_positions().count(), 0);
    }

    #[test]
    fn grid_retired_move_then_same_position_mouse_drag_uses_the_current_mouse_stream() {
        for end_old_contact in [false, true] {
            let ctx = egui::Context::default();
            let old_scope = egui::Id::unique("same-drag-old");
            let scope = egui::Id::unique("same-drag-new");
            let pos = pos2(500.0, 400.0);
            let _ = scoped_grid_frame(&ctx, old_scope, grid_start(1, pos), 0.1);
            let mut events = if end_old_contact {
                vec![
                    touch(1, egui::TouchPhase::End, pos),
                    primary(pos, false),
                    Event::PointerGone,
                ]
            } else {
                vec![
                    touch(1, egui::TouchPhase::Move, pos),
                    Event::PointerMoved(pos),
                ]
            };
            events.extend([primary(pos, true), Event::PointerMoved(pos2(500.0, 450.0))]);
            let frame = scoped_grid_frame(&ctx, scope, events, 0.2);
            assert!(frame.commands().is_empty());
            assert!(frame.is_mouse_primary(pos, true));
            assert!(!frame.has_touch_derived_pointer_activity());
            // Starting a real drag does not relabel the earlier retired
            // release as a mouse click before a real mouse release arrives.
            assert_eq!(frame.should_suppress_primary(pos, false), end_old_contact);
        }
    }

    #[test]
    fn grid_mixed_mouse_edge_keeps_backend_touch_gate_for_new_raw_contact() {
        let ctx = egui::Context::default();
        let old_scope = egui::Id::unique("mixed-held-old");
        let scope = egui::Id::unique("mixed-held-new");
        let old = pos2(200.0, 400.0);
        let new = pos2(600.0, 400.0);
        let _ = scoped_grid_frame(&ctx, old_scope, grid_start(1, old), 0.1);
        let mixed = scoped_grid_frame(
            &ctx,
            scope,
            vec![
                touch(1, egui::TouchPhase::Move, old),
                Event::PointerMoved(old),
                primary(old, true),
                Event::PointerMoved(pos2(200.0, 450.0)),
                touch(2, egui::TouchPhase::Start, new),
            ],
            0.2,
        );
        assert!(mixed.commands().is_empty());
        let state = ctx.data(|data| {
            data.get_temp::<TouchCorrelationState>(state_id(
                egui::ViewportId::ROOT,
                TouchSurface::MainGrid,
            ))
            .unwrap()
        });
        assert_eq!(state.pointer_touch, Some(TouchId(1)));
        assert!(state.pending.is_none());
        assert!(state.recognizer().has_contact(2));
        let next = scoped_grid_frame(
            &ctx,
            scope,
            vec![touch(2, egui::TouchPhase::Move, pos2(600.0, 370.0))],
            0.3,
        );
        assert_eq!(
            next.grid_commands(scope),
            &[TouchCommand::ScrollGrid { delta_y: -30.0 }]
        );
    }
}
