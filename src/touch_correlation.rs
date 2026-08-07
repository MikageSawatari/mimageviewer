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

/// Read-only classification produced for one app-frame drive.
///
/// Commands are returned only by the first drive for an app frame because
/// executing them mutates application state. Later egui passes receive the
/// same correlation and suppression answers with an empty command list.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TouchFrame {
    commands: Vec<TouchCommand>,
    primary_events: Vec<CorrelatedPrimary>,
    owner: TouchOwner,
    active: bool,
    touch_cancelled: bool,
}

impl TouchFrame {
    fn replay_for_later_passes(&self) -> Self {
        Self {
            commands: Vec::new(),
            primary_events: self.primary_events.clone(),
            owner: self.owner,
            active: self.active,
            touch_cancelled: self.touch_cancelled,
        }
    }

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

    /// Whether this drive contains an exactly correlated touch-derived
    /// primary stream. Grid cells use this provenance-only answer to disable
    /// native file D&D without changing mouse-only input.
    pub(crate) fn has_touch_derived_pointer_activity(&self) -> bool {
        self.active || !self.primary_events.is_empty() || self.touch_cancelled
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
            .filter(|event| !event.pressed)
            .map(|event| event.pos)
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
            PendingSignature::CancelGone => matches!(event, Event::PointerGone),
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
            owner: state.recognizer.owner(),
            contacts: state.recognizer.contact_count(),
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
struct TouchCorrelationState {
    recognizer: TouchRecognizer,
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
            recognizer: TouchRecognizer::new(),
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
        owner: TouchOwner,
        active: bool,
        after: CorrelationStateSnapshot,
    ) -> (TouchFrame, Option<CorrelationDiagnostics>) {
        if self.ambiguous {
            // Suppression is useful only together with a replacement command.
            // A mixed or malformed primary stream therefore falls back in
            // full: no touch command and no suppression recommendation.
            self.commands.clear();
            self.primary_events.clear();
        }
        if let Some(diagnostics) = &mut self.diagnostics {
            diagnostics.after = after;
        }
        (
            TouchFrame {
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
    #[cfg(test)]
    fn process_frame(
        &mut self,
        geometry: &TapZoneGeometry,
        events: &[Event],
        now_ms: u64,
        disabled: bool,
    ) -> TouchFrame {
        self.process_frame_with_diagnostics(geometry, events, now_ms, disabled, false)
            .0
    }

    fn process_frame_with_diagnostics(
        &mut self,
        geometry: &TapZoneGeometry,
        events: &[Event],
        now_ms: u64,
        disabled: bool,
        diagnostics_enabled: bool,
    ) -> (TouchFrame, Option<CorrelationDiagnostics>) {
        let mut frame = FrameBuilder::new(self, diagnostics_enabled);
        if disabled {
            frame.touch_cancelled = self.recognizer.is_active();
            *self = Self::default();
            return frame.finish(
                self.recognizer.owner(),
                self.recognizer.is_active(),
                CorrelationStateSnapshot::capture(self),
            );
        }

        for event in events {
            self.process_event(geometry, event, now_ms, &mut frame);
        }
        frame.finish(
            self.recognizer.owner(),
            self.recognizer.is_active(),
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
                frame.touch_cancelled |= *phase == egui::TouchPhase::Cancel;
                self.handle_touch(geometry, *id, *phase, *pos, now_ms, frame)
            }
            Event::PointerButton {
                button: PointerButton::Primary,
                ..
            } => {
                // A primary button without a pending exact touch signature is
                // existing mouse input. If touch input is also present in this
                // pass, fail closed and cancel all touch-side action.
                frame.ambiguous = true;
                frame.record_unmatched_primary(event);
                frame.record_cancel_stream_call();
                self.cancel_stream(geometry, now_ms);
            }
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
        let recognizer_was_active = self.recognizer.is_active();
        let commands =
            if matches!(phase, TouchPhase::Move | TouchPhase::End) && !recognizer_was_active {
                // Keep the dependency mirror exact for a stray tail, but never
                // reuse suppression retained by an already completed stream.
                Vec::new()
            } else {
                self.recognizer.handle_sample(geometry, sample)
            };
        let pending = PendingTouch {
            pos,
            commands,
            // This is sampled only after the raw touch event has updated the
            // recognizer. It is never consulted for an uncorrelated primary.
            should_suppress: recognizer_was_active && self.recognizer.should_suppress_primary(),
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
                repeated_drive_result(data, viewport, surface, marker, diagnostics_enabled);
            return (
                touch_frame,
                diagnostics,
                DiagnosticDriveKind::Replay,
                surface_match,
            );
        }
        data.insert_temp(marker_id, DrivenSurface { frame, surface });

        let (touch_frame, diagnostics) = process_surface_state(
            data,
            viewport,
            surface,
            &geometry,
            &events,
            now_ms,
            disabled,
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
    marker: DrivenSurface,
    diagnostics_enabled: bool,
) -> (TouchFrame, Option<CorrelationDiagnostics>, bool) {
    if marker.surface != surface {
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
    geometry: &TapZoneGeometry,
    events: &[Event],
    now_ms: u64,
    disabled: bool,
    diagnostics_enabled: bool,
) -> (TouchFrame, Option<CorrelationDiagnostics>) {
    let id = state_id(viewport, surface);
    let mut state = data
        .get_temp::<TouchCorrelationState>(id)
        .unwrap_or_default();
    let (frame, diagnostics) = state.process_frame_with_diagnostics(
        geometry,
        events,
        now_ms,
        disabled,
        diagnostics_enabled,
    );
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
        TouchCommand::OpenSidePanel { .. } => "OpenSidePanel",
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
        state.process_frame(&geometry(), events, now_ms, false)
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
        let _ = ctx.run(raw, |ctx| {
            frames = Some((
                drive_egui_touch_input_inner(
                    ctx,
                    TouchSurface::StillFullscreen,
                    geometry.clone(),
                    frame,
                    false,
                ),
                drive_egui_touch_input_inner(
                    ctx,
                    TouchSurface::StillFullscreen,
                    geometry.clone(),
                    frame,
                    false,
                ),
            ));
        });
        frames.unwrap()
    }

    #[test]
    fn correlation_diagnostic_line_carries_frame_state_and_command_kinds() {
        let mut state = TouchCorrelationState::default();
        let pos = pos2(500.0, 400.0);
        let (frame, diagnostics) = state.process_frame_with_diagnostics(
            &geometry(),
            &tap_events(1, pos),
            100,
            false,
            true,
        );
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
            state.process_frame_with_diagnostics(&geometry(), &events, 100, false, true);
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
        assert!(frame.primary_events.is_empty());
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
            false,
            true,
        );

        assert!(frame.commands().is_empty());
        assert!(frame.primary_events.is_empty());
        assert!(diagnostics.unwrap().ambiguous());
        assert_eq!(state.recognizer.owner(), TouchOwner::Cancelled);
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
    fn stray_touch_tail_tracks_the_dependency_gate_without_reusing_suppression() {
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
        assert!(cancel_frame.primary_events.is_empty());
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
            invalid_cancel.process_frame_with_diagnostics(&geometry(), &invalid, 301, false, true);
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
                    false,
                );
                assert!(conflicting.commands().is_empty());
            });
        });
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
        assert!(!state.recognizer.is_active());
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
        assert!(!state.recognizer.is_active());
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
        assert!(state.recognizer.is_active());
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
            state.process_frame_with_diagnostics(&geometry(), &shared_frame, 200, false, true);

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
        assert_eq!(state.recognizer.owner(), TouchOwner::Pinch);
        assert!(state.recognizer.is_active());
        assert_eq!(state.pointer_touch, Some(TouchId(3)));
        assert!(state.pending.is_none());
    }
}
