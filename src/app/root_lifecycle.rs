//! Root Close policy, independent of key input and dialog rendering.
//!
//! The old driver keeps acceptance and tray effects at their original checkpoints.
//! The snapshot belongs to one call, never to App or a viewer context.
use super::App;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RootCloseIntent {
    WindowClose,
    ApplicationQuit,
    TrayHide,
}

pub(crate) struct RootLifecycleSnapshot {
    intent: Option<RootCloseIntent>,
    modal_owner: Option<&'static str>,
    sidecar_rejected: bool,
    zip_deferred: bool,
    process_deferred: bool,
}

impl App {
    /// Accept this delivery once, before any terminal polls alter its modal owners.
    pub(crate) fn accept_root_lifecycle_before_frame(
        &mut self,
        ctx: &egui::Context,
    ) -> RootLifecycleSnapshot {
        // RawInput remains readable by admission checks during the old UI driver.
        // The backend owns delivery consumption; this checkpoint accepts once per delivery.
        let close = ctx.input_for(egui::ViewportId::ROOT, |input| {
            input
                .raw
                .viewports
                .get(&egui::ViewportId::ROOT)
                .is_some_and(|root| root.close_requested())
        });
        let intent = close.then(|| {
            if self.sidecar_restore_root_lifecycle_close_requested() {
                RootCloseIntent::ApplicationQuit
            } else {
                RootCloseIntent::WindowClose
            }
        });
        let snapshot = self.accept_root_lifecycle_request(ctx, intent);
        self.resume_composition_editor_root_close(ctx);
        snapshot
    }

    fn accept_root_lifecycle_request(
        &mut self,
        ctx: &egui::Context,
        intent: Option<RootCloseIntent>,
    ) -> RootLifecycleSnapshot {
        let modal_owner = self.root_tray_hide_modal_owner();
        let sidecar_rejected = intent.is_some_and(|intent| {
            !matches!(intent, RootCloseIntent::ApplicationQuit) && self.sidecar_restore_active()
        });
        let mut snapshot = RootLifecycleSnapshot {
            intent,
            modal_owner,
            sidecar_rejected,
            zip_deferred: false,
            process_deferred: false,
        };
        if let Some(intent) = intent {
            // These are independent existing owners: a single close must reach all of them.
            snapshot.process_deferred = self.accept_settings_family_root_close(ctx, intent);
            snapshot.process_deferred |= self.accept_composition_editor_root_close(ctx);
            snapshot.zip_deferred = self.accept_image_folder_zip_root_close(ctx, intent);
            if sidecar_rejected {
                ctx.send_viewport_cmd_to(
                    egui::ViewportId::ROOT,
                    egui::ViewportCommand::CancelClose,
                );
            }
        }
        snapshot
    }

    /// Called after the old tray request checkpoint; no root input is re-read here.
    pub(crate) fn service_root_lifecycle_tray_checkpoint(
        &mut self,
        ctx: &egui::Context,
        snapshot: RootLifecycleSnapshot,
    ) {
        let Some(intent) = snapshot.intent else {
            return;
        };
        if snapshot.zip_deferred || snapshot.sidecar_rejected {
            return;
        }
        self.intercept_root_close_request(
            ctx,
            matches!(intent, RootCloseIntent::ApplicationQuit)
                || self.sidecar_restore_root_lifecycle_close_requested(),
            matches!(intent, RootCloseIntent::TrayHide),
            snapshot.modal_owner,
        );
    }

    /// Existing operation owners hand their one consumed continuation back to this policy.
    pub(crate) fn resume_root_lifecycle_close(
        &mut self,
        ctx: &egui::Context,
        intent: RootCloseIntent,
    ) {
        let intent = if self.sidecar_restore_root_lifecycle_close_requested() {
            RootCloseIntent::ApplicationQuit
        } else {
            intent
        };
        if matches!(intent, RootCloseIntent::ApplicationQuit) {
            self.shutdown_requested
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let snapshot = self.accept_root_lifecycle_request(ctx, Some(intent));
        let blocked =
            snapshot.process_deferred || snapshot.zip_deferred || snapshot.sidecar_rejected;
        // A resumed ordinary Close still crosses modal/tray policy. It never becomes Quit.
        let tray_close = !matches!(intent, RootCloseIntent::ApplicationQuit)
            && (matches!(intent, RootCloseIntent::TrayHide)
                || (self.settings.minimize_to_tray_on_close && self.tray_controller.is_some()));
        self.service_root_lifecycle_tray_checkpoint(ctx, snapshot);
        if !blocked && !tray_close {
            self.request_main_window_close(ctx);
        }
    }

    /// Non-drawing entry for lifecycle regression and the later logic-only connection.
    /// Live display/adoption polls are deliberately absent.
    pub(crate) fn service_root_lifecycle(&mut self, ctx: &egui::Context) {
        let snapshot = self.accept_root_lifecycle_before_frame(ctx);
        self.service_root_lifecycle_tray_checkpoint(ctx, snapshot);
        self.poll_composition_editor_save_receipts(ctx);
        self.resume_composition_editor_root_close(ctx);
        self.service_settings_family_operation(ctx);
        self.service_image_folder_zip_exit(ctx);
        self.service_discarded_sidecar_restore(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_headless(
        ctx: &egui::Context,
        input: egui::RawInput,
        run: impl FnMut(&egui::Context),
    ) -> egui::FullOutput {
        let mut output = ctx.run_pass(input, run);
        // These lifecycle assertions inspect commands and have no renderer.
        output.textures_delta.clear();
        output
    }

    fn close_input() -> egui::RawInput {
        let mut raw = egui::RawInput::default();
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        raw
    }

    #[test]
    fn egui037_continuation_root_lifecycle_tray_once_and_explicit_quit() {
        for explicit in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            app.settings.minimize_to_tray_on_close = true;
            app.tray_controller = Some(crate::tray::TrayController::controller_for_test());
            app.shutdown_requested
                .store(explicit, std::sync::atomic::Ordering::SeqCst);
            let ctx = egui::Context::default();
            let output = run_headless(&ctx, close_input(), |ctx| {
                let snapshot = app.accept_root_lifecycle_before_frame(ctx);
                app.service_root_lifecycle_tray_checkpoint(ctx, snapshot);
                assert!(ctx.input_for(egui::ViewportId::ROOT, |i| i.viewport().close_requested()));
            });
            let cancelled = output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .is_some_and(|v| v.commands.contains(&egui::ViewportCommand::CancelClose));
            assert_eq!(cancelled, !explicit);
            assert_eq!(app.window_visible, explicit);
            let output = run_headless(&ctx, egui::RawInput::default(), |ctx| {
                app.service_root_lifecycle(ctx)
            });
            assert!(
                !output
                    .viewport_output
                    .get(&egui::ViewportId::ROOT)
                    .is_some_and(|v| v.commands.contains(&egui::ViewportCommand::Close))
            );
            assert_eq!(app.window_visible, explicit);
        }
    }

    #[test]
    fn egui037_continuation_root_lifecycle_modal_rejection_is_not_deferred() {
        let mut app = crate::app::setup_app_for_test();
        let folder = app.tmp.path().join("book");
        app.activate_sidecar_restore_modal_for_test(folder);
        let ctx = egui::Context::default();
        let output = run_headless(&ctx, close_input(), |ctx| app.service_root_lifecycle(ctx));
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::CancelClose)
        );
        let output = run_headless(&ctx, egui::RawInput::default(), |ctx| {
            app.service_root_lifecycle(ctx)
        });
        assert!(
            !output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .is_some_and(|v| v.commands.contains(&egui::ViewportCommand::Close))
        );
        assert!(app.window_visible);
    }

    #[test]
    fn egui037_continuation_root_lifecycle_plain_close_does_not_add_a_barrier() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.minimize_to_tray_on_close = false;
        let ctx = egui::Context::default();
        let output = run_headless(&ctx, close_input(), |ctx| app.service_root_lifecycle(ctx));
        assert!(
            !output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .is_some_and(|v| v.commands.contains(&egui::ViewportCommand::CancelClose))
        );
        assert!(app.window_visible);
    }
    #[test]
    fn egui037_continuation_root_lifecycle_sent_save_resumes_once_without_ui() {
        for (status, late_quit) in [
            (Ok(()), false),
            (Err("fake save failure".to_owned()), false),
            (Ok(()), true),
        ] {
            let mut app = crate::app::setup_app_for_test();
            app.settings.minimize_to_tray_on_close = false;
            let complete = app.sent_composition_save_for_lifecycle_test();
            let frame = app.frame_counter;
            let input_seq = app.input_seq;
            let ctx = egui::Context::default();
            let blocked = run_headless(&ctx, close_input(), |ctx| app.service_root_lifecycle(ctx));
            assert!(
                blocked.viewport_output[&egui::ViewportId::ROOT]
                    .commands
                    .contains(&egui::ViewportCommand::CancelClose)
            );
            assert!(app.composition_editor_writes_pending());
            if late_quit {
                app.shutdown_requested
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                let quit = run_headless(&ctx, close_input(), |ctx| app.service_root_lifecycle(ctx));
                assert!(
                    quit.viewport_output[&egui::ViewportId::ROOT]
                        .commands
                        .contains(&egui::ViewportCommand::CancelClose)
                );
            }
            complete(status);
            let resumed = run_headless(&ctx, egui::RawInput::default(), |ctx| {
                app.service_root_lifecycle(ctx)
            });
            assert!(!app.composition_editor_writes_pending());
            assert_eq!(
                app.shutdown_requested
                    .load(std::sync::atomic::Ordering::SeqCst),
                late_quit
            );
            assert_eq!(
                resumed.viewport_output[&egui::ViewportId::ROOT]
                    .commands
                    .iter()
                    .filter(|command| matches!(command, egui::ViewportCommand::Close))
                    .count(),
                1
            );
            let again = run_headless(&ctx, egui::RawInput::default(), |ctx| {
                app.service_root_lifecycle(ctx)
            });
            assert!(
                !again
                    .viewport_output
                    .get(&egui::ViewportId::ROOT)
                    .is_some_and(|v| v.commands.contains(&egui::ViewportCommand::Close))
            );
            assert_eq!(app.frame_counter, frame);
            assert_eq!(app.input_seq, input_seq);
        }
    }
    #[test]
    fn root_lifecycle_actual_app_logic_reaches_backend_close_decision_without_ui() {
        for tray in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            app.settings.minimize_to_tray_on_close = tray;
            if tray {
                app.tray_controller = Some(crate::tray::TrayController::controller_for_test());
            }
            let mut backend = eframe::HeadlessEpiIntegration::new(egui::Context::default());
            let frame = app.frame_counter;
            let sequence = app.input_seq;
            let (output, unused) = backend.update_logic_only(&mut app.app, close_input());
            assert_eq!(backend.should_close(), !tray);
            assert_eq!(app.frame_counter, frame);
            assert_eq!(app.input_seq, sequence);
            assert_eq!(app.window_visible, !tray);
            assert!(unused.events.is_empty());
            let cancelled = output
                .viewport_commands
                .get(&egui::ViewportId::ROOT)
                .is_some_and(|commands| commands.contains(&egui::ViewportCommand::CancelClose));
            assert_eq!(cancelled, tray);
        }
    }

    #[test]
    fn root_lifecycle_actual_app_save_ack_precedes_resumed_backend_close() {
        for result in [Ok(()), Err("fake save failure".to_owned())] {
            let mut app = crate::app::setup_app_for_test();
            app.settings.minimize_to_tray_on_close = false;
            let complete = app.sent_composition_save_for_lifecycle_test();
            let mut backend = eframe::HeadlessEpiIntegration::new(egui::Context::default());
            let (output, _) = backend.update_logic_only(&mut app.app, close_input());
            assert!(!backend.should_close());
            assert!(
                output.viewport_commands[&egui::ViewportId::ROOT]
                    .contains(&egui::ViewportCommand::CancelClose)
            );
            complete(result);
            let (output, _) = backend.update_logic_only(&mut app.app, Default::default());
            assert!(
                !backend.should_close(),
                "Close command is the next delivery, not an incoming request"
            );
            assert!(!app.composition_editor_writes_pending());
            assert_eq!(
                output.viewport_commands[&egui::ViewportId::ROOT]
                    .iter()
                    .filter(|command| matches!(command, egui::ViewportCommand::Close))
                    .count(),
                1
            );
            let (output, _) = backend.update_logic_only(&mut app.app, close_input());
            assert!(backend.should_close());
            assert!(
                !output
                    .viewport_commands
                    .get(&egui::ViewportId::ROOT)
                    .is_some_and(|commands| commands.contains(&egui::ViewportCommand::CancelClose))
            );
        }
    }

    #[test]
    fn root_lifecycle_actual_app_modal_rejection_consumes_only_one_delivery() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.minimize_to_tray_on_close = false;
        let folder = app.tmp.path().join("book");
        app.activate_sidecar_restore_modal_for_test(folder);
        let mut backend = eframe::HeadlessEpiIntegration::new(egui::Context::default());
        let (output, _) = backend.update_logic_only(&mut app.app, close_input());
        assert!(!backend.should_close());
        assert!(
            output.viewport_commands[&egui::ViewportId::ROOT]
                .contains(&egui::ViewportCommand::CancelClose)
        );
        let (output, _) = backend.update_logic_only(&mut app.app, Default::default());
        assert!(!backend.should_close());
        assert!(
            !output
                .viewport_commands
                .values()
                .flatten()
                .any(|command| matches!(command, egui::ViewportCommand::Close))
        );
        let (_output, _) = backend.update_logic_only(&mut app.app, close_input());
        assert!(
            backend.should_close(),
            "a new request is accepted after the discarded owner terminates"
        );
    }
}
