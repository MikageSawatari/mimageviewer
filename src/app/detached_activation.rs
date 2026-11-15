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

/// Expected residence belongs to the request's creation boundary. Input source
/// is diagnostic provenance and does not decide which parked state it owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DetachedActivationTargetState {
    Parked,
    ParkedLive,
    Unavailable,
}

impl DetachedActivationTargetState {
    fn from_runtime(state: Option<DetachedWindowState>) -> Self {
        match state {
            Some(DetachedWindowState::Parked) => Self::Parked,
            Some(DetachedWindowState::ParkedLive) => Self::ParkedLive,
            _ => Self::Unavailable,
        }
    }

    fn matches(self, state: Option<DetachedWindowState>) -> bool {
        matches!(
            (self, state),
            (Self::Parked, Some(DetachedWindowState::Parked))
                | (Self::ParkedLive, Some(DetachedWindowState::ParkedLive))
        )
    }
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
    MinimizeOwned,
    CommandOwned,
    ActivationFailed,
}

#[derive(Clone, Debug)]
pub(crate) struct DetachedActivationRequest {
    pub(crate) window_id: u64,
    pub(crate) registration: Option<egui::ViewportRegistration>,
    pub(crate) context_id: Option<ViewerContextId>,
    pub(crate) source: DetachedActivationSource,
    pub(crate) expected_state: DetachedActivationTargetState,
    pub(crate) allocation: Option<crate::key_input::NativeInputAllocationWitness>,
}

impl PartialEq for DetachedActivationRequest {
    fn eq(&self, other: &Self) -> bool {
        self.window_id == other.window_id
            && self.registration == other.registration
            && self.context_id == other.context_id
            && self.source == other.source
            && self.expected_state == other.expected_state
            && match (&self.allocation, &other.allocation) {
                (None, None) => true,
                (Some(a), Some(b)) => a.same_allocation(b),
                _ => false,
            }
    }
}

impl DetachedActivationRequest {
    pub(crate) fn log(&self, boundary: &str, decision: DetachedActivationDecision) {
        crate::logger::log(self.diagnostic_line(boundary, decision, None));
    }

    pub(crate) fn log_client_cancelled(
        &self,
        boundary: &str,
        decision: DetachedActivationDecision,
        reason: super::detached_activation_gesture::ClientActivationCancelReason,
    ) {
        crate::logger::log(self.diagnostic_line(boundary, decision, Some(&reason)));
    }

    fn diagnostic_line(
        &self,
        boundary: &str,
        decision: DetachedActivationDecision,
        reason: Option<&dyn std::fmt::Debug>,
    ) -> String {
        let reason = reason
            .map(|reason| format!(" reason={reason:?}"))
            .unwrap_or_default();
        format!(
            "[detached-activation] boundary={boundary} source={:?} decision={decision:?}{reason} window={} registration={:?} context={:?} expected_state={:?}",
            self.source, self.window_id, self.registration, self.context_id, self.expected_state,
        )
    }

    fn native_outcome_diagnostic(
        &self,
        boundary: &str,
        outcome: crate::key_input::NativeNonClientOutcome,
    ) -> (DetachedActivationDecision, String) {
        use crate::key_input::{
            NativeInputCancelReason as Reason, NativeNonClientOutcome as Outcome,
        };
        let decision = match outcome {
            Outcome::Completed => DetachedActivationDecision::Accepted,
            Outcome::CloseOwned => DetachedActivationDecision::CloseOwned,
            Outcome::MinimizeOwned => DetachedActivationDecision::MinimizeOwned,
            Outcome::Cancelled(Reason::ModalOwned) => DetachedActivationDecision::ModalOwned,
            Outcome::Cancelled(Reason::RetiredRegistration) => {
                DetachedActivationDecision::RetiredRegistration
            }
            Outcome::Cancelled(_) => DetachedActivationDecision::GestureCancelled,
        };
        let line = match outcome {
            Outcome::Cancelled(reason) => self.diagnostic_line(boundary, decision, Some(&reason)),
            _ => self.diagnostic_line(boundary, decision, Some(&None::<Reason>)),
        };
        (decision, line)
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
            expected_state: DetachedActivationTargetState::from_runtime(
                self.detached_window_state(window_id),
            ),
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
        if !request
            .expected_state
            .matches(self.detached_window_state(request.window_id))
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
            if !matches!(
                self.detached_window_state(*id),
                Some(DetachedWindowState::Parked | DetachedWindowState::ParkedLive)
            ) {
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
                kind: if request.expected_state == DetachedActivationTargetState::ParkedLive {
                    NativeDetachedInputTargetKind::ParkedLive
                } else {
                    NativeDetachedInputTargetKind::Parked
                },
                availability: if blocked {
                    NativeTargetAvailability::Blocked
                } else {
                    NativeTargetAvailability::Enabled
                },
                sink: Arc::new(move |event| {
                    match event.kind {
                        NativeDetachedInputEventKind::ClientBoundary(boundary) => {
                            // ParkedLive's existing immediate/native media
                            // producer retains client activation ownership.
                            if request.expected_state != DetachedActivationTargetState::Parked {
                                return;
                            }
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
                        NativeDetachedInputEventKind::NonClient(outcome)
                        | NativeDetachedInputEventKind::NativeMediaTouch(outcome) => {
                            let mut request = request.clone();
                            if matches!(
                                event.kind,
                                NativeDetachedInputEventKind::NativeMediaTouch(_)
                            ) {
                                request.source = DetachedActivationSource::NativeMedia;
                            }
                            request.registration = Some(event.source.registration);
                            request.allocation = Some(event.source.epoch);
                            let (decision, line) =
                                request.native_outcome_diagnostic("normalized", outcome);
                            crate::logger::log(line);
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
    fn native_detached_normalized_diagnostic_keeps_cancel_reason_and_owner() {
        use crate::key_input::{
            NativeInputCancelReason as Reason, NativeNonClientOutcome as Outcome,
        };
        for source in [
            DetachedActivationSource::NonClientClick,
            DetachedActivationSource::NativeMedia,
        ] {
            let request = DetachedActivationRequest {
                window_id: 56,
                registration: None,
                context_id: Some(ViewerContextId::for_test(7)),
                source,
                expected_state: DetachedActivationTargetState::ParkedLive,
                allocation: None,
            };
            for reason in [
                Reason::ModalOwned,
                Reason::CaptureLost,
                Reason::PointerCancelled,
                Reason::CancelMode,
                Reason::FocusLost,
                Reason::Escape,
                Reason::CaptionButtonOutside,
                Reason::RetiredRegistration,
            ] {
                let (decision, line) =
                    request.native_outcome_diagnostic("normalized", Outcome::Cancelled(reason));
                assert_eq!(
                    decision,
                    match reason {
                        Reason::ModalOwned => DetachedActivationDecision::ModalOwned,
                        Reason::RetiredRegistration =>
                            DetachedActivationDecision::RetiredRegistration,
                        _ => DetachedActivationDecision::GestureCancelled,
                    }
                );
                assert!(
                    line.contains(&format!(
                        "source={source:?} decision={decision:?} reason={reason:?}"
                    )),
                    "{line}"
                );
                assert!(
                    line.contains("boundary=normalized") && line.contains("window=56"),
                    "{line}"
                );
                assert!(
                    line.contains("context=Some(ViewerContextId(7))")
                        && line.contains("expected_state=ParkedLive"),
                    "{line}"
                );
            }
            for (outcome, expected) in [
                (Outcome::Completed, DetachedActivationDecision::Accepted),
                (Outcome::CloseOwned, DetachedActivationDecision::CloseOwned),
                (
                    Outcome::MinimizeOwned,
                    DetachedActivationDecision::MinimizeOwned,
                ),
            ] {
                let (decision, line) = request.native_outcome_diagnostic("normalized", outcome);
                assert_eq!(decision, expected);
                assert!(line.contains("reason=None"), "{line}");
            }
        }
    }

    fn realmachine55_parked_media_fixture() -> (
        super::super::tests::phase_c_support::AppTestEnv,
        egui::Context,
        u64,
    ) {
        use super::super::detached_media_exchange_tests as media;
        let mut app = super::super::tests::phase_c_support::setup_app();
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let root = app.tmp.path().to_path_buf();
        media::install_items(&mut app, &ctx, &root, false);
        media::open_grid(&mut app, &ctx, 0);
        let id = app.detached_viewer_window_id().unwrap();
        media::open_grid(&mut app, &ctx, 3);
        assert_eq!(
            app.detached_window_state(id),
            Some(DetachedWindowState::ParkedLive)
        );
        egui::Context::set_immediate_viewport_renderer(|ctx, mut viewport| {
            let id = viewport.ids.this;
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
            let mut input = egui::RawInput {
                viewport_id: id,
                screen_rect: Some(rect),
                focused: false,
                ..Default::default()
            };
            let info = input.viewports.entry(id).or_default();
            info.parent = Some(egui::ViewportId::ROOT);
            info.focused = Some(false);
            info.outer_rect = Some(rect);
            info.inner_rect = Some(rect);
            ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui))
                .drop_without_applying_deltas();
        });
        assert!(!app.deferred_detached_image_window_views.contains_key(&id));
        ctx.run_ui(egui::RawInput::default(), |ui| {
            app.render_detached_image_windows(ui.ctx())
        })
        .drop_without_applying_deltas();
        assert!(
            app.deferred_detached_image_window_views.contains_key(&id),
            "the real ParkedLive render branch must create and retain its native event transport"
        );
        app.prune_deferred_detached_image_window_views();
        assert!(app.deferred_detached_image_window_views.contains_key(&id));
        (app, ctx, id)
    }

    #[test]
    fn realmachine58_minimize_outcome_keeps_parked_owner_and_emits_no_restore_or_focus() {
        use windows::Win32::UI::WindowsAndMessaging::{
            HTMINBUTTON, SC_MINIMIZE, WM_NCLBUTTONDOWN, WM_SYSCOMMAND,
        };
        let _input = crate::key_input::lock_test_input();
        for live in [false, true] {
            let (mut app, ctx, id) = if live {
                realmachine55_parked_media_fixture()
            } else {
                use super::super::detached_media_exchange_tests as media;
                let mut app = super::super::tests::phase_c_support::setup_app();
                let ctx = egui::Context::default();
                ctx.set_embed_viewports(false);
                let root = app.tmp.path().to_path_buf();
                media::install_items(&mut app, &ctx, &root, false);
                app.settings.detached_viewer_enabled = true;
                app.settings.detached_viewer_open_images_in_window = true;
                media::open_grid(&mut app, &ctx, 2);
                let id = app.active_detached_window_id().unwrap();
                media::open_grid(&mut app, &ctx, 3);
                assert_eq!(
                    app.detached_window_state(id),
                    Some(DetachedWindowState::Parked)
                );
                ctx.run_ui(egui::RawInput::default(), |ui| {
                    app.render_detached_image_windows(ui.ctx())
                })
                .drop_without_applying_deltas();
                (app, ctx, id)
            };
            let active_before = app.active_viewer_context_id();
            let window_before = app.active_detached_window_id();
            let owner_before = app.locate_window_context(id);
            let state_before = app.detached_window_state(id);
            let focus_before = app.detached_viewer_focus_requested;
            let registration = ctx
                .viewport_registration(App::detached_image_window_viewport_id(id))
                .unwrap();
            let allocation = crate::key_input::register_test_native_input_allocation(
                registration,
                0x8059 + u64::from(live),
            );
            app.sync_detached_native_input_targets(&ctx);
            let request =
                app.detached_activation_request(&ctx, id, DetachedActivationSource::NonClientClick);
            assert_eq!(
                app.detached_activation_admission(&ctx, &request),
                DetachedActivationDecision::Accepted,
                "the native input must address a live, activatable parked owner"
            );
            allocation.native_message_for_test(WM_NCLBUTTONDOWN, HTMINBUTTON as usize, 0);
            allocation.native_message_for_test(WM_SYSCOMMAND, SC_MINIMIZE as usize, 0);
            allocation.finish_native_dispatch_for_test(WM_SYSCOMMAND);
            allocation.finish_native_dispatch_for_test(WM_NCLBUTTONDOWN);
            // Native transport is drained before the next UI pass, as in the
            // real handler tests. An empty ROOT-only pass would retire the
            // parked registration before its request could be admitted.
            app.service_deferred_detached_window_events(&ctx);
            let committed = app.commit_pending_deferred_detached_window_activation(&ctx);
            assert!(
                !committed,
                "minimizing a parked window must not enter the activation commit"
            );
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                app.render_detached_image_windows(ui.ctx());
            });
            assert_eq!(app.active_viewer_context_id(), active_before);
            assert_eq!(app.active_detached_window_id(), window_before);
            assert_eq!(app.locate_window_context(id), owner_before);
            assert_eq!(app.detached_window_state(id), state_before);
            assert_eq!(app.detached_viewer_focus_requested, focus_before);
            assert!(!app.detached_window_deferred_activation_pending(id));
            assert!(
                !output
                    .viewport_output
                    .values()
                    .any(|viewport| viewport.commands.iter().any(|command| matches!(
                        command,
                        egui::ViewportCommand::Focus | egui::ViewportCommand::Minimized(false)
                    )))
            );
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn realmachine55_parked_live_nonclient_is_admitted_without_changing_native_media() {
        let _input = crate::key_input::lock_test_input();
        let (app, ctx, id) = realmachine55_parked_media_fixture();
        for source in [
            DetachedActivationSource::NativeMedia,
            DetachedActivationSource::NonClientClick,
        ] {
            let request = app.detached_activation_request(&ctx, id, source);
            assert_eq!(
                app.detached_activation_admission(&ctx, &request),
                DetachedActivationDecision::Accepted,
                "both producers preserve the ParkedLive intent: {source:?}"
            );
        }
    }

    #[test]
    fn realmachine55_parked_live_native_target_reaches_existing_transport() {
        use egui_winit::pointer_input_observer::{
            NormalizedPointerInput as Phase, PointerInputSource as Source,
        };
        let _input = crate::key_input::lock_test_input();
        let (app, ctx, id) = realmachine55_parked_media_fixture();
        let registration = ctx
            .viewport_registration(App::detached_image_window_viewport_id(id))
            .unwrap();
        let allocation =
            crate::key_input::register_test_native_input_allocation(registration, 0x7553);
        app.sync_detached_native_input_targets(&ctx);
        assert!(
            allocation
                .normalized_input(Phase::Pressed(Source::Mouse))
                .is_some(),
            "ParkedLive must publish its exact native allocation input target"
        );
        allocation.normalized_input(Phase::Released(Source::Mouse));
        assert!(
            app.deferred_detached_image_window_views[&id]
                .drain_events()
                .is_empty(),
            "client activation remains owned by the existing NativeMedia producer"
        );
    }

    #[test]
    fn realmachine55_parked_live_caption_button_and_doubleclick_resume_same_context() {
        use windows::Win32::UI::WindowsAndMessaging::{
            HTCAPTION, HTLEFT, HTMAXBUTTON, SC_MAXIMIZE, WM_CAPTURECHANGED, WM_MOVING,
            WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_SIZING, WM_SYSCOMMAND,
        };
        let _input = crate::key_input::lock_test_input();
        for (start, hit, movement) in [
            (WM_NCLBUTTONDOWN, HTCAPTION, None),
            (WM_NCLBUTTONDOWN, HTMAXBUTTON, None),
            (WM_NCLBUTTONDBLCLK, HTCAPTION, None),
            (WM_NCLBUTTONDOWN, HTCAPTION, Some(WM_MOVING)),
            (WM_NCLBUTTONDOWN, HTLEFT, Some(WM_SIZING)),
        ] {
            let (mut app, ctx, id) = realmachine55_parked_media_fixture();
            let context = app.locate_window_context(id).unwrap().0;
            let registration = ctx
                .viewport_registration(App::detached_image_window_viewport_id(id))
                .unwrap();
            let allocation =
                crate::key_input::register_test_native_input_allocation(registration, 0x7554);
            app.sync_detached_native_input_targets(&ctx);
            allocation.native_message_for_test(start, hit as usize, 0);
            if let Some(movement) = movement {
                let mut rect = windows::Win32::Foundation::RECT {
                    left: -800,
                    top: -700,
                    right: 1000,
                    bottom: 900,
                };
                allocation.native_message_for_test(movement, 0, (&mut rect as *mut _) as isize);
            }
            if hit == HTMAXBUTTON {
                allocation.native_message_for_test(WM_SYSCOMMAND, SC_MAXIMIZE as usize, 0);
            } else {
                allocation.native_message_for_test(WM_CAPTURECHANGED, 0, 0);
            }
            allocation.finish_native_dispatch_for_test(start);
            let shared = Arc::clone(&app.deferred_detached_image_window_views[&id]);
            let events = shared.drain_events();
            assert_eq!(events.len(), 1, "one NC operation has one request");
            for event in events {
                let DeferredDetachedImageWindowEvent::Activation { request } = &event else {
                    panic!("NC operation must use existing activation transport");
                };
                assert_eq!(request.source, DetachedActivationSource::NonClientClick);
                assert_eq!(
                    request.expected_state,
                    DetachedActivationTargetState::ParkedLive
                );
                shared.push_event(event);
            }
            app.service_deferred_detached_window_events(&ctx);
            assert!(app.detached_window_deferred_activation_pending(id));
            assert!(app.commit_pending_deferred_detached_window_activation(&ctx));
            assert_eq!(app.active_detached_window_id(), Some(id));
            assert_eq!(app.active_viewer_context_id(), Some(context));
            assert!(
                !app.detached_image_windows
                    .iter()
                    .any(|window| window.id == id)
            );
            app.prune_deferred_detached_image_window_views();
            assert!(!app.deferred_detached_image_window_views.contains_key(&id));
            assert!(!app.commit_pending_deferred_detached_window_activation(&ctx));
        }
    }

    #[test]
    fn realmachine55_parked_live_request_does_not_follow_a_later_runtime_state() {
        let _input = crate::key_input::lock_test_input();
        let (mut app, ctx, id) = realmachine55_parked_media_fixture();
        let request =
            app.detached_activation_request(&ctx, id, DetachedActivationSource::NonClientClick);
        app.transition_detached_window_state(id, DetachedWindowState::Parked, "test_late_nc");
        assert_eq!(
            app.detached_activation_admission(&ctx, &request),
            DetachedActivationDecision::NotParked
        );
    }

    #[test]
    fn realmachine55_parked_live_native_canvas_touch_drag_resumes_original_context_once() {
        use crate::video::native_touch::NativeTouchOwnership;
        use crate::video::native_window::{
            NativeVideoTouchEvent, NativeVideoTouchPhase as Phase, NativeVideoWindowEventSink,
            NativeVideoWindowSource as Source, native_window_event_route,
        };
        let _input = crate::key_input::lock_test_input();
        let (mut app, ctx, id) = realmachine55_parked_media_fixture();
        let context = app.locate_window_context(id).unwrap().0;
        let registration = ctx
            .viewport_registration(App::detached_image_window_viewport_id(id))
            .unwrap();
        let _allocation =
            crate::key_input::register_test_native_input_allocation(registration, 0x7559);
        app.sync_detached_native_input_targets(&ctx);
        let fault = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (pump, _pump_rx) = native_window_event_route(16, Arc::clone(&fault));
        pump.publish_native_input_epoch(31);
        let (render, render_rx) = native_window_event_route(16, fault);
        let sink = NativeVideoWindowEventSink::new(31, 31, Source::Presenter, pump, render)
            .with_detached_activation_host(
                0x7559,
                Arc::new(std::sync::atomic::AtomicU64::new(1)),
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            );
        let mut ownership = NativeTouchOwnership::default();
        for (phase, x, y) in [
            (Phase::Start, 100, 200),
            (Phase::Move, 900, 800),
            (Phase::End, 1000, 900),
        ] {
            sink.owned_touch_for_test(
                &mut ownership,
                NativeVideoTouchEvent {
                    source: Source::Presenter,
                    pointer_id: 7,
                    x,
                    y,
                    phase,
                    suppress_widget_primary: false,
                },
            );
        }
        let shared = Arc::clone(&app.deferred_detached_image_window_views[&id]);
        let events = shared.drain_events();
        assert_eq!(
            events.len(),
            1,
            "native canvas End must complete one captured ParkedLive activation"
        );
        assert!(
            render_rx.drain().is_empty(),
            "ActivateOnly cannot reach normal touch commands"
        );
        for event in events {
            let DeferredDetachedImageWindowEvent::Activation { request } = &event else {
                panic!("native canvas completion must use the existing activation transport");
            };
            assert_eq!(request.source, DetachedActivationSource::NativeMedia);
            assert_eq!(
                request.expected_state,
                DetachedActivationTargetState::ParkedLive
            );
            shared.push_event(event);
        }
        app.service_deferred_detached_window_events(&ctx);
        assert!(app.commit_pending_deferred_detached_window_activation(&ctx));
        assert_eq!(app.active_detached_window_id(), Some(id));
        assert_eq!(app.active_viewer_context_id(), Some(context));
        assert!(!app.commit_pending_deferred_detached_window_activation(&ctx));
    }

    #[test]
    fn client_press_cannot_be_reassigned_to_a_replacement_context() {
        let ctx = egui::Context::default();
        let mut capture = DeferredDetachedClientCapture::default();
        let mut request = DetachedActivationRequest {
            window_id: 7,
            registration: None,
            context_id: Some(ViewerContextId::for_test(1)),
            source: DetachedActivationSource::ClientClick,
            expected_state: DetachedActivationTargetState::Parked,
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
            expected_state: DetachedActivationTargetState::Parked,
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
            expected_state: DetachedActivationTargetState::Parked,
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
