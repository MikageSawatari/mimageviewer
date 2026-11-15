//! One callback-owned client gesture for a parked still viewport.
//!
//! Recognition never changes viewer state. Only a completed press/release is sent
//! through the existing activation transport; its original input is not replayed.

use crate::key_input::{NativeClientBoundary, NativeClientInputBoundary, NativeInputCancelReason};
use crate::touch_correlation::{
    TouchSurface, drive_egui_touch_input_with_recognition_cancellations,
    preview_touch_primary_event_indices,
};
use crate::touch_input::{TapZoneGeometry, TouchCommand, TouchSurfaceBehavior};
use egui::{Event, PointerButton};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ClientActivationAvailability {
    #[default]
    Available,
    CommandOwned,
    Blocked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClientActivationSource {
    MouseClick,
    TouchTap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClientActivationCancelReason {
    ModalOwned,
    NotParked,
    CommandOwned,
    Native(NativeInputCancelReason),
    FocusLost,
    PointerGone,
    TouchCancelled,
    MultipleTouch,
    ButtonStateLost,
    Drag,
    OutsideClient,
    PointerTakeover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClientActivationDecision {
    Completed(ClientActivationSource),
    CompletedClose(ClientActivationSource),
    Cancelled {
        source: ClientActivationSource,
        reason: ClientActivationCancelReason,
    },
}

/// The operation's origin is captured when its native proof is consumed. It
/// never follows the published scope when a later context replaces that scope.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScopedClientActivationDecision {
    pub(crate) scope: Option<super::DetachedActivationRequest>,
    pub(crate) decision: ClientActivationDecision,
}

#[derive(Default)]
struct ScopedClientGesture {
    scope: Option<super::DetachedActivationRequest>,
    gesture: ClientGestureOwner,
}

#[derive(Clone, Copy, Debug, Default)]
enum ClientGestureOwner {
    #[default]
    Idle,
    Mouse(ClientGesture),
    Touch(ClientGesture, TouchContactEnd),
    SuppressedMouse(ClientActivationCancelReason, ClientOperation),
    SuppressedTouch(ClientActivationCancelReason, ClientOperation),
}

#[derive(Clone, Copy, Debug)]
struct ClientGesture {
    operation: ClientOperation,
    destination: ClientGestureDestination,
    press_pos: egui::Pos2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ClientGestureDestination {
    ActivateOnly,
    WindowClose(egui::Rect),
}

#[derive(Clone, Copy, Debug)]
enum TouchContactEnd {
    ContactDown,
    Released(egui::Pos2),
}

#[derive(Default)]
enum ClientFrameCompletion {
    #[default]
    None,
    Activate(ClientActivationSource),
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClientOperation {
    Canonical(egui::Id),
    LogicalFixture,
    Unowned,
}

#[derive(Clone, Copy, Debug)]
enum PressAdmission {
    Available,
    Cancelled(ClientActivationCancelReason),
}

#[derive(Clone, Debug)]
enum ControlBoundary {
    Native(NativeClientBoundary),
    ScopedNative {
        scope: super::DetachedActivationRequest,
        input: NativeClientInputBoundary,
        admission: PressAdmission,
    },
    Cancel(ClientActivationCancelReason),
    CancelAfterPressed(ClientActivationCancelReason),
    RetireScope,
}

#[derive(Default)]
pub(crate) struct DeferredClientActivationGesture {
    scope: Option<super::DetachedActivationRequest>,
    owner: ScopedClientGesture,
    frame: Option<(egui::ViewportId, u64)>,
    availability: ClientActivationAvailability,
    boundaries: Vec<ControlBoundary>,
    decisions: Vec<ScopedClientActivationDecision>,
}

impl DeferredClientActivationGesture {
    #[cfg(test)]
    pub(crate) fn scope(&self) -> Option<&super::DetachedActivationRequest> {
        self.scope.as_ref()
    }

    pub(crate) fn set_scope(&mut self, request: super::DetachedActivationRequest) {
        if self.scope.as_ref().is_some_and(|old| old != &request) {
            self.retire_scope_for(&request);
        } else {
            self.scope = Some(request);
        }
    }

    /// Feed the owning allocation's ordered native transcript before capture.
    /// Press/Release only align cancellation with raw input; they never activate.
    #[cfg(test)]
    pub(crate) fn feed_native_boundary(&mut self, boundary: NativeClientBoundary) {
        self.boundaries.push(ControlBoundary::Native(boundary));
        self.retain_press_admission(boundary);
    }

    /// Keep generation-time scope and modal admission in the same ordered
    /// transcript as native input, availability changes and scope retirement.
    pub(crate) fn feed_native_scoped_boundary(
        &mut self,
        scope: super::DetachedActivationRequest,
        input: NativeClientInputBoundary,
    ) {
        let admission = self
            .availability
            .cancel_reason()
            .map_or(PressAdmission::Available, PressAdmission::Cancelled);
        self.boundaries.push(ControlBoundary::ScopedNative {
            scope,
            input,
            admission,
        });
    }

    #[cfg(test)]
    fn retain_press_admission(&mut self, boundary: NativeClientBoundary) {
        if boundary == NativeClientBoundary::Pressed
            && let Some(reason) = self.availability.cancel_reason()
        {
            self.boundaries
                .push(ControlBoundary::CancelAfterPressed(reason));
        }
    }

    /// Called when publishing the shared view, including brief modal ownership
    /// between callbacks. Re-enabling cannot restore a cancelled old press.
    pub(crate) fn set_availability(&mut self, availability: ClientActivationAvailability) {
        if self.availability != availability
            && let Some(reason) = availability.cancel_reason()
        {
            // Retire only proof operations already observed when the modal
            // took ownership. A future proof receives its own fresh admission.
            for boundary in &mut self.boundaries {
                if let ControlBoundary::ScopedNative {
                    input, admission, ..
                } = boundary
                    && input.boundary == NativeClientBoundary::Pressed
                    && !matches!(
                        admission,
                        PressAdmission::Cancelled(ClientActivationCancelReason::Native(
                            NativeInputCancelReason::RetiredRegistration
                        ))
                    )
                {
                    if reason != ClientActivationCancelReason::CommandOwned
                        || !matches!(
                            admission,
                            PressAdmission::Cancelled(ClientActivationCancelReason::ModalOwned)
                        )
                    {
                        *admission = PressAdmission::Cancelled(reason);
                    }
                }
            }
            self.boundaries.push(ControlBoundary::Cancel(reason));
            self.cancel_owner(reason);
        }
        self.availability = availability;
    }

    /// Retire a captured old scope immediately. Unlike a native cancellation,
    /// this invalidates every remaining old batch edge, including a queued Up
    /// whose native Released metadata arrived before the scope changed.
    #[cfg(test)]
    pub(crate) fn retire_scope(&mut self) {
        self.cancel_owner(ClientActivationCancelReason::Native(
            NativeInputCancelReason::RetiredRegistration,
        ));
        self.boundaries
            .retain(|boundary| !matches!(boundary, ControlBoundary::ScopedNative { .. }));
        self.boundaries.push(ControlBoundary::RetireScope);
        self.scope = None;
    }

    /// Retire old authority, keeping its proof only as immutable origin metadata
    /// for a delayed raw batch. Returning to equal scope values cannot revive it.
    /// Proofs already generated by the replacement keep their own admission.
    pub(crate) fn retire_scope_for(&mut self, new_scope: &super::DetachedActivationRequest) {
        self.cancel_owner(ClientActivationCancelReason::Native(
            NativeInputCancelReason::RetiredRegistration,
        ));
        self.boundaries.retain_mut(|boundary| {
            let ControlBoundary::ScopedNative {
                scope,
                input,
                admission,
            } = boundary
            else {
                return false;
            };
            if scope != new_scope && input.boundary == NativeClientBoundary::Pressed {
                *admission = PressAdmission::Cancelled(ClientActivationCancelReason::Native(
                    NativeInputCancelReason::RetiredRegistration,
                ));
            }
            true
        });
        self.boundaries.insert(0, ControlBoundary::RetireScope);
        self.scope = Some(new_scope.clone());
    }

    pub(crate) fn drain_scoped_decisions(&mut self) -> Vec<ScopedClientActivationDecision> {
        std::mem::take(&mut self.decisions)
    }

    #[cfg(test)]
    pub(crate) fn drain_decisions(&mut self) -> Vec<ClientActivationDecision> {
        self.drain_scoped_decisions()
            .into_iter()
            .map(|entry| entry.decision)
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn capture(
        &mut self,
        ctx: &egui::Context,
        geometry: TapZoneGeometry,
        availability: ClientActivationAvailability,
    ) -> Option<ClientActivationSource> {
        self.capture_inner(ctx, geometry, availability, None, None)
    }

    #[cfg(test)]
    pub(crate) fn capture_for_scope(
        &mut self,
        ctx: &egui::Context,
        geometry: TapZoneGeometry,
        availability: ClientActivationAvailability,
        scope: &super::DetachedActivationRequest,
    ) -> Option<ClientActivationSource> {
        self.capture_inner(ctx, geometry, availability, Some(scope), None)
    }

    pub(crate) fn capture_for_scope_with_close(
        &mut self,
        ctx: &egui::Context,
        geometry: TapZoneGeometry,
        availability: ClientActivationAvailability,
        scope: &super::DetachedActivationRequest,
        close_rect: egui::Rect,
    ) -> Option<ClientActivationSource> {
        self.capture_inner(ctx, geometry, availability, Some(scope), Some(close_rect))
    }

    fn capture_inner(
        &mut self,
        ctx: &egui::Context,
        mut geometry: TapZoneGeometry,
        availability: ClientActivationAvailability,
        scope: Option<&super::DetachedActivationRequest>,
        close_rect: Option<egui::Rect>,
    ) -> Option<ClientActivationSource> {
        if let Some(scope) = scope {
            self.set_scope(scope.clone());
        }
        self.set_availability(availability);
        let frame = (ctx.viewport_id(), ctx.cumulative_frame_nr());
        if self.frame == Some(frame) {
            return None;
        }
        self.frame = Some(frame);
        let (events, releases, primary_down) = ctx.input(|input| {
            (
                input.raw.events.clone(),
                input
                    .pointer
                    .released_buttons_with_click_counts()
                    .filter_map(|(button, click)| {
                        (button == PointerButton::Primary).then_some(click)
                    })
                    .collect::<Vec<_>>(),
                input.pointer.primary_down(),
            )
        });
        let mouse_drag_distance = ctx.options(|options| options.input_options.max_click_dist);
        let promoted =
            preview_touch_primary_event_indices(ctx, TouchSurface::StillFullscreen, &geometry);
        let held_operation = self.current_operation();
        let ordered = ordered_cancellations(
            &events,
            std::mem::take(&mut self.boundaries),
            scope,
            &promoted,
            held_operation,
        );
        self.boundaries = ordered.retained;
        let require_proof = scoped_press_proof_required(scope);
        let mut cancellations = ordered.cancellations;
        if let Some(reason) = availability.cancel_reason() {
            cancellations.push((0, reason));
        }
        for (index, event) in events.iter().enumerate() {
            if matches!(event, Event::WindowFocused(false)) {
                cancellations.push((index, ClientActivationCancelReason::FocusLost));
            }
        }
        cancellations.retain(|(index, reason)| {
            *reason != ClientActivationCancelReason::CommandOwned
                || !((*index == 0 && self.owns_close())
                    || index.checked_sub(1).and_then(|start| events.get(start)).is_some_and(|event| {
                        matches!(event, Event::Touch { phase: egui::TouchPhase::Start, pos, .. }
                            | Event::PointerButton { button: PointerButton::Primary, pressed: true, pos, .. }
                            if close_rect.is_some_and(|rect| rect.contains(*pos)))
                    }))
        });
        cancellations.sort_by_key(|(index, _)| *index);
        let cancellation_indices = cancellations
            .iter()
            .map(|(index, _)| *index)
            .collect::<Vec<_>>();
        // Pinch recognition reserves every multi-contact gesture. Its transform
        // commands are observed, never executed on the parked snapshot.
        geometry.behavior = TouchSurfaceBehavior::Viewer {
            accepts_pinch: true,
            tap_zones: true,
        };
        let touch = drive_egui_touch_input_with_recognition_cancellations(
            ctx,
            TouchSurface::StillFullscreen,
            geometry.clone(),
            frame.1,
            availability == ClientActivationAvailability::Available
                || (availability == ClientActivationAvailability::CommandOwned
                    && close_rect.is_some()),
            &cancellation_indices,
        );
        let mut activation = ClientFrameCompletion::None;
        let mut release_ordinal = 0;
        let mut accepts_press = true;
        for (index, event) in events.iter().enumerate() {
            for (_, reason) in cancellations.iter().filter(|(at, _)| *at == index) {
                self.cancel_owner(*reason);
            }
            let proof = ordered
                .authorized_presses
                .iter()
                .find(|proof| proof.index == index);
            let operation = proof
                .map(|proof| ClientOperation::Canonical(proof.operation))
                .unwrap_or(if require_proof {
                    ClientOperation::Unowned
                } else {
                    ClientOperation::LogicalFixture
                });
            let origin = proof.map(|proof| proof.scope.clone()).or_else(|| {
                (operation == ClientOperation::LogicalFixture)
                    .then(|| scope.cloned())
                    .flatten()
            });
            match event {
                Event::WindowFocused(focused) => accepts_press = *focused,
                Event::Touch {
                    phase: egui::TouchPhase::Start,
                    pos,
                    ..
                } => {
                    let destination = destination_at(*pos, close_rect);
                    let continues_owned_gesture = touch.continues_gesture_at(index)
                        && matches!(
                            self.owner.gesture,
                            ClientGestureOwner::Touch(
                                ClientGesture {
                                    operation: ClientOperation::Canonical(_),
                                    ..
                                },
                                _
                            )
                        );
                    // A raw End may leave only its exact synthetic tail pending.
                    // A fresh contact owns a new operation and destination; it
                    // cannot complete that old press after a tail mismatch.
                    if !continues_owned_gesture {
                        self.cancel_owner(ClientActivationCancelReason::PointerTakeover);
                    }
                    if require_proof
                        && operation == ClientOperation::Unowned
                        && !continues_owned_gesture
                    {
                        self.cancel_owner(ClientActivationCancelReason::NotParked);
                        self.owner = ScopedClientGesture {
                            scope: origin,
                            gesture: ClientGestureOwner::SuppressedTouch(
                                ClientActivationCancelReason::NotParked,
                                operation,
                            ),
                        };
                    } else if matches!(self.owner.gesture, ClientGestureOwner::Idle) {
                        let gesture = match admission(
                            availability,
                            accepts_press,
                            &geometry,
                            *pos,
                            start_scope_proven(require_proof, operation, destination),
                            destination,
                        ) {
                            Ok(()) => ClientGestureOwner::Touch(
                                ClientGesture {
                                    operation,
                                    destination,
                                    press_pos: *pos,
                                },
                                TouchContactEnd::ContactDown,
                            ),
                            Err(reason) => ClientGestureOwner::SuppressedTouch(reason, operation),
                        };
                        self.owner = ScopedClientGesture {
                            scope: origin,
                            gesture,
                        };
                    }
                }
                Event::Touch {
                    phase: egui::TouchPhase::End,
                    ..
                } if matches!(self.owner.gesture, ClientGestureOwner::SuppressedTouch(..)) => {
                    if let ClientGestureOwner::SuppressedTouch(reason, _) = self.owner.gesture {
                        self.cancel_owner(reason);
                    }
                }
                Event::Touch {
                    phase: egui::TouchPhase::End,
                    pos,
                    ..
                } => {
                    if let ClientGestureOwner::Touch(gesture, _) = self.owner.gesture {
                        self.owner.gesture =
                            ClientGestureOwner::Touch(gesture, TouchContactEnd::Released(*pos));
                    }
                }
                Event::Touch {
                    phase: egui::TouchPhase::Cancel,
                    ..
                } => {
                    self.cancel_owner(ClientActivationCancelReason::TouchCancelled);
                }
                Event::PointerButton {
                    button: PointerButton::Primary,
                    pressed: true,
                    pos,
                    ..
                } => {
                    let destination = destination_at(*pos, close_rect);
                    let edge = touch.grid_primary_edge(release_ordinal, true);
                    if !edge.touch_derived {
                        // Positive mouse correlation permits independent physical
                        // takeover; unknown touch edges never become mouse clicks.
                        let touch_owned = matches!(
                            self.owner.gesture,
                            ClientGestureOwner::Touch(..) | ClientGestureOwner::SuppressedTouch(..)
                        );
                        if !touch_owned || edge.ordinary_mouse {
                            self.cancel_owner(ClientActivationCancelReason::PointerTakeover);
                            let gesture = match admission(
                                availability,
                                accepts_press,
                                &geometry,
                                *pos,
                                start_scope_proven(require_proof, operation, destination),
                                destination,
                            ) {
                                Ok(()) => ClientGestureOwner::Mouse(ClientGesture {
                                    operation,
                                    destination,
                                    press_pos: *pos,
                                }),
                                Err(reason) => {
                                    ClientGestureOwner::SuppressedMouse(reason, operation)
                                }
                            };
                            self.owner = ScopedClientGesture {
                                scope: origin,
                                gesture,
                            };
                        }
                    }
                }
                Event::PointerButton {
                    button: PointerButton::Primary,
                    pressed: false,
                    pos,
                    ..
                } => {
                    let edge = touch.grid_primary_edge(release_ordinal, false);
                    if !edge.touch_derived {
                        match self.owner.gesture {
                            ClientGestureOwner::Mouse(gesture) => {
                                // Resuming a parked owner needs a completed
                                // stationary press, independent of its duration.
                                // Window close retains the ordinary click policy.
                                let completed = match gesture.destination {
                                    ClientGestureDestination::ActivateOnly => {
                                        !mouse_press_became_drag(
                                            gesture.press_pos,
                                            *pos,
                                            mouse_drag_distance,
                                        )
                                    }
                                    ClientGestureDestination::WindowClose(_) => {
                                        releases.get(release_ordinal).is_some_and(Option::is_some)
                                    }
                                };
                                if completed
                                    && release_inside(
                                        gesture.destination,
                                        &geometry,
                                        *pos,
                                        close_rect,
                                    )
                                {
                                    self.complete(
                                        ClientActivationSource::MouseClick,
                                        &mut activation,
                                    );
                                } else {
                                    self.cancel_owner(ClientActivationCancelReason::Drag);
                                }
                            }
                            ClientGestureOwner::SuppressedMouse(reason, _) => {
                                self.cancel_owner(reason)
                            }
                            _ => {}
                        }
                    } else if let ClientGestureOwner::SuppressedTouch(reason, _) =
                        self.owner.gesture
                    {
                        self.cancel_owner(reason);
                    }
                    release_ordinal += 1;
                }
                Event::PointerMoved(pos) => {
                    if let ClientGestureOwner::Mouse(gesture) = self.owner.gesture
                        && mouse_press_became_drag(gesture.press_pos, *pos, mouse_drag_distance)
                    {
                        self.cancel_owner(ClientActivationCancelReason::Drag);
                    }
                }
                _ => {}
            }
            // A canonical owner may consume its own completed input fact even
            // when an unrelated older mouse event forbids generic touch commands.
            // Logical fixtures continue to use the unchanged command permission.
            let owned_tap_completed = match self.owner.gesture {
                ClientGestureOwner::Touch(
                    ClientGesture {
                        operation: ClientOperation::Canonical(_),
                        ..
                    },
                    _,
                ) => {
                    touch.completed_tap_at(index)
                        || (self.owns_activation() && touch.completed_stationary_press_at(index))
                }
                ClientGestureOwner::Touch(
                    ClientGesture {
                        operation: ClientOperation::LogicalFixture,
                        ..
                    },
                    _,
                ) => touch.commands_with_event_indices().any(|(at, command)| {
                    at == index
                        && matches!(
                            command,
                            TouchCommand::ToggleChrome | TouchCommand::PageSide { .. }
                        )
                }),
                _ => false,
            };
            if owned_tap_completed {
                let release_inside = match self.owner.gesture {
                    ClientGestureOwner::Touch(gesture, TouchContactEnd::Released(pos)) => {
                        release_inside(gesture.destination, &geometry, pos, close_rect)
                    }
                    _ => false,
                };
                if release_inside {
                    self.complete(ClientActivationSource::TouchTap, &mut activation);
                } else {
                    self.cancel_owner(ClientActivationCancelReason::OutsideClient);
                }
            }
            if touch
                .commands_with_event_indices()
                .any(|(at, command)| at == index && command == TouchCommand::PinchEnd)
            {
                self.cancel_owner(ClientActivationCancelReason::MultipleTouch);
            }
            if matches!(self.owner.gesture, ClientGestureOwner::Touch(..)) {
                match touch.ended_gesture_at(index) {
                    Some(crate::touch_input::TouchOwner::ViewerPointerPassthrough) => {
                        self.cancel_owner(ClientActivationCancelReason::Drag)
                    }
                    Some(crate::touch_input::TouchOwner::Undecided) => {
                        if !(self.owns_activation() && touch.stationary_press_ended_at(index)) {
                            self.cancel_owner(ClientActivationCancelReason::TouchCancelled);
                        }
                    }
                    Some(crate::touch_input::TouchOwner::Pinch) => {
                        self.cancel_owner(ClientActivationCancelReason::MultipleTouch)
                    }
                    _ => {}
                }
            }
            if matches!(event, Event::PointerGone)
                && !(matches!(self.owner.gesture, ClientGestureOwner::Touch(..))
                    && touch.gesture_active_after(index))
            {
                self.cancel_owner(ClientActivationCancelReason::PointerGone);
            }
        }
        for (_, reason) in cancellations
            .iter()
            .filter(|(index, _)| *index == events.len())
        {
            self.cancel_owner(*reason);
        }
        if matches!(
            self.owner.gesture,
            ClientGestureOwner::Mouse(_) | ClientGestureOwner::SuppressedMouse(..)
        ) && !primary_down
        {
            self.cancel_owner(ClientActivationCancelReason::ButtonStateLost);
        }
        let active_operation = self.current_operation();
        self.boundaries.retain(|boundary| {
            let Some(operation) = canonical_operation(boundary) else {
                return true;
            };
            active_operation == Some(operation)
                || !(held_operation == Some(operation)
                    || ordered
                        .authorized_presses
                        .iter()
                        .any(|proof| proof.operation == operation))
        });
        match activation {
            ClientFrameCompletion::Activate(source) => Some(source),
            ClientFrameCompletion::None | ClientFrameCompletion::Close => None,
        }
    }

    fn current_operation(&self) -> Option<egui::Id> {
        match self.owner.gesture {
            ClientGestureOwner::Mouse(ClientGesture {
                operation: ClientOperation::Canonical(operation),
                ..
            })
            | ClientGestureOwner::Touch(
                ClientGesture {
                    operation: ClientOperation::Canonical(operation),
                    ..
                },
                _,
            )
            | ClientGestureOwner::SuppressedMouse(_, ClientOperation::Canonical(operation))
            | ClientGestureOwner::SuppressedTouch(_, ClientOperation::Canonical(operation)) => {
                Some(operation)
            }
            _ => None,
        }
    }

    fn owns_close(&self) -> bool {
        matches!(
            self.owner.gesture,
            ClientGestureOwner::Mouse(ClientGesture {
                destination: ClientGestureDestination::WindowClose(_),
                ..
            }) | ClientGestureOwner::Touch(
                ClientGesture {
                    destination: ClientGestureDestination::WindowClose(_),
                    ..
                },
                _
            )
        )
    }

    fn owns_activation(&self) -> bool {
        matches!(
            self.owner.gesture,
            ClientGestureOwner::Mouse(ClientGesture {
                destination: ClientGestureDestination::ActivateOnly,
                ..
            }) | ClientGestureOwner::Touch(
                ClientGesture {
                    destination: ClientGestureDestination::ActivateOnly,
                    ..
                },
                _
            )
        )
    }

    fn complete(&mut self, source: ClientActivationSource, activation: &mut ClientFrameCompletion) {
        let destination = match self.owner.gesture {
            ClientGestureOwner::Mouse(gesture) | ClientGestureOwner::Touch(gesture, _) => {
                gesture.destination
            }
            _ => return,
        };
        let origin = std::mem::take(&mut self.owner).scope;
        match destination {
            ClientGestureDestination::ActivateOnly => {
                self.decisions.push(ScopedClientActivationDecision {
                    scope: origin,
                    decision: ClientActivationDecision::Completed(source),
                });
                if matches!(activation, ClientFrameCompletion::None) {
                    *activation = ClientFrameCompletion::Activate(source);
                }
            }
            ClientGestureDestination::WindowClose(_) => {
                self.decisions.push(ScopedClientActivationDecision {
                    scope: origin,
                    decision: ClientActivationDecision::CompletedClose(source),
                });
                *activation = ClientFrameCompletion::Close;
            }
        }
    }

    fn cancel_owner(&mut self, reason: ClientActivationCancelReason) {
        if reason == ClientActivationCancelReason::CommandOwned && self.owns_close() {
            return;
        }
        let owner = std::mem::take(&mut self.owner);
        let (source, reason) = match owner.gesture {
            ClientGestureOwner::Idle => return,
            ClientGestureOwner::Mouse(_) => (ClientActivationSource::MouseClick, reason),
            ClientGestureOwner::Touch(..) => (ClientActivationSource::TouchTap, reason),
            ClientGestureOwner::SuppressedMouse(original, _) => {
                (ClientActivationSource::MouseClick, original)
            }
            ClientGestureOwner::SuppressedTouch(original, _) => {
                (ClientActivationSource::TouchTap, original)
            }
        };
        self.decisions.push(ScopedClientActivationDecision {
            scope: owner.scope,
            decision: ClientActivationDecision::Cancelled { source, reason },
        });
    }
}

/// Keep parked mouse drag cancellation at egui's existing distance boundary,
/// including an out-and-back move. Duration is deliberately not part of it.
fn mouse_press_became_drag(origin: egui::Pos2, pos: egui::Pos2, max_distance: f32) -> bool {
    origin.distance(pos) > max_distance
}

impl ClientActivationAvailability {
    fn cancel_reason(self) -> Option<ClientActivationCancelReason> {
        match self {
            Self::Available => None,
            Self::CommandOwned => Some(ClientActivationCancelReason::CommandOwned),
            Self::Blocked => Some(ClientActivationCancelReason::ModalOwned),
        }
    }
}

fn in_client(geometry: &TapZoneGeometry, pos: egui::Pos2) -> bool {
    geometry.surface.contains(pos) && !geometry.excluded.iter().any(|rect| rect.contains(pos))
}

fn admission(
    availability: ClientActivationAvailability,
    accepts_press: bool,
    geometry: &TapZoneGeometry,
    pos: egui::Pos2,
    scope_proven: bool,
    destination: ClientGestureDestination,
) -> Result<(), ClientActivationCancelReason> {
    let availability = if availability == ClientActivationAvailability::CommandOwned
        && matches!(destination, ClientGestureDestination::WindowClose(_))
    {
        ClientActivationAvailability::Available
    } else {
        availability
    };
    if let Some(reason) = availability.cancel_reason() {
        Err(reason)
    } else if !accepts_press {
        Err(ClientActivationCancelReason::FocusLost)
    } else if !scope_proven {
        Err(ClientActivationCancelReason::NotParked)
    } else if !match destination {
        ClientGestureDestination::ActivateOnly => in_client(geometry, pos),
        ClientGestureDestination::WindowClose(rect) => {
            geometry.surface.contains(pos) && rect.contains(pos)
        }
    } {
        Err(ClientActivationCancelReason::OutsideClient)
    } else {
        Ok(())
    }
}

fn start_scope_proven(
    require_proof: bool,
    operation: ClientOperation,
    destination: ClientGestureDestination,
) -> bool {
    match destination {
        ClientGestureDestination::WindowClose(_) => {
            matches!(operation, ClientOperation::Canonical(_))
        }
        ClientGestureDestination::ActivateOnly => {
            !require_proof || operation != ClientOperation::Unowned
        }
    }
}

fn destination_at(pos: egui::Pos2, close_rect: Option<egui::Rect>) -> ClientGestureDestination {
    match close_rect.filter(|rect| rect.contains(pos)) {
        Some(rect) => ClientGestureDestination::WindowClose(rect),
        None => ClientGestureDestination::ActivateOnly,
    }
}

fn release_inside(
    destination: ClientGestureDestination,
    geometry: &TapZoneGeometry,
    pos: egui::Pos2,
    close_rect: Option<egui::Rect>,
) -> bool {
    geometry.surface.contains(pos)
        && match destination {
            ClientGestureDestination::WindowClose(rect) => rect.contains(pos),
            ClientGestureDestination::ActivateOnly => {
                in_client(geometry, pos) && !close_rect.is_some_and(|rect| rect.contains(pos))
            }
        }
}

fn scoped_press_proof_required(scope: Option<&super::DetachedActivationRequest>) -> bool {
    #[cfg(test)]
    {
        scope.is_some_and(|scope| scope.allocation.is_some())
    }
    #[cfg(not(test))]
    {
        scope.is_some()
    }
}

struct OrderedClientBoundaries {
    cancellations: Vec<(usize, ClientActivationCancelReason)>,
    authorized_presses: Vec<AuthorizedClientPress>,
    retained: Vec<ControlBoundary>,
}

struct AuthorizedClientPress {
    index: usize,
    operation: egui::Id,
    scope: super::DetachedActivationRequest,
}

/// Align native control metadata to the same callback's ordered primary edges.
/// A cancellation before a promoted press belongs before its raw Touch Start,
/// so the old recognizer is retired before the new contact is admitted.
fn ordered_cancellations(
    events: &[Event],
    boundaries: Vec<ControlBoundary>,
    current_scope: Option<&super::DetachedActivationRequest>,
    promoted_primary_indices: &[usize],
    held_operation: Option<egui::Id>,
) -> OrderedClientBoundaries {
    let mut cancellations = Vec::new();
    let mut authorized_presses = Vec::new();
    let mut ordered = Vec::new();
    let mut canonical = Vec::new();
    for boundary in boundaries {
        match boundary {
            ControlBoundary::RetireScope => cancellations.push((
                0,
                ClientActivationCancelReason::Native(NativeInputCancelReason::RetiredRegistration),
            )),
            boundary @ ControlBoundary::ScopedNative { .. } => canonical.push(boundary),
            boundary => ordered.push(boundary),
        }
    }
    // A canonical cancel can terminate only its own held operation. A queued
    // Released before Cancelled means the physical gesture already completed.
    if let Some(operation) = held_operation
        && let Some(reason) = operation_cancel(&canonical, operation)
    {
        cancellations.push((0, reason));
        canonical.retain(|boundary| canonical_operation(boundary) != Some(operation));
    }
    for (index, event) in events.iter().enumerate() {
        let start = matches!(
            event,
            Event::Touch {
                phase: egui::TouchPhase::Start,
                ..
            }
        ) || (matches!(
            event,
            Event::PointerButton {
                button: PointerButton::Primary,
                pressed: true,
                ..
            }
        ) && !promoted_primary_indices.contains(&index));
        if !start || index == 0 {
            continue;
        }
        let Event::PointerInputProvenance(operation) = events[index - 1] else {
            continue;
        };
        let Some(proof_index) = canonical.iter().position(|boundary| {
            matches!(boundary,
            ControlBoundary::ScopedNative { input, .. }
                if input.operation == operation && input.boundary == NativeClientBoundary::Pressed)
        }) else {
            continue;
        };
        let ControlBoundary::ScopedNative {
            scope, admission, ..
        } = &canonical[proof_index]
        else {
            unreachable!()
        };
        authorized_presses.push(AuthorizedClientPress {
            index,
            operation,
            scope: scope.clone(),
        });
        let reason = if current_scope.is_some_and(|current| current != scope) {
            Some(ClientActivationCancelReason::Native(
                NativeInputCancelReason::RetiredRegistration,
            ))
        } else {
            match admission {
                PressAdmission::Available => operation_cancel(&canonical, operation),
                PressAdmission::Cancelled(ClientActivationCancelReason::CommandOwned) => {
                    operation_cancel(&canonical, operation)
                        .or(Some(ClientActivationCancelReason::CommandOwned))
                }
                PressAdmission::Cancelled(reason) => Some(*reason),
            }
        };
        if let Some(reason) = reason {
            cancellations.push((index + 1, reason));
        }
        // Consume this proof once. Terminal metadata stays until that same
        // operation's raw end is observed, unless cancellation ended it here.
        canonical.remove(proof_index);
        if reason.is_some() {
            canonical.retain(|boundary| canonical_operation(boundary) != Some(operation));
        }
    }
    let mut boundaries = ordered.into_iter().peekable();
    // A leading cancellation belongs to an already observed old press. It
    // must also precede raw-touch-only input with no promoted pointer edge.
    loop {
        let reason = match boundaries.peek().cloned() {
            Some(ControlBoundary::Cancel(reason)) => reason,
            Some(ControlBoundary::Native(NativeClientBoundary::Cancelled(reason))) => {
                ClientActivationCancelReason::Native(reason)
            }
            _ => break,
        };
        cancellations.push((0, reason));
        boundaries.next();
    }
    for (index, event) in events.iter().enumerate() {
        let pressed = match event {
            Event::Touch {
                phase: egui::TouchPhase::Start,
                ..
            } => true,
            Event::Touch {
                phase: egui::TouchPhase::End,
                ..
            } => false,
            Event::PointerButton {
                button: PointerButton::Primary,
                pressed,
                ..
            } if !promoted_primary_indices.contains(&index) => *pressed,
            _ => continue,
        };
        let before = index;
        loop {
            match boundaries.peek().cloned() {
                Some(ControlBoundary::Cancel(reason)) => {
                    cancellations.push((before, reason));
                    boundaries.next();
                }
                Some(ControlBoundary::Native(NativeClientBoundary::Cancelled(reason))) => {
                    cancellations.push((before, ClientActivationCancelReason::Native(reason)));
                    boundaries.next();
                }
                Some(ControlBoundary::Native(NativeClientBoundary::Pressed)) if pressed => {
                    boundaries.next();
                    while let Some(ControlBoundary::CancelAfterPressed(reason)) =
                        boundaries.peek().cloned()
                    {
                        cancellations.push((index + 1, reason));
                        boundaries.next();
                    }
                    break;
                }
                Some(ControlBoundary::Native(NativeClientBoundary::Released)) if !pressed => {
                    boundaries.next();
                    break;
                }
                _ => break,
            }
        }
    }
    cancellations.extend(boundaries.filter_map(|boundary| match boundary {
        ControlBoundary::Cancel(reason) | ControlBoundary::CancelAfterPressed(reason) => {
            Some((events.len(), reason))
        }
        ControlBoundary::Native(NativeClientBoundary::Cancelled(reason)) => {
            Some((events.len(), ClientActivationCancelReason::Native(reason)))
        }
        _ => None,
    }));
    OrderedClientBoundaries {
        cancellations,
        authorized_presses,
        retained: canonical,
    }
}

fn canonical_operation(boundary: &ControlBoundary) -> Option<egui::Id> {
    match boundary {
        ControlBoundary::ScopedNative { input, .. } => Some(input.operation),
        _ => None,
    }
}

fn operation_cancel(
    boundaries: &[ControlBoundary],
    operation: egui::Id,
) -> Option<ClientActivationCancelReason> {
    for boundary in boundaries {
        if let ControlBoundary::ScopedNative { input, .. } = boundary
            && input.operation == operation
        {
            match input.boundary {
                NativeClientBoundary::Released => return None,
                NativeClientBoundary::Cancelled(NativeInputCancelReason::ModalOwned) => {
                    return Some(ClientActivationCancelReason::ModalOwned);
                }
                NativeClientBoundary::Cancelled(reason) => {
                    return Some(ClientActivationCancelReason::Native(reason));
                }
                NativeClientBoundary::Pressed => {}
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{
        Context, Modifiers, Pos2, RawInput, Rect, TouchDeviceId, TouchId, TouchPhase, pos2,
    };

    fn geometry() -> TapZoneGeometry {
        TapZoneGeometry {
            surface: Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 300.0)),
            excluded: vec![Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 40.0))],
            behavior: TouchSurfaceBehavior::Viewer {
                accepts_pinch: true,
                tap_zones: true,
            },
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

    fn touch(id: u64, phase: TouchPhase, pos: Pos2) -> Event {
        Event::Touch {
            device_id: TouchDeviceId(7),
            id: TouchId(id),
            phase,
            pos,
            force: None,
        }
    }

    fn mouse_click(pos: Pos2) -> Vec<Event> {
        vec![
            Event::PointerMoved(pos),
            primary(pos, true),
            primary(pos, false),
        ]
    }

    fn tap(id: u64, pos: Pos2) -> Vec<Event> {
        vec![
            touch(id, TouchPhase::Start, pos),
            Event::PointerMoved(pos),
            primary(pos, true),
            touch(id, TouchPhase::End, pos),
            primary(pos, false),
            Event::PointerGone,
        ]
    }

    fn drive(
        ctx: &Context,
        owner: &mut DeferredClientActivationGesture,
        time: f64,
        events: Vec<Event>,
        availability: ClientActivationAvailability,
        discard: bool,
    ) -> Vec<ClientActivationSource> {
        let mut accepted = Vec::new();
        let mut passes = 0;
        ctx.run_pass(
            RawInput {
                screen_rect: Some(geometry().surface),
                time: Some(time),
                events,
                focused: true,
                ..Default::default()
            },
            |ctx| {
                if let Some(source) = owner.capture(ctx, geometry(), availability) {
                    accepted.push(source);
                }
                assert!(
                    owner.capture(ctx, geometry(), availability).is_none(),
                    "same callback pass cannot replay input"
                );
                if discard && ctx.current_pass_index() == 0 {
                    ctx.request_discard("parked activation regression");
                }
                passes += 1;
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(passes, if discard { 2 } else { 1 });
        accepted
    }

    fn available(
        ctx: &Context,
        owner: &mut DeferredClientActivationGesture,
        time: f64,
        events: Vec<Event>,
    ) -> Vec<ClientActivationSource> {
        drive(
            ctx,
            owner,
            time,
            events,
            ClientActivationAvailability::Available,
            false,
        )
    }

    fn scope(context: u64) -> super::super::DetachedActivationRequest {
        super::super::DetachedActivationRequest {
            window_id: 7,
            registration: None,
            context_id: Some(super::super::ViewerContextId::for_test(context)),
            source: super::super::DetachedActivationSource::ClientClick,
            expected_state: super::super::DetachedActivationTargetState::Parked,
            allocation: None,
        }
    }

    fn scoped(
        ctx: &Context,
        owner: &mut DeferredClientActivationGesture,
        time: f64,
        events: Vec<Event>,
        scope: &super::super::DetachedActivationRequest,
    ) -> Vec<ClientActivationSource> {
        let mut accepted = Vec::new();
        ctx.run_pass(
            RawInput {
                screen_rect: Some(geometry().surface),
                time: Some(time),
                events,
                focused: true,
                ..Default::default()
            },
            |ctx| {
                if scope.allocation.is_none() {
                    let promoted = preview_touch_primary_event_indices(
                        ctx,
                        TouchSurface::StillFullscreen,
                        &geometry(),
                    );
                    let mut operations = owner
                        .boundaries
                        .iter()
                        .filter_map(|boundary| match boundary {
                            ControlBoundary::ScopedNative { input, .. }
                                if input.boundary == NativeClientBoundary::Pressed =>
                            {
                                Some(input.operation)
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .into_iter();
                    ctx.input_mut(|input| {
                        let raw = std::mem::take(&mut input.raw.events);
                        for (index, event) in raw.into_iter().enumerate() {
                            let start = matches!(
                                event,
                                Event::Touch {
                                    phase: TouchPhase::Start,
                                    ..
                                }
                            ) || (matches!(
                                event,
                                Event::PointerButton {
                                    button: PointerButton::Primary,
                                    pressed: true,
                                    ..
                                }
                            ) && !promoted.contains(&index));
                            if start && let Some(operation) = operations.next() {
                                input
                                    .raw
                                    .events
                                    .push(Event::PointerInputProvenance(operation));
                            }
                            input.raw.events.push(event);
                        }
                    });
                }
                if let Some(source) = owner.capture_for_scope(
                    ctx,
                    geometry(),
                    ClientActivationAvailability::Available,
                    scope,
                ) {
                    accepted.push(source);
                }
                assert!(
                    owner
                        .capture_for_scope(
                            ctx,
                            geometry(),
                            ClientActivationAvailability::Available,
                            scope
                        )
                        .is_none()
                );
            },
        )
        .drop_without_applying_deltas();
        accepted
    }

    fn fixture_input(
        owner: &DeferredClientActivationGesture,
        boundary: NativeClientBoundary,
    ) -> NativeClientInputBoundary {
        let operation = if boundary == NativeClientBoundary::Pressed {
            static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            egui::Id::new((
                "fixture-canonical-operation",
                SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            ))
        } else {
            owner
                .boundaries
                .iter()
                .rev()
                .find_map(|boundary| match boundary {
                    ControlBoundary::ScopedNative { input, .. }
                        if input.boundary == NativeClientBoundary::Pressed =>
                    {
                        Some(input.operation)
                    }
                    _ => None,
                })
                .or_else(|| owner.current_operation())
                .expect("terminal fixture boundary must have an operation")
        };
        NativeClientInputBoundary {
            operation,
            boundary,
        }
    }

    fn feed_scoped_click(
        owner: &mut DeferredClientActivationGesture,
        scope: &super::super::DetachedActivationRequest,
    ) -> egui::Id {
        let input = fixture_input(owner, NativeClientBoundary::Pressed);
        owner.feed_native_scoped_boundary(scope.clone(), input);
        owner.feed_native_scoped_boundary(
            scope.clone(),
            NativeClientInputBoundary {
                operation: input.operation,
                boundary: NativeClientBoundary::Released,
            },
        );
        input.operation
    }

    fn marked(mut events: Vec<Event>, operation: egui::Id) -> Vec<Event> {
        let index = events
            .iter()
            .position(|event| {
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
            .unwrap();
        events.insert(index, Event::PointerInputProvenance(operation));
        events
    }

    fn canonical_scope(ctx: &Context) -> super::super::DetachedActivationRequest {
        ctx.run_pass(
            RawInput {
                screen_rect: Some(geometry().surface),
                ..Default::default()
            },
            |_| {},
        )
        .drop_without_applying_deltas();
        let registration = ctx.viewport_registration(egui::ViewportId::ROOT).unwrap();
        let mut scope = scope(1);
        scope.registration = Some(registration);
        scope.allocation = Some(crate::key_input::register_test_native_input_allocation(
            registration,
            0x4A71,
        ));
        scope
    }

    fn close_rect() -> egui::Rect {
        egui::Rect::from_min_max(pos2(340.0, 5.0), pos2(390.0, 35.0))
    }

    fn close_frame(
        ctx: &Context,
        owner: &mut DeferredClientActivationGesture,
        time: f64,
        events: Vec<Event>,
        scope: &super::super::DetachedActivationRequest,
        availability: ClientActivationAvailability,
        discard: bool,
    ) -> Vec<ClientActivationSource> {
        let mut activation = Vec::new();
        ctx.run_pass(
            RawInput {
                screen_rect: Some(geometry().surface),
                time: Some(time),
                events,
                focused: true,
                ..Default::default()
            },
            |ctx| {
                let mut geometry = geometry();
                // The caller gives the close affordance its own input destination,
                // rather than excluding it with the ordinary passive-viewer chrome.
                geometry.excluded.clear();
                if let Some(source) = owner.capture_for_scope_with_close(
                    ctx,
                    geometry.clone(),
                    availability,
                    scope,
                    close_rect(),
                ) {
                    activation.push(source);
                }
                assert!(
                    owner
                        .capture_for_scope_with_close(
                            ctx,
                            geometry,
                            availability,
                            scope,
                            close_rect()
                        )
                        .is_none()
                );
                if discard && ctx.current_pass_index() == 0 {
                    ctx.request_discard("passive close gesture regression");
                }
            },
        )
        .drop_without_applying_deltas();
        activation
    }

    #[test]
    fn diagnostic_touch_disable_keeps_canonical_tap_and_close_in_same_and_split_batches() {
        let _input = crate::key_input::lock_test_input();
        crate::touch_correlation::with_gestures_disabled(true, || {
            for close in [false, true] {
                for split in [false, true] {
                    let ctx = Context::default();
                    let scope = canonical_scope(&ctx);
                    let mut owner = DeferredClientActivationGesture::default();
                    let operation = feed_scoped_click(&mut owner, &scope);
                    let pos = if close {
                        close_rect().center()
                    } else {
                        pos2(200.0, 150.0)
                    };
                    let mut events = marked(tap(1, pos), operation);
                    let accepted = if split {
                        let end = events.split_off(4);
                        assert!(
                            close_frame(
                                &ctx,
                                &mut owner,
                                1.0,
                                events,
                                &scope,
                                ClientActivationAvailability::Available,
                                false,
                            )
                            .is_empty()
                        );
                        assert!(owner.drain_decisions().is_empty());
                        close_frame(
                            &ctx,
                            &mut owner,
                            1.1,
                            end,
                            &scope,
                            ClientActivationAvailability::Available,
                            true,
                        )
                    } else {
                        close_frame(
                            &ctx,
                            &mut owner,
                            1.0,
                            events,
                            &scope,
                            ClientActivationAvailability::Available,
                            true,
                        )
                    };
                    assert_eq!(
                        accepted,
                        if close {
                            vec![]
                        } else {
                            vec![ClientActivationSource::TouchTap]
                        },
                        "diagnostic disable must retain ordinary click completion (close={close}, split={split})"
                    );
                    assert_eq!(
                        owner.drain_decisions(),
                        vec![if close {
                            ClientActivationDecision::CompletedClose(
                                ClientActivationSource::TouchTap,
                            )
                        } else {
                            ClientActivationDecision::Completed(ClientActivationSource::TouchTap)
                        }]
                    );
                }
            }
        });
    }

    #[test]
    fn diagnostic_touch_disable_still_rejects_cancel_drag_and_multiple_contacts() {
        let _input = crate::key_input::lock_test_input();
        crate::touch_correlation::with_gestures_disabled(true, || {
            for close in [false, true] {
                for failure in 0..3 {
                    let ctx = Context::default();
                    let scope = canonical_scope(&ctx);
                    let mut owner = DeferredClientActivationGesture::default();
                    let pos = if close {
                        close_rect().center()
                    } else {
                        pos2(200.0, 150.0)
                    };
                    let mut events = vec![
                        touch(1, TouchPhase::Start, pos),
                        Event::PointerMoved(pos),
                        primary(pos, true),
                    ];
                    let reason = match failure {
                        0 => {
                            events.push(touch(1, TouchPhase::Cancel, pos));
                            ClientActivationCancelReason::TouchCancelled
                        }
                        1 => {
                            let away = pos + egui::vec2(-100.0, 60.0);
                            events.extend([
                                touch(1, TouchPhase::Move, away),
                                Event::PointerMoved(away),
                                touch(1, TouchPhase::Move, pos),
                                Event::PointerMoved(pos),
                                touch(1, TouchPhase::End, pos),
                            ]);
                            ClientActivationCancelReason::Drag
                        }
                        _ => {
                            let other = pos + egui::vec2(-40.0, 40.0);
                            events.extend([
                                touch(2, TouchPhase::Start, other),
                                touch(2, TouchPhase::End, other),
                                touch(1, TouchPhase::End, pos),
                            ]);
                            ClientActivationCancelReason::MultipleTouch
                        }
                    };
                    events.extend([primary(pos, false), Event::PointerGone]);
                    let operation = feed_scoped_click(&mut owner, &scope);
                    assert!(
                        close_frame(
                            &ctx,
                            &mut owner,
                            1.0,
                            marked(events, operation),
                            &scope,
                            ClientActivationAvailability::Available,
                            false
                        )
                        .is_empty()
                    );
                    assert_eq!(
                        owner.drain_decisions(),
                        [ClientActivationDecision::Cancelled {
                            source: ClientActivationSource::TouchTap,
                            reason,
                        }]
                    );
                    let fresh = feed_scoped_click(&mut owner, &scope);
                    let accepted = close_frame(
                        &ctx,
                        &mut owner,
                        1.1,
                        marked(tap(3, pos), fresh),
                        &scope,
                        ClientActivationAvailability::Available,
                        false,
                    );
                    assert_eq!(
                        accepted,
                        if close {
                            vec![]
                        } else {
                            vec![ClientActivationSource::TouchTap]
                        }
                    );
                    assert_eq!(
                        owner.drain_decisions(),
                        [if close {
                            ClientActivationDecision::CompletedClose(
                                ClientActivationSource::TouchTap,
                            )
                        } else {
                            ClientActivationDecision::Completed(ClientActivationSource::TouchTap)
                        }]
                    );
                }
            }
        });
    }

    #[test]
    fn diagnostic_touch_disable_cannot_readmit_a_modal_or_retired_touch() {
        let _input = crate::key_input::lock_test_input();
        crate::touch_correlation::with_gestures_disabled(true, || {
            for close in [false, true] {
                for modal in [false, true] {
                    let ctx = Context::default();
                    let old = canonical_scope(&ctx);
                    let mut owner = DeferredClientActivationGesture::default();
                    let press = fixture_input(&owner, NativeClientBoundary::Pressed);
                    owner.feed_native_scoped_boundary(old.clone(), press);
                    let pos = if close {
                        close_rect().center()
                    } else {
                        pos2(200.0, 150.0)
                    };
                    assert!(
                        close_frame(
                            &ctx,
                            &mut owner,
                            1.0,
                            marked(
                                vec![
                                    touch(1, TouchPhase::Start, pos),
                                    Event::PointerMoved(pos),
                                    primary(pos, true)
                                ],
                                press.operation
                            ),
                            &old,
                            ClientActivationAvailability::Available,
                            false
                        )
                        .is_empty()
                    );
                    let mut current = old.clone();
                    if modal {
                        owner.set_availability(ClientActivationAvailability::Blocked);
                        owner.set_availability(ClientActivationAvailability::Available);
                    } else {
                        current.context_id = Some(super::super::ViewerContextId::for_test(2));
                        owner.set_scope(current.clone());
                    }
                    let cancelled = owner.drain_scoped_decisions();
                    assert_eq!(cancelled.len(), 1);
                    assert_eq!(cancelled[0].scope.as_ref(), Some(&old));
                    assert_eq!(
                        cancelled[0].decision,
                        ClientActivationDecision::Cancelled {
                            source: ClientActivationSource::TouchTap,
                            reason: if modal {
                                ClientActivationCancelReason::ModalOwned
                            } else {
                                ClientActivationCancelReason::Native(
                                    NativeInputCancelReason::RetiredRegistration,
                                )
                            },
                        }
                    );
                    owner.feed_native_scoped_boundary(
                        old.clone(),
                        NativeClientInputBoundary {
                            operation: press.operation,
                            boundary: NativeClientBoundary::Released,
                        },
                    );
                    assert!(
                        close_frame(
                            &ctx,
                            &mut owner,
                            1.1,
                            vec![
                                touch(1, TouchPhase::End, pos),
                                primary(pos, false),
                                Event::PointerGone
                            ],
                            &current,
                            ClientActivationAvailability::Available,
                            false
                        )
                        .is_empty()
                    );
                    assert!(owner.drain_decisions().is_empty());
                    let fresh = feed_scoped_click(&mut owner, &current);
                    let accepted = close_frame(
                        &ctx,
                        &mut owner,
                        1.2,
                        marked(tap(2, pos), fresh),
                        &current,
                        ClientActivationAvailability::Available,
                        false,
                    );
                    assert_eq!(
                        accepted,
                        if close {
                            vec![]
                        } else {
                            vec![ClientActivationSource::TouchTap]
                        }
                    );
                    assert_eq!(
                        owner.drain_decisions(),
                        [if close {
                            ClientActivationDecision::CompletedClose(
                                ClientActivationSource::TouchTap,
                            )
                        } else {
                            ClientActivationDecision::Completed(ClientActivationSource::TouchTap)
                        }]
                    );
                }
            }
        });
    }

    #[test]
    fn coalesced_canonical_close_needs_no_hover_or_previous_widget_and_wins_command_ownership() {
        let _input = crate::key_input::lock_test_input();
        for touch_input in [false, true] {
            for availability in [
                ClientActivationAvailability::Available,
                ClientActivationAvailability::CommandOwned,
            ] {
                let ctx = Context::default();
                let scope = canonical_scope(&ctx);
                let mut owner = DeferredClientActivationGesture::default();
                owner.set_availability(availability);
                let operation = feed_scoped_click(&mut owner, &scope);
                let pos = close_rect().center();
                let events = if touch_input {
                    tap(1, pos)
                } else {
                    mouse_click(pos)
                };
                assert!(
                    close_frame(
                        &ctx,
                        &mut owner,
                        1.0,
                        marked(events, operation),
                        &scope,
                        availability,
                        true
                    )
                    .is_empty()
                );
                let source = if touch_input {
                    ClientActivationSource::TouchTap
                } else {
                    ClientActivationSource::MouseClick
                };
                assert_eq!(
                    owner.drain_decisions(),
                    [ClientActivationDecision::CompletedClose(source)]
                );
            }
        }
    }

    #[test]
    fn close_release_outside_or_drag_never_falls_back_to_activation() {
        let _input = crate::key_input::lock_test_input();
        for touch_input in [false, true] {
            for drag in [false, true] {
                let ctx = Context::default();
                let scope = canonical_scope(&ctx);
                let mut owner = DeferredClientActivationGesture::default();
                let operation = feed_scoped_click(&mut owner, &scope);
                let start = if drag {
                    close_rect().center()
                } else {
                    pos2(388.0, 20.0)
                };
                let end = if drag { start } else { pos2(394.0, 20.0) };
                let far = pos2(220.0, 140.0);
                let mut events = if touch_input {
                    vec![
                        touch(1, TouchPhase::Start, start),
                        Event::PointerMoved(start),
                        primary(start, true),
                    ]
                } else {
                    vec![Event::PointerMoved(start), primary(start, true)]
                };
                if drag {
                    if touch_input {
                        events.push(touch(1, TouchPhase::Move, far));
                    }
                    events.push(Event::PointerMoved(far));
                }
                if touch_input {
                    events.extend([
                        touch(1, TouchPhase::End, end),
                        primary(if drag { far } else { start }, false),
                        Event::PointerGone,
                    ]);
                } else {
                    events.push(primary(end, false));
                }
                assert!(
                    close_frame(
                        &ctx,
                        &mut owner,
                        1.0,
                        marked(events, operation),
                        &scope,
                        ClientActivationAvailability::Available,
                        false
                    )
                    .is_empty()
                );
                assert!(owner.drain_decisions().iter().all(|decision| matches!(
                    decision,
                    ClientActivationDecision::Cancelled { .. }
                )));
            }
        }
    }

    #[test]
    fn close_requires_its_canonical_start_and_a_single_touch() {
        let _input = crate::key_input::lock_test_input();
        for touch_input in [false, true] {
            let ctx = Context::default();
            let scope = canonical_scope(&ctx);
            let mut owner = DeferredClientActivationGesture::default();
            let pos = close_rect().center();
            let events = if touch_input {
                tap(1, pos)
            } else {
                mouse_click(pos)
            };
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    1.0,
                    events,
                    &scope,
                    ClientActivationAvailability::Available,
                    false
                )
                .is_empty()
            );
            assert!(owner.drain_decisions().iter().all(|decision| matches!(
                decision,
                ClientActivationDecision::Cancelled {
                    reason: ClientActivationCancelReason::NotParked,
                    ..
                }
            )));
            let operation = feed_scoped_click(&mut owner, &scope);
            let events = vec![
                touch(2, TouchPhase::Start, pos),
                Event::PointerMoved(pos),
                primary(pos, true),
                touch(3, TouchPhase::Start, pos2(355.0, 25.0)),
                touch(3, TouchPhase::End, pos2(355.0, 25.0)),
                touch(2, TouchPhase::End, pos),
                primary(pos, false),
                Event::PointerGone,
            ];
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    1.1,
                    marked(events, operation),
                    &scope,
                    ClientActivationAvailability::Available,
                    false
                )
                .is_empty()
            );
            assert_eq!(
                owner.drain_decisions(),
                [ClientActivationDecision::Cancelled {
                    source: ClientActivationSource::TouchTap,
                    reason: ClientActivationCancelReason::MultipleTouch
                }]
            );
        }
    }

    #[test]
    fn modal_between_close_down_and_up_outranks_command_ownership() {
        let _input = crate::key_input::lock_test_input();
        for touch_input in [false, true] {
            let ctx = Context::default();
            let scope = canonical_scope(&ctx);
            let mut owner = DeferredClientActivationGesture::default();
            let press = fixture_input(&owner, NativeClientBoundary::Pressed);
            owner.feed_native_scoped_boundary(scope.clone(), press);
            let pos = close_rect().center();
            let down = if touch_input {
                vec![
                    touch(1, TouchPhase::Start, pos),
                    Event::PointerMoved(pos),
                    primary(pos, true),
                ]
            } else {
                vec![primary(pos, true)]
            };
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    1.0,
                    marked(down, press.operation),
                    &scope,
                    ClientActivationAvailability::Available,
                    false
                )
                .is_empty()
            );
            owner.set_availability(ClientActivationAvailability::CommandOwned);
            owner.set_availability(ClientActivationAvailability::Blocked);
            owner.set_availability(ClientActivationAvailability::CommandOwned);
            owner.feed_native_scoped_boundary(
                scope.clone(),
                NativeClientInputBoundary {
                    operation: press.operation,
                    boundary: NativeClientBoundary::Released,
                },
            );
            let up = if touch_input {
                vec![
                    touch(1, TouchPhase::End, pos),
                    primary(pos, false),
                    Event::PointerGone,
                ]
            } else {
                vec![primary(pos, false)]
            };
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    1.1,
                    up,
                    &scope,
                    ClientActivationAvailability::CommandOwned,
                    false
                )
                .is_empty()
            );
            let source = if touch_input {
                ClientActivationSource::TouchTap
            } else {
                ClientActivationSource::MouseClick
            };
            assert_eq!(
                owner.drain_decisions(),
                [ClientActivationDecision::Cancelled {
                    source,
                    reason: ClientActivationCancelReason::ModalOwned
                }]
            );
            let fresh = feed_scoped_click(&mut owner, &scope);
            let events = if touch_input {
                tap(2, pos)
            } else {
                mouse_click(pos)
            };
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    1.2,
                    marked(events, fresh),
                    &scope,
                    ClientActivationAvailability::CommandOwned,
                    false
                )
                .is_empty()
            );
            assert_eq!(
                owner.drain_decisions(),
                [ClientActivationDecision::CompletedClose(source)]
            );
        }
    }

    #[test]
    fn scope_retirement_and_native_cancel_end_close_without_fallback() {
        let _input = crate::key_input::lock_test_input();
        for retire in [false, true] {
            let ctx = Context::default();
            let old = canonical_scope(&ctx);
            let mut owner = DeferredClientActivationGesture::default();
            let press = fixture_input(&owner, NativeClientBoundary::Pressed);
            owner.feed_native_scoped_boundary(old.clone(), press);
            let pos = close_rect().center();
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    1.0,
                    marked(vec![primary(pos, true)], press.operation),
                    &old,
                    ClientActivationAvailability::Available,
                    false
                )
                .is_empty()
            );
            let (current, reason) = if retire {
                let current = recreated_canonical_scope(&old);
                owner.retire_scope_for(&current);
                (current, NativeInputCancelReason::RetiredRegistration)
            } else {
                owner.feed_native_scoped_boundary(
                    old.clone(),
                    NativeClientInputBoundary {
                        operation: press.operation,
                        boundary: NativeClientBoundary::Cancelled(
                            NativeInputCancelReason::CaptureLost,
                        ),
                    },
                );
                (old.clone(), NativeInputCancelReason::CaptureLost)
            };
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    1.1,
                    vec![primary(pos, false)],
                    &current,
                    ClientActivationAvailability::Available,
                    false
                )
                .is_empty()
            );
            assert_eq!(
                owner.drain_decisions(),
                [ClientActivationDecision::Cancelled {
                    source: ClientActivationSource::MouseClick,
                    reason: ClientActivationCancelReason::Native(reason)
                }]
            );
        }
    }

    #[test]
    fn native_cancelled_close_cannot_be_unblocked_by_command_ownership_priority() {
        let _input = crate::key_input::lock_test_input();
        for native_reason in [
            NativeInputCancelReason::CaptureLost,
            NativeInputCancelReason::ModalOwned,
        ] {
            let ctx = Context::default();
            let scope = canonical_scope(&ctx);
            let mut owner = DeferredClientActivationGesture::default();
            let press = fixture_input(&owner, NativeClientBoundary::Pressed);
            owner.feed_native_scoped_boundary(scope.clone(), press);
            owner.feed_native_scoped_boundary(
                scope.clone(),
                NativeClientInputBoundary {
                    operation: press.operation,
                    boundary: NativeClientBoundary::Cancelled(native_reason),
                },
            );
            owner.feed_native_scoped_boundary(
                scope.clone(),
                NativeClientInputBoundary {
                    operation: press.operation,
                    boundary: NativeClientBoundary::Released,
                },
            );
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    1.0,
                    marked(tap(1, close_rect().center()), press.operation),
                    &scope,
                    ClientActivationAvailability::CommandOwned,
                    false
                )
                .is_empty()
            );
            let reason = if native_reason == NativeInputCancelReason::ModalOwned {
                ClientActivationCancelReason::ModalOwned
            } else {
                ClientActivationCancelReason::Native(native_reason)
            };
            assert_eq!(
                owner.drain_decisions(),
                [ClientActivationDecision::Cancelled {
                    source: ClientActivationSource::TouchTap,
                    reason
                }]
            );
        }
    }

    #[test]
    fn split_touch_release_uses_its_raw_end_position_for_close() {
        let _input = crate::key_input::lock_test_input();
        for outside in [false, true] {
            let ctx = Context::default();
            let scope = canonical_scope(&ctx);
            let mut owner = DeferredClientActivationGesture::default();
            let press = fixture_input(&owner, NativeClientBoundary::Pressed);
            owner.feed_native_scoped_boundary(scope.clone(), press);
            let start = pos2(388.0, 20.0);
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    1.0,
                    marked(
                        vec![
                            touch(1, TouchPhase::Start, start),
                            Event::PointerMoved(start),
                            primary(start, true)
                        ],
                        press.operation
                    ),
                    &scope,
                    ClientActivationAvailability::Available,
                    false
                )
                .is_empty()
            );
            owner.feed_native_scoped_boundary(
                scope.clone(),
                NativeClientInputBoundary {
                    operation: press.operation,
                    boundary: NativeClientBoundary::Released,
                },
            );
            let end = if outside { pos2(394.0, 20.0) } else { start };
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    1.1,
                    vec![touch(1, TouchPhase::End, end)],
                    &scope,
                    ClientActivationAvailability::Available,
                    false
                )
                .is_empty()
            );
            assert!(owner.drain_decisions().is_empty());
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    1.2,
                    vec![primary(start, false), Event::PointerGone],
                    &scope,
                    ClientActivationAvailability::Available,
                    false
                )
                .is_empty()
            );
            let decisions = owner.drain_decisions();
            if outside {
                assert_eq!(
                    decisions,
                    [ClientActivationDecision::Cancelled {
                        source: ClientActivationSource::TouchTap,
                        reason: ClientActivationCancelReason::OutsideClient
                    }]
                );
            } else {
                assert_eq!(
                    decisions,
                    [ClientActivationDecision::CompletedClose(
                        ClientActivationSource::TouchTap
                    )]
                );
            }
        }
    }

    #[test]
    fn close_completion_suppresses_activation_from_an_earlier_operation_in_the_batch() {
        let _input = crate::key_input::lock_test_input();
        let ctx = Context::default();
        let scope = canonical_scope(&ctx);
        let mut owner = DeferredClientActivationGesture::default();
        let activate = feed_scoped_click(&mut owner, &scope);
        let close = feed_scoped_click(&mut owner, &scope);
        let mut events = marked(mouse_click(pos2(100.0, 100.0)), activate);
        events.extend(marked(mouse_click(close_rect().center()), close));
        assert!(
            close_frame(
                &ctx,
                &mut owner,
                1.0,
                events,
                &scope,
                ClientActivationAvailability::Available,
                false
            )
            .is_empty()
        );
        assert_eq!(
            owner.drain_decisions(),
            [
                ClientActivationDecision::Completed(ClientActivationSource::MouseClick),
                ClientActivationDecision::CompletedClose(ClientActivationSource::MouseClick)
            ]
        );
    }

    #[test]
    fn a_press_outside_close_cannot_change_destination_on_release() {
        let _input = crate::key_input::lock_test_input();
        for touch_input in [false, true] {
            let ctx = Context::default();
            let scope = canonical_scope(&ctx);
            let mut owner = DeferredClientActivationGesture::default();
            let operation = feed_scoped_click(&mut owner, &scope);
            let start = pos2(338.0, 20.0);
            let end = pos2(342.0, 20.0);
            let events = if touch_input {
                vec![
                    touch(1, TouchPhase::Start, start),
                    Event::PointerMoved(start),
                    primary(start, true),
                    touch(1, TouchPhase::End, end),
                    primary(start, false),
                    Event::PointerGone,
                ]
            } else {
                vec![
                    Event::PointerMoved(start),
                    primary(start, true),
                    primary(end, false),
                ]
            };
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    1.0,
                    marked(events, operation),
                    &scope,
                    ClientActivationAvailability::Available,
                    false
                )
                .is_empty()
            );
            assert!(
                owner
                    .drain_decisions()
                    .iter()
                    .all(|decision| matches!(decision, ClientActivationDecision::Cancelled { .. }))
            );
        }
    }

    fn recreated_canonical_scope(
        old: &super::super::DetachedActivationRequest,
    ) -> super::super::DetachedActivationRequest {
        let mut scope = old.clone();
        scope.allocation = Some(crate::key_input::register_test_native_input_allocation(
            old.registration.unwrap(),
            0x4A72,
        ));
        assert_eq!(old.context_id, scope.context_id);
        assert_eq!(old.registration, scope.registration);
        assert!(
            !old.allocation
                .as_ref()
                .unwrap()
                .same_allocation(scope.allocation.as_ref().unwrap())
        );
        scope
    }

    #[test]
    fn queued_cancellation_keeps_the_operations_scope_after_successor_publication() {
        let _input = crate::key_input::lock_test_input();
        for touch_input in [false, true] {
            for modal in [false, true] {
                let ctx = Context::default();
                let old = canonical_scope(&ctx);
                let mut owner = DeferredClientActivationGesture::default();
                let press = fixture_input(&owner, NativeClientBoundary::Pressed);
                owner.feed_native_scoped_boundary(old.clone(), press);
                let pos = pos2(100.0, 100.0);
                let start = if touch_input {
                    vec![
                        touch(1, TouchPhase::Start, pos),
                        Event::PointerMoved(pos),
                        primary(pos, true),
                    ]
                } else {
                    vec![Event::PointerMoved(pos), primary(pos, true)]
                };
                assert!(
                    scoped(&ctx, &mut owner, 1.0, marked(start, press.operation), &old).is_empty()
                );
                if modal {
                    owner.set_availability(ClientActivationAvailability::Blocked);
                    owner.set_availability(ClientActivationAvailability::Available);
                }
                let mut current = recreated_canonical_scope(&old);
                current.window_id += 1;
                current.context_id = Some(super::super::ViewerContextId::for_test(2));
                owner.set_scope(current.clone());
                assert_eq!(owner.scope(), Some(&current));
                let decisions = owner.drain_scoped_decisions();
                assert_eq!(decisions.len(), 1);
                assert_eq!(decisions[0].scope.as_ref(), Some(&old));
                assert_eq!(
                    decisions[0].decision,
                    ClientActivationDecision::Cancelled {
                        source: if touch_input {
                            ClientActivationSource::TouchTap
                        } else {
                            ClientActivationSource::MouseClick
                        },
                        reason: if modal {
                            ClientActivationCancelReason::ModalOwned
                        } else {
                            ClientActivationCancelReason::Native(
                                NativeInputCancelReason::RetiredRegistration,
                            )
                        },
                    }
                );
                assert!(owner.drain_scoped_decisions().is_empty());
            }
        }
    }

    #[test]
    fn rejected_old_proof_keeps_its_origin_but_unproven_input_has_no_successor_origin() {
        let _input = crate::key_input::lock_test_input();
        for touch_input in [false, true] {
            let ctx = Context::default();
            let old = canonical_scope(&ctx);
            let current = recreated_canonical_scope(&old);
            let mut owner = DeferredClientActivationGesture::default();
            let operation = feed_scoped_click(&mut owner, &old);
            let make_events = |id, pos| {
                if touch_input {
                    tap(id, pos)
                } else {
                    mouse_click(pos)
                }
            };
            let mut events = marked(make_events(1, pos2(100.0, 100.0)), operation);
            events.extend(make_events(2, pos2(200.0, 150.0)));
            assert!(scoped(&ctx, &mut owner, 1.0, events, &current).is_empty());
            let decisions = owner.drain_scoped_decisions();
            assert_eq!(decisions.len(), 2);
            assert_eq!(decisions[0].scope.as_ref(), Some(&old));
            assert_eq!(decisions[1].scope, None);
            assert_eq!(
                decisions[0].decision,
                ClientActivationDecision::Cancelled {
                    source: if touch_input {
                        ClientActivationSource::TouchTap
                    } else {
                        ClientActivationSource::MouseClick
                    },
                    reason: ClientActivationCancelReason::Native(
                        NativeInputCancelReason::RetiredRegistration
                    ),
                }
            );
            assert_eq!(
                decisions[1].decision,
                ClientActivationDecision::Cancelled {
                    source: if touch_input {
                        ClientActivationSource::TouchTap
                    } else {
                        ClientActivationSource::MouseClick
                    },
                    reason: ClientActivationCancelReason::NotParked,
                }
            );
        }
    }

    #[test]
    fn completed_decision_keeps_its_origin_across_scope_retirement() {
        let _input = crate::key_input::lock_test_input();
        let ctx = Context::default();
        let old = canonical_scope(&ctx);
        let mut owner = DeferredClientActivationGesture::default();
        let operation = feed_scoped_click(&mut owner, &old);
        assert_eq!(
            scoped(
                &ctx,
                &mut owner,
                1.0,
                marked(tap(1, pos2(100.0, 100.0)), operation),
                &old
            ),
            [ClientActivationSource::TouchTap]
        );
        let current = recreated_canonical_scope(&old);
        owner.set_scope(current);
        let decisions = owner.drain_scoped_decisions();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].scope.as_ref(), Some(&old));
        assert_eq!(
            decisions[0].decision,
            ClientActivationDecision::Completed(ClientActivationSource::TouchTap)
        );
    }

    fn delayed_retired_proof_decision(return_to_old_scope: bool) {
        let _input = crate::key_input::lock_test_input();
        for touch_input in [false, true] {
            let ctx = Context::default();
            let old = canonical_scope(&ctx);
            let mut owner = DeferredClientActivationGesture::default();
            owner.set_scope(old.clone());
            let operation = feed_scoped_click(&mut owner, &old);
            let current = recreated_canonical_scope(&old);
            owner.set_scope(current.clone());
            let published = if return_to_old_scope {
                old.clone()
            } else {
                current
            };
            owner.set_scope(published.clone());
            // Later modal transitions cannot overwrite irreversible retirement.
            owner.set_availability(ClientActivationAvailability::Blocked);
            owner.set_availability(ClientActivationAvailability::Available);
            let pos = pos2(100.0, 100.0);
            let events = if touch_input {
                tap(1, pos)
            } else {
                mouse_click(pos)
            };
            assert!(
                scoped(&ctx, &mut owner, 1.0, marked(events, operation), &published).is_empty()
            );
            let decisions = owner.drain_scoped_decisions();
            assert_eq!(decisions.len(), 1);
            assert_eq!(decisions[0].scope.as_ref(), Some(&old));
            assert_eq!(
                decisions[0].decision,
                ClientActivationDecision::Cancelled {
                    source: if touch_input {
                        ClientActivationSource::TouchTap
                    } else {
                        ClientActivationSource::MouseClick
                    },
                    reason: ClientActivationCancelReason::Native(
                        NativeInputCancelReason::RetiredRegistration
                    ),
                }
            );
        }
    }

    #[test]
    fn delayed_old_proof_keeps_origin_after_scope_publication_retires_its_authority() {
        delayed_retired_proof_decision(false);
    }

    #[test]
    fn retired_proof_cannot_reactivate_after_publishing_the_same_scope_values_again() {
        delayed_retired_proof_decision(true);
    }

    #[test]
    fn publishing_new_scope_keeps_its_already_arrived_first_canonical_click() {
        let _input = crate::key_input::lock_test_input();
        for old_was_held in [false, true] {
            let ctx = Context::default();
            let old = canonical_scope(&ctx);
            let mut owner = DeferredClientActivationGesture::default();
            let pos = pos2(100.0, 100.0);
            if old_was_held {
                let press = fixture_input(&owner, NativeClientBoundary::Pressed);
                owner.feed_native_scoped_boundary(old.clone(), press);
                assert!(
                    scoped(
                        &ctx,
                        &mut owner,
                        1.0,
                        marked(vec![primary(pos, true)], press.operation),
                        &old
                    )
                    .is_empty()
                );
            }
            let current = recreated_canonical_scope(&old);
            let operation = feed_scoped_click(&mut owner, &current);
            // Backend E2 publishes input before Root's callback scope catches up
            // from None/E. Its proof belongs to E2 and must survive retirement.
            owner.retire_scope_for(&current);
            if old_was_held {
                assert_eq!(
                    owner.drain_decisions(),
                    [ClientActivationDecision::Cancelled {
                        source: ClientActivationSource::MouseClick,
                        reason: ClientActivationCancelReason::Native(
                            NativeInputCancelReason::RetiredRegistration
                        ),
                    }]
                );
            }
            assert_eq!(
                scoped(
                    &ctx,
                    &mut owner,
                    1.1,
                    marked(mouse_click(pos), operation),
                    &current
                ),
                [ClientActivationSource::MouseClick]
            );
            assert_eq!(
                owner.drain_decisions(),
                [ClientActivationDecision::Completed(
                    ClientActivationSource::MouseClick
                )]
            );
        }
    }

    #[test]
    fn replacement_scope_retires_old_proofs_and_keeps_new_proof_authority() {
        let _input = crate::key_input::lock_test_input();
        let ctx = Context::default();
        let old = canonical_scope(&ctx);
        let current = recreated_canonical_scope(&old);
        let mut owner = DeferredClientActivationGesture::default();
        let old_operation = feed_scoped_click(&mut owner, &old);
        let new_operation = feed_scoped_click(&mut owner, &current);
        owner.feed_native_boundary(NativeClientBoundary::Cancelled(
            NativeInputCancelReason::FocusLost,
        ));
        owner.retire_scope_for(&current);
        assert!(owner.boundaries.iter().all(|boundary| match boundary {
            ControlBoundary::ScopedNative {
                scope,
                input,
                admission,
            } =>
                scope == &current
                    || input.boundary != NativeClientBoundary::Pressed
                    || matches!(
                        admission,
                        PressAdmission::Cancelled(ClientActivationCancelReason::Native(
                            NativeInputCancelReason::RetiredRegistration
                        ))
                    ),
            ControlBoundary::RetireScope => true,
            _ => false,
        }));
        let mut events = marked(mouse_click(pos2(100.0, 100.0)), old_operation);
        events.extend(marked(mouse_click(pos2(220.0, 120.0)), new_operation));
        assert_eq!(
            scoped(&ctx, &mut owner, 1.0, events, &current),
            [ClientActivationSource::MouseClick]
        );
        assert_eq!(
            owner.drain_decisions(),
            [
                ClientActivationDecision::Cancelled {
                    source: ClientActivationSource::MouseClick,
                    reason: ClientActivationCancelReason::Native(
                        NativeInputCancelReason::RetiredRegistration
                    )
                },
                ClientActivationDecision::Completed(ClientActivationSource::MouseClick),
            ]
        );
    }

    #[test]
    fn publishing_replacement_scope_keeps_modal_cancellation_on_early_new_proof() {
        let _input = crate::key_input::lock_test_input();
        let ctx = Context::default();
        let old = canonical_scope(&ctx);
        let current = recreated_canonical_scope(&old);
        let mut owner = DeferredClientActivationGesture::default();
        owner.set_availability(ClientActivationAvailability::Blocked);
        let cancelled = feed_scoped_click(&mut owner, &current);
        owner.set_availability(ClientActivationAvailability::Available);
        owner.retire_scope_for(&current);
        let pos = pos2(100.0, 100.0);
        assert!(
            scoped(
                &ctx,
                &mut owner,
                1.0,
                marked(mouse_click(pos), cancelled),
                &current
            )
            .is_empty()
        );
        assert_eq!(
            owner.drain_decisions(),
            [ClientActivationDecision::Cancelled {
                source: ClientActivationSource::MouseClick,
                reason: ClientActivationCancelReason::ModalOwned,
            }]
        );
        let fresh = feed_scoped_click(&mut owner, &current);
        assert_eq!(
            scoped(
                &ctx,
                &mut owner,
                1.1,
                marked(mouse_click(pos), fresh),
                &current
            ),
            [ClientActivationSource::MouseClick]
        );
    }

    #[test]
    fn canonical_scope_rejects_unproven_old_mouse_input_and_accepts_fresh_proof_once() {
        let _input = crate::key_input::lock_test_input();
        let ctx = Context::default();
        let scope = canonical_scope(&ctx);
        let mut owner = DeferredClientActivationGesture::default();
        let pos = pos2(100.0, 100.0);
        assert!(scoped(&ctx, &mut owner, 1.0, mouse_click(pos), &scope).is_empty());
        assert_eq!(
            owner.drain_decisions(),
            [ClientActivationDecision::Cancelled {
                source: ClientActivationSource::MouseClick,
                reason: ClientActivationCancelReason::NotParked,
            }]
        );
        let operation = feed_scoped_click(&mut owner, &scope);
        assert_eq!(
            scoped(
                &ctx,
                &mut owner,
                1.1,
                marked(mouse_click(pos), operation),
                &scope
            ),
            [ClientActivationSource::MouseClick]
        );
        assert_eq!(
            owner.drain_decisions(),
            [ClientActivationDecision::Completed(
                ClientActivationSource::MouseClick
            )]
        );
    }

    #[test]
    fn canonical_scope_fresh_proof_cannot_be_taken_by_older_unproven_input_in_the_same_batch() {
        let _input = crate::key_input::lock_test_input();
        let ctx = Context::default();
        let scope = canonical_scope(&ctx);
        let mut owner = DeferredClientActivationGesture::default();
        // Only the second operation was appended while this scope owned the
        // canonical observer. The first operation was buffered while active.
        let operation = feed_scoped_click(&mut owner, &scope);
        let mut events = mouse_click(pos2(100.0, 100.0));
        events.extend(marked(mouse_click(pos2(220.0, 120.0)), operation));
        assert_eq!(
            scoped(&ctx, &mut owner, 1.0, events, &scope),
            [ClientActivationSource::MouseClick]
        );
        assert_eq!(
            owner.drain_decisions(),
            [
                ClientActivationDecision::Cancelled {
                    source: ClientActivationSource::MouseClick,
                    reason: ClientActivationCancelReason::NotParked
                },
                ClientActivationDecision::Completed(ClientActivationSource::MouseClick),
            ],
            "proof must belong to the actual canonical raw append, never the first matching legacy edge"
        );
    }

    #[test]
    fn unmarked_old_click_cannot_consume_a_marked_clicks_modal_cancellation() {
        let _input = crate::key_input::lock_test_input();
        for modal_before_press in [false, true] {
            let ctx = Context::default();
            let scope = canonical_scope(&ctx);
            let mut owner = DeferredClientActivationGesture::default();
            if modal_before_press {
                owner.set_availability(ClientActivationAvailability::Blocked);
            }
            let operation = feed_scoped_click(&mut owner, &scope);
            if !modal_before_press {
                owner.set_availability(ClientActivationAvailability::Blocked);
            }
            owner.set_availability(ClientActivationAvailability::Available);
            let mut events = mouse_click(pos2(100.0, 100.0));
            events.extend(marked(mouse_click(pos2(220.0, 120.0)), operation));
            assert!(scoped(&ctx, &mut owner, 1.0, events, &scope).is_empty());
            assert_eq!(
                owner.drain_decisions(),
                [
                    ClientActivationDecision::Cancelled {
                        source: ClientActivationSource::MouseClick,
                        reason: ClientActivationCancelReason::NotParked
                    },
                    ClientActivationDecision::Cancelled {
                        source: ClientActivationSource::MouseClick,
                        reason: ClientActivationCancelReason::ModalOwned
                    },
                ]
            );
            let fresh = feed_scoped_click(&mut owner, &scope);
            assert_eq!(
                scoped(
                    &ctx,
                    &mut owner,
                    1.1,
                    marked(mouse_click(pos2(100.0, 100.0)), fresh),
                    &scope
                ),
                [ClientActivationSource::MouseClick]
            );
        }
    }

    #[test]
    fn canonical_proof_waits_for_its_marker_without_authorizing_an_older_frame() {
        let _input = crate::key_input::lock_test_input();
        let ctx = Context::default();
        let scope = canonical_scope(&ctx);
        let mut owner = DeferredClientActivationGesture::default();
        let operation = feed_scoped_click(&mut owner, &scope);
        let pos = pos2(100.0, 100.0);
        assert!(scoped(&ctx, &mut owner, 1.0, mouse_click(pos), &scope).is_empty());
        assert_eq!(
            scoped(
                &ctx,
                &mut owner,
                1.1,
                marked(mouse_click(pos), operation),
                &scope
            ),
            [ClientActivationSource::MouseClick]
        );
        assert_eq!(
            owner.drain_decisions(),
            [
                ClientActivationDecision::Cancelled {
                    source: ClientActivationSource::MouseClick,
                    reason: ClientActivationCancelReason::NotParked
                },
                ClientActivationDecision::Completed(ClientActivationSource::MouseClick),
            ]
        );
    }

    #[test]
    fn orphan_marker_cannot_authorize_another_press_or_a_later_frame() {
        let _input = crate::key_input::lock_test_input();
        let ctx = Context::default();
        let scope = canonical_scope(&ctx);
        let mut owner = DeferredClientActivationGesture::default();
        let operation = feed_scoped_click(&mut owner, &scope);
        let pos = pos2(100.0, 100.0);
        assert!(
            scoped(
                &ctx,
                &mut owner,
                1.0,
                vec![
                    Event::PointerInputProvenance(operation),
                    Event::PointerMoved(pos),
                    primary(pos, true),
                    primary(pos, false)
                ],
                &scope
            )
            .is_empty()
        );
        assert!(scoped(&ctx, &mut owner, 1.1, mouse_click(pos), &scope).is_empty());
        owner.retire_scope();
        assert!(
            owner
                .boundaries
                .iter()
                .all(|boundary| !matches!(boundary, ControlBoundary::ScopedNative { .. }))
        );
        let fresh = feed_scoped_click(&mut owner, &scope);
        assert_eq!(
            scoped(
                &ctx,
                &mut owner,
                1.2,
                marked(mouse_click(pos), fresh),
                &scope
            ),
            [ClientActivationSource::MouseClick]
        );
    }

    #[test]
    fn canonical_cancel_for_another_operation_cannot_end_the_held_click() {
        let _input = crate::key_input::lock_test_input();
        let ctx = Context::default();
        let scope = canonical_scope(&ctx);
        let mut owner = DeferredClientActivationGesture::default();
        let pos = pos2(100.0, 100.0);
        let press = fixture_input(&owner, NativeClientBoundary::Pressed);
        owner.feed_native_scoped_boundary(scope.clone(), press);
        assert!(
            scoped(
                &ctx,
                &mut owner,
                1.0,
                marked(vec![primary(pos, true)], press.operation),
                &scope
            )
            .is_empty()
        );
        owner.feed_native_scoped_boundary(
            scope.clone(),
            NativeClientInputBoundary {
                operation: egui::Id::new("retired-other-operation"),
                boundary: NativeClientBoundary::Cancelled(NativeInputCancelReason::CaptureLost),
            },
        );
        owner.feed_native_scoped_boundary(
            scope.clone(),
            NativeClientInputBoundary {
                operation: press.operation,
                boundary: NativeClientBoundary::Released,
            },
        );
        assert_eq!(
            scoped(&ctx, &mut owner, 1.1, vec![primary(pos, false)], &scope),
            [ClientActivationSource::MouseClick]
        );
        assert_eq!(
            owner.drain_decisions(),
            [ClientActivationDecision::Completed(
                ClientActivationSource::MouseClick
            )]
        );
    }

    #[test]
    fn canonical_multiple_contacts_share_authority_and_end_without_not_parked() {
        let _input = crate::key_input::lock_test_input();
        for raw_only in [false, true] {
            let ctx = Context::default();
            let scope = canonical_scope(&ctx);
            let mut owner = DeferredClientActivationGesture::default();
            let pos = pos2(100.0, 100.0);
            if raw_only {
                let held = fixture_input(&owner, NativeClientBoundary::Pressed);
                owner.feed_native_scoped_boundary(scope.clone(), held);
                assert!(
                    scoped(
                        &ctx,
                        &mut owner,
                        1.0,
                        marked(
                            vec![
                                touch(9, TouchPhase::Start, pos),
                                Event::PointerMoved(pos),
                                primary(pos, true)
                            ],
                            held.operation
                        ),
                        &scope
                    )
                    .is_empty()
                );
                owner.feed_native_scoped_boundary(
                    scope.clone(),
                    NativeClientInputBoundary {
                        operation: held.operation,
                        boundary: NativeClientBoundary::Cancelled(
                            NativeInputCancelReason::CaptureLost,
                        ),
                    },
                );
                assert!(scoped(&ctx, &mut owner, 1.01, Vec::new(), &scope).is_empty());
                owner.drain_decisions();
            }
            let first = fixture_input(&owner, NativeClientBoundary::Pressed);
            owner.feed_native_scoped_boundary(scope.clone(), first);
            let down = if raw_only {
                vec![touch(1, TouchPhase::Start, pos)]
            } else {
                vec![
                    touch(1, TouchPhase::Start, pos),
                    Event::PointerMoved(pos),
                    primary(pos, true),
                ]
            };
            assert!(
                scoped(&ctx, &mut owner, 1.1, marked(down, first.operation), &scope).is_empty()
            );
            // Native owns this same contact gesture: the second finger has no
            // new operation marker, but cannot borrow an unowned gesture.
            assert!(
                scoped(
                    &ctx,
                    &mut owner,
                    1.11,
                    vec![touch(2, TouchPhase::Start, pos2(220.0, 140.0))],
                    &scope
                )
                .is_empty()
            );
            assert!(owner.drain_decisions().is_empty());
            owner.feed_native_scoped_boundary(
                scope.clone(),
                NativeClientInputBoundary {
                    operation: first.operation,
                    boundary: NativeClientBoundary::Released,
                },
            );
            let first_end = if raw_only {
                vec![touch(1, TouchPhase::End, pos)]
            } else {
                vec![
                    touch(1, TouchPhase::End, pos),
                    primary(pos, false),
                    Event::PointerGone,
                ]
            };
            assert!(scoped(&ctx, &mut owner, 1.12, first_end, &scope).is_empty());
            assert!(
                owner.drain_decisions().is_empty(),
                "remaining contact still owns the pinch"
            );
            let last_end = if raw_only {
                vec![touch(2, TouchPhase::End, pos2(220.0, 140.0))]
            } else {
                vec![
                    touch(2, TouchPhase::End, pos2(220.0, 140.0)),
                    Event::PointerGone,
                ]
            };
            assert!(scoped(&ctx, &mut owner, 1.13, last_end, &scope).is_empty());
            assert_eq!(
                owner.drain_decisions(),
                [ClientActivationDecision::Cancelled {
                    source: ClientActivationSource::TouchTap,
                    reason: ClientActivationCancelReason::MultipleTouch,
                }]
            );
            if raw_only {
                assert!(
                    scoped(
                        &ctx,
                        &mut owner,
                        1.14,
                        vec![
                            touch(9, TouchPhase::End, pos),
                            primary(pos, false),
                            Event::PointerGone
                        ],
                        &scope
                    )
                    .is_empty()
                );
            }
            let fresh = feed_scoped_click(&mut owner, &scope);
            assert_eq!(
                scoped(&ctx, &mut owner, 1.2, marked(tap(3, pos), fresh), &scope),
                [ClientActivationSource::TouchTap]
            );
        }
    }

    #[test]
    fn unowned_active_touch_cannot_lend_authority_to_an_additional_contact() {
        let _input = crate::key_input::lock_test_input();
        let ctx = Context::default();
        let scope = canonical_scope(&ctx);
        let mut owner = DeferredClientActivationGesture::default();
        let pos = pos2(100.0, 100.0);
        let events = vec![
            touch(1, TouchPhase::Start, pos),
            Event::PointerMoved(pos),
            primary(pos, true),
            touch(2, TouchPhase::Start, pos2(220.0, 140.0)),
            touch(2, TouchPhase::End, pos2(220.0, 140.0)),
            touch(1, TouchPhase::End, pos),
            primary(pos, false),
            Event::PointerGone,
        ];
        assert!(scoped(&ctx, &mut owner, 1.0, events, &scope).is_empty());
        assert!(owner.drain_decisions().iter().all(|decision| matches!(
            decision,
            ClientActivationDecision::Cancelled {
                source: ClientActivationSource::TouchTap,
                reason: ClientActivationCancelReason::NotParked
            }
        )));
        let fresh = feed_scoped_click(&mut owner, &scope);
        assert_eq!(
            scoped(&ctx, &mut owner, 1.1, marked(tap(3, pos), fresh), &scope),
            [ClientActivationSource::TouchTap]
        );
    }

    #[test]
    fn old_unmarked_tap_and_fresh_marked_tap_only_complete_the_fresh_tap() {
        let _input = crate::key_input::lock_test_input();
        let ctx = Context::default();
        let scope = canonical_scope(&ctx);
        let mut owner = DeferredClientActivationGesture::default();
        let fresh = feed_scoped_click(&mut owner, &scope);
        let mut events = tap(1, pos2(100.0, 100.0));
        events.extend(marked(tap(2, pos2(220.0, 140.0)), fresh));
        assert_eq!(
            scoped(&ctx, &mut owner, 1.0, events, &scope),
            [ClientActivationSource::TouchTap]
        );
        assert_eq!(
            owner.drain_decisions(),
            [
                ClientActivationDecision::Cancelled {
                    source: ClientActivationSource::TouchTap,
                    reason: ClientActivationCancelReason::NotParked
                },
                ClientActivationDecision::Completed(ClientActivationSource::TouchTap),
            ]
        );
    }

    #[test]
    fn old_unowned_mouse_does_not_erase_a_fresh_canonical_tap_completion() {
        let _input = crate::key_input::lock_test_input();
        let ctx = Context::default();
        let scope = canonical_scope(&ctx);
        let mut owner = DeferredClientActivationGesture::default();
        let operation = feed_scoped_click(&mut owner, &scope);
        let mut events = mouse_click(pos2(100.0, 100.0));
        events.extend(marked(tap(1, pos2(220.0, 140.0)), operation));
        assert_eq!(
            scoped(&ctx, &mut owner, 1.0, events, &scope),
            [ClientActivationSource::TouchTap]
        );
        assert_eq!(
            owner.drain_decisions(),
            [
                ClientActivationDecision::Cancelled {
                    source: ClientActivationSource::MouseClick,
                    reason: ClientActivationCancelReason::NotParked
                },
                ClientActivationDecision::Completed(ClientActivationSource::TouchTap),
            ]
        );
    }

    #[test]
    fn old_unowned_tap_completion_cannot_complete_a_fresh_canonical_mouse_owner() {
        let _input = crate::key_input::lock_test_input();
        let ctx = Context::default();
        let scope = canonical_scope(&ctx);
        let mut owner = DeferredClientActivationGesture::default();
        let operation = feed_scoped_click(&mut owner, &scope);
        let mut events = tap(1, pos2(100.0, 100.0));
        events.extend(marked(mouse_click(pos2(220.0, 140.0)), operation));
        assert_eq!(
            scoped(&ctx, &mut owner, 1.0, events, &scope),
            [ClientActivationSource::MouseClick]
        );
        assert_eq!(
            owner.drain_decisions(),
            [
                ClientActivationDecision::Cancelled {
                    source: ClientActivationSource::TouchTap,
                    reason: ClientActivationCancelReason::NotParked
                },
                ClientActivationDecision::Completed(ClientActivationSource::MouseClick),
            ]
        );
    }

    #[test]
    fn canonical_scope_requires_proof_at_raw_only_touch_start() {
        let _input = crate::key_input::lock_test_input();
        let ctx = Context::default();
        let scope = canonical_scope(&ctx);
        let mut owner = DeferredClientActivationGesture::default();
        let pos = pos2(100.0, 100.0);
        assert!(scoped(&ctx, &mut owner, 1.0, tap(1, pos), &scope).is_empty());
        assert_eq!(
            owner.drain_decisions(),
            [ClientActivationDecision::Cancelled {
                source: ClientActivationSource::TouchTap,
                reason: ClientActivationCancelReason::NotParked,
            }]
        );
        let down = fixture_input(&owner, NativeClientBoundary::Pressed);
        owner.feed_native_scoped_boundary(scope.clone(), down);
        assert!(
            scoped(
                &ctx,
                &mut owner,
                1.1,
                marked(
                    vec![
                        touch(2, TouchPhase::Start, pos),
                        Event::PointerMoved(pos),
                        primary(pos, true)
                    ],
                    down.operation
                ),
                &scope
            )
            .is_empty()
        );
        owner.feed_native_scoped_boundary(
            scope.clone(),
            fixture_input(
                &owner,
                NativeClientBoundary::Cancelled(NativeInputCancelReason::CaptureLost),
            ),
        );
        assert!(scoped(&ctx, &mut owner, 1.11, Vec::new(), &scope).is_empty());
        owner.drain_decisions();
        assert!(
            scoped(
                &ctx,
                &mut owner,
                1.12,
                vec![
                    touch(3, TouchPhase::Start, pos),
                    touch(3, TouchPhase::End, pos)
                ],
                &scope
            )
            .is_empty()
        );
        assert_eq!(
            owner.drain_decisions(),
            [ClientActivationDecision::Cancelled {
                source: ClientActivationSource::TouchTap,
                reason: ClientActivationCancelReason::NotParked,
            }]
        );
        let operation = feed_scoped_click(&mut owner, &scope);
        assert_eq!(
            scoped(
                &ctx,
                &mut owner,
                1.13,
                marked(
                    vec![
                        touch(4, TouchPhase::Start, pos),
                        touch(4, TouchPhase::End, pos)
                    ],
                    operation
                ),
                &scope
            ),
            [ClientActivationSource::TouchTap]
        );
        assert_eq!(
            owner.drain_decisions(),
            [ClientActivationDecision::Completed(
                ClientActivationSource::TouchTap
            )]
        );
    }

    #[test]
    fn uncaptured_scoped_press_cannot_survive_a_brief_modal_or_start_inside_it() {
        for press_inside_modal in [false, true] {
            for touch_input in [false, true] {
                let ctx = Context::default();
                let mut owner = DeferredClientActivationGesture::default();
                let scope = scope(1);
                let pos = pos2(100.0, 100.0);
                if press_inside_modal {
                    owner.set_availability(ClientActivationAvailability::Blocked);
                }
                owner.feed_native_scoped_boundary(
                    scope.clone(),
                    fixture_input(&owner, NativeClientBoundary::Pressed),
                );
                if !press_inside_modal {
                    owner.set_availability(ClientActivationAvailability::Blocked);
                }
                owner.set_availability(ClientActivationAvailability::Available);
                owner.feed_native_scoped_boundary(
                    scope.clone(),
                    fixture_input(&owner, NativeClientBoundary::Released),
                );
                let old = if touch_input {
                    tap(1, pos)
                } else {
                    mouse_click(pos)
                };
                assert!(scoped(&ctx, &mut owner, 1.0, old, &scope).is_empty());
                assert!(owner.drain_decisions().iter().any(|decision| matches!(
                    decision,
                    ClientActivationDecision::Cancelled {
                        reason: ClientActivationCancelReason::ModalOwned,
                        ..
                    }
                )));
                feed_scoped_click(&mut owner, &scope);
                let fresh = if touch_input {
                    tap(2, pos)
                } else {
                    mouse_click(pos)
                };
                let source = if touch_input {
                    ClientActivationSource::TouchTap
                } else {
                    ClientActivationSource::MouseClick
                };
                assert_eq!(scoped(&ctx, &mut owner, 1.1, fresh, &scope), [source]);
            }
        }
    }

    #[test]
    fn old_and_current_scoped_clicks_in_one_batch_only_complete_the_current_click() {
        let ctx = Context::default();
        let mut owner = DeferredClientActivationGesture::default();
        let old = scope(1);
        let current = scope(2);
        feed_scoped_click(&mut owner, &old);
        feed_scoped_click(&mut owner, &current);
        let mut events = mouse_click(pos2(100.0, 100.0));
        events.extend(mouse_click(pos2(220.0, 120.0)));
        assert_eq!(
            scoped(&ctx, &mut owner, 1.0, events, &current),
            [ClientActivationSource::MouseClick]
        );
        assert_eq!(
            owner.drain_decisions(),
            [
                ClientActivationDecision::Cancelled {
                    source: ClientActivationSource::MouseClick,
                    reason: ClientActivationCancelReason::Native(
                        NativeInputCancelReason::RetiredRegistration
                    )
                },
                ClientActivationDecision::Completed(ClientActivationSource::MouseClick),
            ]
        );
    }

    #[test]
    fn blocked_scoped_raw_only_tap_stays_cancelled_after_capture_loss() {
        let ctx = Context::default();
        let mut owner = DeferredClientActivationGesture::default();
        let scope = scope(1);
        let pos = pos2(100.0, 100.0);
        owner.feed_native_scoped_boundary(
            scope.clone(),
            fixture_input(&owner, NativeClientBoundary::Pressed),
        );
        assert!(
            scoped(
                &ctx,
                &mut owner,
                1.0,
                vec![
                    touch(1, TouchPhase::Start, pos),
                    Event::PointerMoved(pos),
                    primary(pos, true)
                ],
                &scope
            )
            .is_empty()
        );
        owner.feed_native_scoped_boundary(
            scope.clone(),
            fixture_input(
                &owner,
                NativeClientBoundary::Cancelled(NativeInputCancelReason::CaptureLost),
            ),
        );
        assert!(scoped(&ctx, &mut owner, 1.01, Vec::new(), &scope).is_empty());
        owner.drain_decisions();
        owner.set_availability(ClientActivationAvailability::Blocked);
        owner.feed_native_scoped_boundary(
            scope.clone(),
            fixture_input(&owner, NativeClientBoundary::Pressed),
        );
        owner.set_availability(ClientActivationAvailability::Available);
        owner.feed_native_scoped_boundary(
            scope.clone(),
            fixture_input(&owner, NativeClientBoundary::Released),
        );
        // Contact 1 remains in the backend transport mirror. Contact 2 has no
        // promoted Primary events with which to align its native controls.
        assert!(
            scoped(
                &ctx,
                &mut owner,
                1.02,
                vec![
                    touch(2, TouchPhase::Start, pos),
                    touch(2, TouchPhase::End, pos)
                ],
                &scope
            )
            .is_empty()
        );
        assert_eq!(
            owner.drain_decisions(),
            [ClientActivationDecision::Cancelled {
                source: ClientActivationSource::TouchTap,
                reason: ClientActivationCancelReason::ModalOwned,
            }]
        );
        assert!(
            scoped(
                &ctx,
                &mut owner,
                1.03,
                vec![
                    touch(1, TouchPhase::End, pos),
                    primary(pos, false),
                    Event::PointerGone
                ],
                &scope
            )
            .is_empty()
        );
        feed_scoped_click(&mut owner, &scope);
        assert_eq!(
            scoped(&ctx, &mut owner, 1.04, tap(3, pos), &scope),
            [ClientActivationSource::TouchTap]
        );
    }

    #[test]
    fn pending_promoted_press_cannot_consume_the_next_scoped_native_press() {
        let ctx = Context::default();
        let mut owner = DeferredClientActivationGesture::default();
        let old = scope(1);
        let current = scope(2);
        let pos = pos2(100.0, 100.0);
        owner.feed_native_scoped_boundary(
            old.clone(),
            fixture_input(&owner, NativeClientBoundary::Pressed),
        );
        assert!(
            scoped(
                &ctx,
                &mut owner,
                1.0,
                vec![touch(1, TouchPhase::Start, pos)],
                &old
            )
            .is_empty()
        );
        owner.retire_scope();
        owner.set_availability(ClientActivationAvailability::Blocked);
        owner.feed_native_scoped_boundary(
            current.clone(),
            fixture_input(&owner, NativeClientBoundary::Pressed),
        );
        owner.set_availability(ClientActivationAvailability::Available);
        owner.feed_native_scoped_boundary(
            current.clone(),
            fixture_input(&owner, NativeClientBoundary::Released),
        );
        let mut events = vec![Event::PointerMoved(pos), primary(pos, true)];
        events.extend(mouse_click(pos2(220.0, 120.0)));
        assert!(
            scoped(&ctx, &mut owner, 1.01, events, &current).is_empty(),
            "old pending Primary cannot steal the new blocked press's cancellation"
        );
        feed_scoped_click(&mut owner, &current);
        assert_eq!(
            scoped(&ctx, &mut owner, 1.02, mouse_click(pos), &current),
            [ClientActivationSource::MouseClick]
        );
    }

    #[test]
    fn realmachine55_parked_long_mouse_press_completes_on_release_once() {
        let ctx = Context::default();
        let mut owner = DeferredClientActivationGesture::default();
        let scope = scope(55);
        let pos = pos2(100.0, 100.0);
        owner.feed_native_scoped_boundary(
            scope.clone(),
            fixture_input(&owner, NativeClientBoundary::Pressed),
        );
        assert!(scoped(&ctx, &mut owner, 1.0, vec![primary(pos, true)], &scope).is_empty());
        assert!(scoped(&ctx, &mut owner, 2.0, Vec::new(), &scope).is_empty());
        owner.feed_native_scoped_boundary(
            scope.clone(),
            fixture_input(&owner, NativeClientBoundary::Released),
        );
        assert_eq!(
            scoped(&ctx, &mut owner, 2.5, vec![primary(pos, false)], &scope),
            [ClientActivationSource::MouseClick],
            "a stationary 1.5-second press resumes the parked owner on release"
        );
        assert_eq!(
            owner.drain_scoped_decisions(),
            [ScopedClientActivationDecision {
                scope: Some(scope.clone()),
                decision: ClientActivationDecision::Completed(ClientActivationSource::MouseClick),
            }]
        );
        assert!(scoped(&ctx, &mut owner, 2.6, Vec::new(), &scope).is_empty());
        assert!(owner.drain_decisions().is_empty());
    }

    #[test]
    fn realmachine55_parked_long_touch_press_completes_on_release_once() {
        let ctx = Context::default();
        let mut owner = DeferredClientActivationGesture::default();
        let scope = scope(55);
        let pos = pos2(100.0, 100.0);
        owner.feed_native_scoped_boundary(
            scope.clone(),
            fixture_input(&owner, NativeClientBoundary::Pressed),
        );
        assert!(
            scoped(
                &ctx,
                &mut owner,
                1.0,
                vec![
                    touch(1, TouchPhase::Start, pos),
                    Event::PointerMoved(pos),
                    primary(pos, true),
                ],
                &scope,
            )
            .is_empty()
        );
        assert!(scoped(&ctx, &mut owner, 2.0, Vec::new(), &scope).is_empty());
        owner.feed_native_scoped_boundary(
            scope.clone(),
            fixture_input(&owner, NativeClientBoundary::Released),
        );
        assert_eq!(
            scoped(
                &ctx,
                &mut owner,
                2.5,
                vec![
                    touch(1, TouchPhase::End, pos),
                    primary(pos, false),
                    Event::PointerGone,
                ],
                &scope,
            ),
            [ClientActivationSource::TouchTap],
            "the correlated 1.5-second touch resumes only its captured parked scope"
        );
        assert_eq!(
            owner.drain_scoped_decisions(),
            [ScopedClientActivationDecision {
                scope: Some(scope.clone()),
                decision: ClientActivationDecision::Completed(ClientActivationSource::TouchTap),
            }]
        );
        assert!(scoped(&ctx, &mut owner, 2.6, Vec::new(), &scope).is_empty());
        assert!(owner.drain_decisions().is_empty());
    }

    #[test]
    fn realmachine55_fresh_touch_close_cannot_complete_an_old_unmatched_activation() {
        let _input = crate::key_input::lock_test_input();
        for old_release_time in [1.1, 2.5] {
            let ctx = Context::default();
            let scope = canonical_scope(&ctx);
            let mut owner = DeferredClientActivationGesture::default();
            let pos = pos2(100.0, 100.0);
            let old = fixture_input(&owner, NativeClientBoundary::Pressed);
            owner.feed_native_scoped_boundary(scope.clone(), old);
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    1.0,
                    marked(
                        vec![
                            touch(1, TouchPhase::Start, pos),
                            Event::PointerMoved(pos),
                            primary(pos, true),
                        ],
                        old.operation,
                    ),
                    &scope,
                    ClientActivationAvailability::Available,
                    false,
                )
                .is_empty()
            );
            owner.feed_native_scoped_boundary(
                scope.clone(),
                NativeClientInputBoundary {
                    operation: old.operation,
                    boundary: NativeClientBoundary::Released,
                },
            );
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    old_release_time,
                    vec![touch(1, TouchPhase::End, pos)],
                    &scope,
                    ClientActivationAvailability::Available,
                    false,
                )
                .is_empty()
            );
            assert!(owner.drain_decisions().is_empty());
            let new = feed_scoped_click(&mut owner, &scope);
            let mut events = vec![Event::PointerMoved(pos2(200.0, 150.0))];
            events.extend(marked(tap(2, close_rect().center()), new));
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    old_release_time + 0.1,
                    events,
                    &scope,
                    ClientActivationAvailability::Available,
                    false,
                )
                .is_empty(),
                "a new close must not inherit the old activation destination"
            );
            assert_eq!(
                owner.drain_decisions(),
                [
                    ClientActivationDecision::Cancelled {
                        source: ClientActivationSource::TouchTap,
                        reason: ClientActivationCancelReason::PointerTakeover,
                    },
                    ClientActivationDecision::CompletedClose(ClientActivationSource::TouchTap),
                ]
            );
        }
    }

    #[test]
    fn realmachine55_mouse_release_without_move_uses_the_existing_drag_distance() {
        for distance in [6.0, 6.01] {
            let ctx = Context::default();
            let mut owner = DeferredClientActivationGesture::default();
            let scope = scope(55);
            let pos = pos2(100.0, 100.0);
            owner.feed_native_scoped_boundary(
                scope.clone(),
                fixture_input(&owner, NativeClientBoundary::Pressed),
            );
            assert!(scoped(&ctx, &mut owner, 1.0, vec![primary(pos, true)], &scope).is_empty());
            owner.feed_native_scoped_boundary(
                scope.clone(),
                fixture_input(&owner, NativeClientBoundary::Released),
            );
            let result = scoped(
                &ctx,
                &mut owner,
                2.5,
                vec![primary(pos + egui::vec2(distance, 0.0), false)],
                &scope,
            );
            if distance <= 6.0 {
                assert_eq!(result, [ClientActivationSource::MouseClick]);
            } else {
                assert!(result.is_empty());
                assert_eq!(
                    owner.drain_decisions(),
                    [ClientActivationDecision::Cancelled {
                        source: ClientActivationSource::MouseClick,
                        reason: ClientActivationCancelReason::Drag,
                    }]
                );
            }
        }
    }

    #[test]
    fn realmachine55_long_drag_still_cancels_with_the_actual_typed_reason() {
        for touch_input in [false, true] {
            let ctx = Context::default();
            let mut owner = DeferredClientActivationGesture::default();
            let scope = scope(55);
            let pos = pos2(100.0, 100.0);
            let away = pos2(150.0, 100.0);
            let press = fixture_input(&owner, NativeClientBoundary::Pressed);
            owner.feed_native_scoped_boundary(scope.clone(), press);
            let start = if touch_input {
                vec![
                    touch(1, TouchPhase::Start, pos),
                    Event::PointerMoved(pos),
                    primary(pos, true),
                ]
            } else {
                vec![Event::PointerMoved(pos), primary(pos, true)]
            };
            assert!(scoped(&ctx, &mut owner, 1.0, start, &scope).is_empty());
            let motion = if touch_input {
                vec![
                    touch(1, TouchPhase::Move, away),
                    Event::PointerMoved(away),
                    touch(1, TouchPhase::Move, pos),
                    Event::PointerMoved(pos),
                ]
            } else {
                vec![Event::PointerMoved(away), Event::PointerMoved(pos)]
            };
            assert!(scoped(&ctx, &mut owner, 2.0, motion, &scope).is_empty());
            owner.feed_native_scoped_boundary(
                scope.clone(),
                NativeClientInputBoundary {
                    operation: press.operation,
                    boundary: NativeClientBoundary::Released,
                },
            );
            let end = if touch_input {
                vec![
                    touch(1, TouchPhase::End, pos),
                    primary(pos, false),
                    Event::PointerGone,
                ]
            } else {
                vec![primary(pos, false)]
            };
            assert!(scoped(&ctx, &mut owner, 2.5, end, &scope).is_empty());
            assert_eq!(
                owner.drain_scoped_decisions(),
                [ScopedClientActivationDecision {
                    scope: Some(scope),
                    decision: ClientActivationDecision::Cancelled {
                        source: if touch_input {
                            ClientActivationSource::TouchTap
                        } else {
                            ClientActivationSource::MouseClick
                        },
                        reason: ClientActivationCancelReason::Drag,
                    },
                }]
            );
        }
    }

    #[test]
    fn realmachine55_long_press_does_not_change_window_close_duration_policy() {
        for touch_input in [false, true] {
            let ctx = Context::default();
            let mut owner = DeferredClientActivationGesture::default();
            let scope = scope(55);
            let pos = close_rect().center();
            let start = if touch_input {
                vec![
                    touch(1, TouchPhase::Start, pos),
                    Event::PointerMoved(pos),
                    primary(pos, true),
                ]
            } else {
                vec![Event::PointerMoved(pos), primary(pos, true)]
            };
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    1.0,
                    start,
                    &scope,
                    ClientActivationAvailability::Available,
                    false,
                )
                .is_empty()
            );
            let end = if touch_input {
                vec![
                    touch(1, TouchPhase::End, pos),
                    primary(pos, false),
                    Event::PointerGone,
                ]
            } else {
                vec![primary(pos, false)]
            };
            assert!(
                close_frame(
                    &ctx,
                    &mut owner,
                    2.5,
                    end,
                    &scope,
                    ClientActivationAvailability::Available,
                    false,
                )
                .is_empty()
            );
            let decisions = owner.drain_decisions();
            assert_eq!(decisions.len(), 1);
            assert!(matches!(
                decisions[0],
                ClientActivationDecision::Cancelled { .. }
            ));
        }
    }

    #[test]
    fn coalesced_mouse_click_is_completed_once_even_after_layout_discard() {
        let ctx = Context::default();
        let mut owner = DeferredClientActivationGesture::default();
        assert_eq!(
            drive(
                &ctx,
                &mut owner,
                1.0,
                mouse_click(pos2(100.0, 100.0)),
                ClientActivationAvailability::Available,
                true
            ),
            [ClientActivationSource::MouseClick]
        );
        assert_eq!(
            owner.drain_decisions(),
            [ClientActivationDecision::Completed(
                ClientActivationSource::MouseClick
            )]
        );
        assert!(available(&ctx, &mut owner, 1.1, Vec::new()).is_empty());
    }

    #[test]
    fn held_mouse_waits_for_release_and_egui_rejects_an_out_and_back_drag() {
        for drag in [false, true] {
            let ctx = Context::default();
            let mut owner = DeferredClientActivationGesture::default();
            let pos = pos2(100.0, 100.0);
            assert!(available(&ctx, &mut owner, 1.0, vec![primary(pos, true)]).is_empty());
            if drag {
                assert!(
                    available(
                        &ctx,
                        &mut owner,
                        1.01,
                        vec![
                            Event::PointerMoved(pos2(250.0, 100.0)),
                            Event::PointerMoved(pos)
                        ]
                    )
                    .is_empty()
                );
            }
            let completed = available(&ctx, &mut owner, 1.02, vec![primary(pos, false)]);
            assert_eq!(
                completed,
                if drag {
                    vec![]
                } else {
                    vec![ClientActivationSource::MouseClick]
                }
            );
        }
    }

    #[test]
    fn raw_touch_and_promoted_pointer_produce_one_tap_and_never_a_mouse_click() {
        let ctx = Context::default();
        let mut owner = DeferredClientActivationGesture::default();
        assert_eq!(
            drive(
                &ctx,
                &mut owner,
                1.0,
                tap(1, pos2(100.0, 100.0)),
                ClientActivationAvailability::Available,
                true
            ),
            [ClientActivationSource::TouchTap]
        );
        assert_eq!(
            owner.drain_decisions(),
            [ClientActivationDecision::Completed(
                ClientActivationSource::TouchTap
            )]
        );
    }

    #[test]
    fn raw_end_and_promoted_release_may_span_callback_frames() {
        let ctx = Context::default();
        let mut owner = DeferredClientActivationGesture::default();
        let pos = pos2(100.0, 100.0);
        assert!(
            available(
                &ctx,
                &mut owner,
                1.0,
                vec![
                    touch(1, TouchPhase::Start, pos),
                    Event::PointerMoved(pos),
                    primary(pos, true)
                ]
            )
            .is_empty()
        );
        assert!(available(&ctx, &mut owner, 1.01, vec![touch(1, TouchPhase::End, pos)]).is_empty());
        assert_eq!(
            available(
                &ctx,
                &mut owner,
                1.02,
                vec![primary(pos, false), Event::PointerGone]
            ),
            [ClientActivationSource::TouchTap]
        );
    }

    #[test]
    fn touch_drag_and_multiple_contacts_do_not_activate_and_fresh_tap_still_works() {
        for multiple in [false, true] {
            let ctx = Context::default();
            let mut owner = DeferredClientActivationGesture::default();
            let pos = pos2(100.0, 100.0);
            let other = pos2(220.0, 150.0);
            let mut events = vec![
                touch(1, TouchPhase::Start, pos),
                Event::PointerMoved(pos),
                primary(pos, true),
            ];
            if multiple {
                events.extend([
                    touch(2, TouchPhase::Start, other),
                    touch(2, TouchPhase::End, other),
                ]);
            } else {
                events.extend([
                    touch(1, TouchPhase::Move, other),
                    Event::PointerMoved(other),
                ]);
            }
            let end = if multiple { pos } else { other };
            events.extend([
                touch(1, TouchPhase::End, end),
                primary(end, false),
                Event::PointerGone,
            ]);
            assert!(available(&ctx, &mut owner, 1.0, events).is_empty());
            assert_eq!(
                available(&ctx, &mut owner, 1.1, tap(3, pos)),
                [ClientActivationSource::TouchTap]
            );
        }
    }

    #[test]
    fn touch_cancel_does_not_complete_and_does_not_poison_the_next_tap() {
        let ctx = Context::default();
        let mut owner = DeferredClientActivationGesture::default();
        let pos = pos2(100.0, 100.0);
        assert!(
            available(
                &ctx,
                &mut owner,
                1.0,
                vec![
                    touch(1, TouchPhase::Start, pos),
                    Event::PointerMoved(pos),
                    primary(pos, true)
                ]
            )
            .is_empty()
        );
        assert!(
            available(
                &ctx,
                &mut owner,
                1.01,
                vec![touch(1, TouchPhase::Cancel, pos), Event::PointerGone]
            )
            .is_empty()
        );
        assert!(
            owner
                .drain_decisions()
                .contains(&ClientActivationDecision::Cancelled {
                    source: ClientActivationSource::TouchTap,
                    reason: ClientActivationCancelReason::TouchCancelled,
                })
        );
        assert_eq!(
            available(&ctx, &mut owner, 1.02, tap(2, pos)),
            [ClientActivationSource::TouchTap]
        );
    }

    #[test]
    fn chrome_and_command_owners_reserve_their_whole_gesture() {
        for touch_input in [false, true] {
            for availability in [
                ClientActivationAvailability::Available,
                ClientActivationAvailability::CommandOwned,
                ClientActivationAvailability::Blocked,
            ] {
                let ctx = Context::default();
                let mut owner = DeferredClientActivationGesture::default();
                let pos = if availability == ClientActivationAvailability::Available {
                    pos2(100.0, 20.0)
                } else {
                    pos2(100.0, 100.0)
                };
                let events = if touch_input {
                    tap(1, pos)
                } else {
                    mouse_click(pos)
                };
                assert!(drive(&ctx, &mut owner, 1.0, events, availability, false).is_empty());
                assert!(owner.drain_decisions().iter().all(|decision| matches!(
                    decision,
                    ClientActivationDecision::Cancelled { .. }
                )));
            }
        }
    }

    #[test]
    fn brief_modal_between_down_and_up_cannot_restore_the_old_gesture() {
        for touch_input in [false, true] {
            let ctx = Context::default();
            let mut owner = DeferredClientActivationGesture::default();
            let pos = pos2(100.0, 100.0);
            let down = if touch_input {
                vec![
                    touch(1, TouchPhase::Start, pos),
                    Event::PointerMoved(pos),
                    primary(pos, true),
                ]
            } else {
                vec![primary(pos, true)]
            };
            assert!(available(&ctx, &mut owner, 1.0, down).is_empty());
            owner.set_availability(ClientActivationAvailability::Blocked);
            owner.set_availability(ClientActivationAvailability::Available);
            let up = if touch_input {
                vec![
                    touch(1, TouchPhase::End, pos),
                    primary(pos, false),
                    Event::PointerGone,
                ]
            } else {
                vec![primary(pos, false)]
            };
            assert!(available(&ctx, &mut owner, 1.01, up).is_empty());
            assert!(
                owner
                    .drain_decisions()
                    .contains(&ClientActivationDecision::Cancelled {
                        source: if touch_input {
                            ClientActivationSource::TouchTap
                        } else {
                            ClientActivationSource::MouseClick
                        },
                        reason: ClientActivationCancelReason::ModalOwned,
                    })
            );
            let fresh = if touch_input {
                tap(2, pos)
            } else {
                mouse_click(pos)
            };
            let source = if touch_input {
                ClientActivationSource::TouchTap
            } else {
                ClientActivationSource::MouseClick
            };
            assert_eq!(available(&ctx, &mut owner, 1.02, fresh), [source]);
        }
    }

    #[test]
    fn scope_retirement_precedes_an_already_queued_native_release() {
        for touch_input in [false, true] {
            let ctx = Context::default();
            let mut owner = DeferredClientActivationGesture::default();
            let pos = pos2(100.0, 100.0);
            owner.feed_native_boundary(NativeClientBoundary::Pressed);
            let down = if touch_input {
                vec![
                    touch(1, TouchPhase::Start, pos),
                    Event::PointerMoved(pos),
                    primary(pos, true),
                ]
            } else {
                vec![primary(pos, true)]
            };
            assert!(available(&ctx, &mut owner, 1.0, down).is_empty());
            // Exact old failure: native Released queues before the registration
            // changes, while the corresponding raw Up has not been captured.
            owner.feed_native_boundary(NativeClientBoundary::Released);
            owner.retire_scope();
            let source = if touch_input {
                ClientActivationSource::TouchTap
            } else {
                ClientActivationSource::MouseClick
            };
            assert_eq!(
                owner.drain_decisions(),
                [ClientActivationDecision::Cancelled {
                    source,
                    reason: ClientActivationCancelReason::Native(
                        NativeInputCancelReason::RetiredRegistration
                    ),
                }]
            );
            let up = if touch_input {
                vec![
                    touch(1, TouchPhase::End, pos),
                    primary(pos, false),
                    Event::PointerGone,
                ]
            } else {
                vec![primary(pos, false)]
            };
            assert!(available(&ctx, &mut owner, 1.01, up).is_empty());
            assert!(
                owner.drain_decisions().is_empty(),
                "retired input cannot complete or cancel twice"
            );
            let fresh = if touch_input {
                tap(2, pos)
            } else {
                mouse_click(pos)
            };
            assert_eq!(available(&ctx, &mut owner, 1.02, fresh), [source]);
        }
    }

    #[test]
    fn native_cancel_between_old_down_and_new_click_preserves_the_new_click() {
        let _input = crate::key_input::lock_test_input();
        let ctx = Context::default();
        let scope = canonical_scope(&ctx);
        let mut owner = DeferredClientActivationGesture::default();
        let old = fixture_input(&owner, NativeClientBoundary::Pressed);
        owner.feed_native_scoped_boundary(scope.clone(), old);
        owner.feed_native_scoped_boundary(
            scope.clone(),
            NativeClientInputBoundary {
                operation: old.operation,
                boundary: NativeClientBoundary::Cancelled(NativeInputCancelReason::CaptureLost),
            },
        );
        let fresh = feed_scoped_click(&mut owner, &scope);
        let pos = pos2(100.0, 100.0);
        let mut events = marked(vec![primary(pos, true)], old.operation);
        events.extend(marked(mouse_click(pos2(200.0, 100.0)), fresh));
        // Production metadata belongs to an operation, so capture loss retires
        // the old press before the fresh click's cursor motion reaches it.
        assert_eq!(
            scoped(&ctx, &mut owner, 1.0, events, &scope),
            [ClientActivationSource::MouseClick]
        );
        assert_eq!(
            owner.drain_scoped_decisions(),
            [
                ScopedClientActivationDecision {
                    scope: Some(scope.clone()),
                    decision: ClientActivationDecision::Cancelled {
                        source: ClientActivationSource::MouseClick,
                        reason: ClientActivationCancelReason::Native(
                            NativeInputCancelReason::CaptureLost
                        ),
                    },
                },
                ScopedClientActivationDecision {
                    scope: Some(scope.clone()),
                    decision: ClientActivationDecision::Completed(
                        ClientActivationSource::MouseClick
                    ),
                },
            ]
        );
        assert!(scoped(&ctx, &mut owner, 1.1, Vec::new(), &scope).is_empty());
        assert!(owner.drain_decisions().is_empty());
    }

    #[test]
    fn native_cancel_between_old_touch_down_and_new_tap_is_ordered_in_one_batch() {
        let ctx = Context::default();
        let mut owner = DeferredClientActivationGesture::default();
        let pos = pos2(100.0, 100.0);
        for boundary in [
            NativeClientBoundary::Pressed,
            NativeClientBoundary::Cancelled(NativeInputCancelReason::CaptureLost),
            NativeClientBoundary::Pressed,
            NativeClientBoundary::Released,
        ] {
            owner.feed_native_boundary(boundary);
        }
        // A contact identifier can be reused after the old capture ends. The
        // native cancellation must precede the fresh raw Start, not merely its
        // promoted Primary press two events later.
        let mut events = vec![
            touch(1, TouchPhase::Start, pos),
            Event::PointerMoved(pos),
            primary(pos, true),
        ];
        events.extend(tap(1, pos));
        assert_eq!(
            available(&ctx, &mut owner, 1.0, events),
            [ClientActivationSource::TouchTap]
        );
        assert_eq!(
            owner.drain_decisions(),
            [
                ClientActivationDecision::Cancelled {
                    source: ClientActivationSource::TouchTap,
                    reason: ClientActivationCancelReason::Native(
                        NativeInputCancelReason::CaptureLost
                    ),
                },
                ClientActivationDecision::Completed(ClientActivationSource::TouchTap),
            ]
        );
    }

    #[test]
    fn native_capture_loss_cancels_touch_recognition_before_its_old_end() {
        let ctx = Context::default();
        let mut owner = DeferredClientActivationGesture::default();
        let pos = pos2(100.0, 100.0);
        owner.feed_native_boundary(NativeClientBoundary::Pressed);
        assert!(
            available(
                &ctx,
                &mut owner,
                1.0,
                vec![
                    touch(1, TouchPhase::Start, pos),
                    Event::PointerMoved(pos),
                    primary(pos, true)
                ]
            )
            .is_empty()
        );
        owner.feed_native_boundary(NativeClientBoundary::Cancelled(
            NativeInputCancelReason::CaptureLost,
        ));
        owner.feed_native_boundary(NativeClientBoundary::Released);
        assert!(
            available(
                &ctx,
                &mut owner,
                1.01,
                vec![
                    touch(1, TouchPhase::End, pos),
                    primary(pos, false),
                    Event::PointerGone
                ]
            )
            .is_empty()
        );
        assert_eq!(
            available(&ctx, &mut owner, 1.02, tap(2, pos)),
            [ClientActivationSource::TouchTap]
        );
    }

    #[test]
    fn touch_capture_loss_retires_old_contact_without_resetting_backend_transport() {
        let ctx = Context::default();
        let mut owner = DeferredClientActivationGesture::default();
        let pos = pos2(100.0, 100.0);
        assert!(
            available(
                &ctx,
                &mut owner,
                1.0,
                vec![
                    touch(1, TouchPhase::Start, pos),
                    Event::PointerMoved(pos),
                    primary(pos, true)
                ]
            )
            .is_empty()
        );
        owner.feed_native_boundary(NativeClientBoundary::Cancelled(
            NativeInputCancelReason::CaptureLost,
        ));
        // egui-winit still owns contact 1 in its independent transport mirror:
        // a new contact has raw Touch events, with no promoted pointer stream.
        assert_eq!(
            available(
                &ctx,
                &mut owner,
                1.01,
                vec![
                    touch(2, TouchPhase::Start, pos),
                    touch(2, TouchPhase::End, pos)
                ]
            ),
            [ClientActivationSource::TouchTap]
        );
        assert!(
            available(
                &ctx,
                &mut owner,
                1.02,
                vec![
                    touch(1, TouchPhase::End, pos),
                    primary(pos, false),
                    Event::PointerGone
                ]
            )
            .is_empty()
        );
        assert_eq!(
            available(&ctx, &mut owner, 1.03, tap(3, pos)),
            [ClientActivationSource::TouchTap]
        );
    }

    #[test]
    fn native_boundary_metadata_focus_and_keyboard_never_activate_without_a_click() {
        let ctx = Context::default();
        let mut owner = DeferredClientActivationGesture::default();
        owner.feed_native_boundary(NativeClientBoundary::Pressed);
        owner.feed_native_boundary(NativeClientBoundary::Released);
        assert!(
            available(
                &ctx,
                &mut owner,
                1.0,
                vec![
                    Event::WindowFocused(true),
                    Event::Key {
                        key: egui::Key::D,
                        physical_key: Some(egui::Key::D),
                        pressed: true,
                        repeat: false,
                        modifiers: Modifiers::NONE,
                    }
                ]
            )
            .is_empty()
        );
        assert!(owner.drain_decisions().is_empty());
    }

    #[test]
    fn lost_button_state_and_focus_loss_reject_later_orphan_releases() {
        for focus_loss in [false, true] {
            let ctx = Context::default();
            let mut owner = DeferredClientActivationGesture::default();
            let pos = pos2(100.0, 100.0);
            assert!(available(&ctx, &mut owner, 1.0, vec![primary(pos, true)]).is_empty());
            if focus_loss {
                assert!(
                    available(&ctx, &mut owner, 1.01, vec![Event::WindowFocused(false)]).is_empty()
                );
            } else {
                ctx.cancel_pointer_button_for(
                    ctx.viewport_registration(egui::ViewportId::ROOT).unwrap(),
                    PointerButton::Primary,
                    egui::Id::unique("activation-test"),
                );
                assert!(available(&ctx, &mut owner, 1.01, Vec::new()).is_empty());
            }
            assert!(
                available(
                    &ctx,
                    &mut owner,
                    1.02,
                    vec![Event::WindowFocused(true), primary(pos, false)]
                )
                .is_empty()
            );
            assert_eq!(
                available(&ctx, &mut owner, 1.03, mouse_click(pos)),
                [ClientActivationSource::MouseClick]
            );
        }
    }
}
