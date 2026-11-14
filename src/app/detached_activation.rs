//! Discrete activation facts share the existing callback transport and intent owner.
//! Neither focus metadata nor an OS button level is an activation request.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DetachedActivationSource {
    ClientClick,
    TouchTap,
    NonClientClick,
    NativeMedia,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DetachedActivationDecision {
    Accepted,
    Committed,
    ModalOwned,
    NotParked,
    RetiredRegistration,
    GestureCancelled,
    CloseOwned,
    CommandOwned,
    ActivationFailed,
}

#[derive(Clone, Debug)]
pub(crate) struct DetachedActivationRequest {
    pub(crate) window_id: u64,
    pub(crate) registration: Option<egui::ViewportRegistration>,
    pub(crate) context_id: Option<ViewerContextId>,
    pub(crate) source: DetachedActivationSource,
    pub(crate) allocation: Option<crate::key_input::NativeInputAllocationWitness>,
}

impl PartialEq for DetachedActivationRequest {
    fn eq(&self, other: &Self) -> bool {
        self.window_id == other.window_id
            && self.registration == other.registration
            && self.context_id == other.context_id
            && self.source == other.source
            && match (&self.allocation, &other.allocation) {
                (None, None) => true,
                (Some(a), Some(b)) => a.same_allocation(b),
                _ => false,
            }
    }
}

impl DetachedActivationRequest {
    pub(crate) fn log(&self, boundary: &str, decision: DetachedActivationDecision) {
        crate::logger::log(format!(
            "[detached-activation] boundary={boundary} source={:?} decision={decision:?} window={} registration={:?} context={:?}",
            self.source, self.window_id, self.registration, self.context_id,
        ));
    }
}

impl App {
    pub(crate) fn detached_activation_request(
        &self,
        ctx: &egui::Context,
        window_id: u64,
        source: DetachedActivationSource,
    ) -> DetachedActivationRequest {
        let registration =
            ctx.viewport_registration(Self::detached_image_window_viewport_id(window_id));
        DetachedActivationRequest {
            window_id,
            registration,
            context_id: self.locate_window_context(window_id).map(|(id, _)| id),
            source,
            allocation: registration.and_then(crate::key_input::native_input_allocation_witness),
        }
    }

    pub(crate) fn detached_activation_admission(
        &self,
        ctx: &egui::Context,
        request: &DetachedActivationRequest,
    ) -> DetachedActivationDecision {
        use DetachedActivationDecision::*;
        if request.registration.is_some_and(|registration| {
            registration.viewport_id() != Self::detached_image_window_viewport_id(request.window_id)
                || ctx.viewport_registration(registration.viewport_id()) != Some(registration)
        }) || request
            .allocation
            .as_ref()
            .is_some_and(|allocation| !allocation.is_current())
            || self
                .locate_window_context(request.window_id)
                .map(|(id, _)| id)
                != request.context_id
        {
            return RetiredRegistration;
        }
        if self.common_modal_dialog_open()
            || self.document_open_modal_admission_blocked()
            || self.sidecar_restore_blocks_window(request.window_id)
        {
            return ModalOwned;
        }
        let expected = if request.source == DetachedActivationSource::NativeMedia {
            DetachedWindowState::ParkedLive
        } else {
            DetachedWindowState::Parked
        };
        if self.detached_window_state(request.window_id) != Some(expected)
            || !self.detached_window_can_activate(request.window_id)
        {
            return NotParked;
        }
        // Production requires C's native input allocation. Only headless tests
        // without a backend can exercise the logical callback transport alone.
        #[cfg(not(test))]
        let require_native_allocation = true;
        #[cfg(test)]
        let require_native_allocation = eframe::native_window_input::native_window_allocation(
            ctx,
            Self::detached_image_window_viewport_id(request.window_id),
        )
        .is_some();
        if request.source != DetachedActivationSource::NativeMedia
            && (request.registration.is_none()
                || (require_native_allocation && request.allocation.is_none()))
        {
            return RetiredRegistration;
        }
        Accepted
    }

    pub(crate) fn accept_discrete_detached_activation(
        &mut self,
        ctx: &egui::Context,
        request: DetachedActivationRequest,
    ) -> DetachedActivationDecision {
        let mut decision = self.detached_activation_admission(ctx, &request);
        if decision == DetachedActivationDecision::Accepted {
            if !self
                .detached_window_manager
                .queue_discrete_activation(request.clone())
            {
                decision = DetachedActivationDecision::CommandOwned;
            }
        }
        request.log("accept", decision);
        decision
    }

    /// Publish logical listeners after viewport registration, including before
    /// native allocation. C attaches them to the exact input-allocation epoch.
    pub(crate) fn sync_detached_native_input_targets(&self, ctx: &egui::Context) {
        use crate::key_input::*;
        let mut targets = Vec::new();
        for (id, shared) in &self.deferred_detached_image_window_views {
            if self.detached_window_state(*id) != Some(DetachedWindowState::Parked) {
                continue;
            }
            let Some(registration) =
                ctx.viewport_registration(Self::detached_image_window_viewport_id(*id))
            else {
                continue;
            };
            let blocked = self.common_modal_dialog_open()
                || self.document_open_modal_admission_blocked()
                || self.sidecar_restore_blocks_window(*id);
            shared.set_activation_availability(if blocked {
                detached_activation_gesture::ClientActivationAvailability::Blocked
            } else {
                detached_activation_gesture::ClientActivationAvailability::Available
            });
            let request = self.detached_activation_request(
                ctx,
                *id,
                DetachedActivationSource::NonClientClick,
            );
            let mut client_scope = request.clone();
            client_scope.source = DetachedActivationSource::ClientClick;
            shared.set_client_activation_scope(client_scope);
            let shared = Arc::clone(shared);
            let wake = ctx.clone();
            targets.push(NativeDetachedInputTarget {
                registration,
                availability: if blocked {
                    NativeTargetAvailability::Blocked
                } else {
                    NativeTargetAvailability::Enabled
                },
                sink: Arc::new(move |event| {
                    match event.kind {
                        NativeDetachedInputEventKind::ClientBoundary(boundary) => {
                            let mut origin = request.clone();
                            origin.source = DetachedActivationSource::ClientClick;
                            origin.registration = Some(event.source.registration);
                            origin.allocation = Some(event.source.epoch);
                            match boundary.boundary {
                                NativeClientBoundary::Pressed => {
                                    origin.log("normalized", DetachedActivationDecision::Accepted)
                                }
                                NativeClientBoundary::Released => origin.log(
                                    "normalized_release",
                                    DetachedActivationDecision::Accepted,
                                ),
                                NativeClientBoundary::Cancelled(reason) => origin.log(
                                    "normalized_discard",
                                    match reason {
                                        NativeInputCancelReason::ModalOwned => {
                                            DetachedActivationDecision::ModalOwned
                                        }
                                        NativeInputCancelReason::RetiredRegistration => {
                                            DetachedActivationDecision::RetiredRegistration
                                        }
                                        _ => DetachedActivationDecision::GestureCancelled,
                                    },
                                ),
                            }
                            shared.feed_native_activation_boundary(origin, boundary);
                        }
                        NativeDetachedInputEventKind::NonClient(outcome) => {
                            let mut request = request.clone();
                            request.registration = Some(event.source.registration);
                            request.allocation = Some(event.source.epoch);
                            let decision = match outcome {
                                NativeNonClientOutcome::Completed => {
                                    DetachedActivationDecision::Accepted
                                }
                                NativeNonClientOutcome::CloseOwned => {
                                    DetachedActivationDecision::CloseOwned
                                }
                                NativeNonClientOutcome::Cancelled(
                                    NativeInputCancelReason::ModalOwned,
                                ) => DetachedActivationDecision::ModalOwned,
                                NativeNonClientOutcome::Cancelled(
                                    NativeInputCancelReason::RetiredRegistration,
                                ) => DetachedActivationDecision::RetiredRegistration,
                                NativeNonClientOutcome::Cancelled(_) => {
                                    DetachedActivationDecision::GestureCancelled
                                }
                            };
                            request.log("normalized", decision);
                            if decision == DetachedActivationDecision::Accepted {
                                shared.push_event(DeferredDetachedImageWindowEvent::Activation {
                                    request,
                                });
                                // Only a request's transport owns root wake.
                                wake.request_repaint_of(egui::ViewportId::ROOT);
                            }
                        }
                    }
                }),
            });
        }
        set_native_detached_input_targets(targets);
    }
}

#[cfg(test)]
mod tests {
    use super::super::detached_activation_gesture::*;
    use super::*;

    #[test]
    fn client_press_cannot_be_reassigned_to_a_replacement_context() {
        let ctx = egui::Context::default();
        let mut capture = DeferredDetachedClientCapture::default();
        let mut request = DetachedActivationRequest {
            window_id: 7,
            registration: None,
            context_id: Some(ViewerContextId::for_test(1)),
            source: DetachedActivationSource::ClientClick,
            allocation: None,
        };
        let geometry = crate::touch_input::TapZoneGeometry {
            surface: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0)),
            excluded: Vec::new(),
            behavior: crate::touch_input::TouchSurfaceBehavior::Viewer {
                accepts_pinch: true,
                tap_zones: true,
            },
        };
        let pos = egui::pos2(100.0, 100.0);
        for pressed in [true, false] {
            if !pressed {
                request.context_id = Some(ViewerContextId::for_test(2));
            }
            capture.set_scope(request.clone());
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(geometry.surface),
                    events: vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ui| {
                    assert!(
                        capture
                            .gesture
                            .capture(
                                ui.ctx(),
                                geometry.clone(),
                                ClientActivationAvailability::Available
                            )
                            .is_none()
                    );
                },
            )
            .drop_without_applying_deltas();
        }
        assert!(
            capture
                .gesture
                .drain_decisions()
                .iter()
                .any(|decision| matches!(
                    decision,
                    ClientActivationDecision::Cancelled {
                        reason: ClientActivationCancelReason::Native(
                            crate::key_input::NativeInputCancelReason::RetiredRegistration
                        ),
                        ..
                    }
                ))
        );
    }

    #[test]
    fn stale_discrete_request_is_rejected_without_recreating_a_runtime() {
        let mut app = super::super::tests::phase_c_support::setup_app();
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        let request = DetachedActivationRequest {
            window_id: 99,
            registration: ctx.viewport_registration(egui::ViewportId::ROOT),
            context_id: None,
            source: DetachedActivationSource::ClientClick,
            allocation: None,
        };
        assert_eq!(
            app.accept_discrete_detached_activation(&ctx, request),
            DetachedActivationDecision::RetiredRegistration
        );
        assert!(app.detached_window_state(99).is_none());
    }

    #[test]
    fn uncaptured_old_native_press_is_not_relabelled_but_new_click_in_same_batch_survives() {
        use crate::key_input::NativeClientBoundary::*;
        let _input = crate::key_input::lock_test_input();
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        let registration = ctx.viewport_registration(egui::ViewportId::ROOT).unwrap();
        let _allocation =
            crate::key_input::register_test_native_input_allocation(registration, 0x7675_1101);
        let allocation = crate::key_input::native_input_allocation_witness(registration).unwrap();
        let mut capture = DeferredDetachedClientCapture::default();
        let old = DetachedActivationRequest {
            window_id: 7,
            registration: Some(registration),
            context_id: Some(ViewerContextId::for_test(1)),
            source: DetachedActivationSource::ClientClick,
            allocation: Some(allocation),
        };
        let mut current = old.clone();
        current.context_id = Some(ViewerContextId::for_test(2));
        capture.set_scope(current.clone());
        let old_id = egui::Id::new("old-native-press");
        let fresh_id = egui::Id::new("new-native-press");
        for (origin, operation, boundary) in [
            (old.clone(), old_id, Pressed),
            (old, old_id, Released),
            (current.clone(), fresh_id, Pressed),
            (current.clone(), fresh_id, Released),
        ] {
            capture.gesture.feed_native_scoped_boundary(
                origin,
                crate::key_input::NativeClientInputBoundary {
                    operation,
                    boundary,
                },
            );
        }
        let pos = egui::pos2(100.0, 100.0);
        let mut events = Vec::new();
        for operation in [old_id, fresh_id] {
            events.push(egui::Event::PointerInputProvenance(operation));
            for pressed in [true, false] {
                events.push(egui::Event::PointerButton {
                    pos,
                    pressed,
                    button: egui::PointerButton::Primary,
                    modifiers: egui::Modifiers::NONE,
                });
            }
        }
        let geometry = crate::touch_input::TapZoneGeometry {
            surface: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0)),
            excluded: Vec::new(),
            behavior: crate::touch_input::TouchSurfaceBehavior::Viewer {
                accepts_pinch: true,
                tap_zones: true,
            },
        };
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(geometry.surface),
                events,
                ..Default::default()
            },
            |ui| {
                assert_eq!(
                    capture.capture(
                        ui.ctx(),
                        geometry.clone(),
                        ClientActivationAvailability::Available,
                        current.clone()
                    ),
                    Some(ClientActivationSource::MouseClick)
                );
            },
        )
        .drop_without_applying_deltas();
        let decisions = capture.gesture.drain_decisions();
        assert_eq!(
            decisions
                .iter()
                .filter(|d| matches!(d, ClientActivationDecision::Completed(_)))
                .count(),
            1
        );
        assert!(decisions.iter().any(|d| matches!(
            d,
            ClientActivationDecision::Cancelled {
                reason: ClientActivationCancelReason::Native(
                    crate::key_input::NativeInputCancelReason::RetiredRegistration
                ),
                ..
            }
        )));
    }
}
