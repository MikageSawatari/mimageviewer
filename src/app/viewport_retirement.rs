//! Transfer approved child lifetimes at their existing application owner.
use super::App;

/// Authority for an existing egui registration and its root command sink.
/// Capturing never creates a registration; taking this value transfers approval once.
#[cfg(windows)]
#[derive(Debug, Clone)]
pub(super) struct ViewportCommandBinding {
    context: egui::Context,
    registration: egui::ViewportRegistration,
}

#[cfg(windows)]
impl ViewportCommandBinding {
    pub(super) fn capture(context: &egui::Context, id: egui::ViewportId) -> Option<Self> {
        context.viewport_registration(id).map(|registration| Self {
            context: context.clone(),
            registration,
        })
    }
    pub(super) fn viewport_id(&self) -> egui::ViewportId {
        self.registration.viewport_id()
    }
    pub(super) fn transfer_retirement(self) {
        self.context.approve_viewport_retirement(self.registration);
    }
}

impl App {
    #[cfg(windows)]
    pub(crate) fn accept_active_detached_window_close(
        &mut self,
        ctx: &egui::Context,
        window_id: u64,
        reason: &'static str,
    ) -> Option<bool> {
        let is_active = self
            .active_detached_session
            .is_some_and(|session| session.window_id == window_id);
        if !is_active {
            return None;
        }
        if self.sidecar_restore_target_window_id() == Some(window_id) {
            return Some(false);
        }
        let allowed = match self
            .with_window_viewer_context_state(window_id, |app| !app.viewer_mutation_modal_active())
        {
            Ok(allowed) => allowed,
            Err(error) => {
                crate::logger::log(format!(
                    "detached_close_owner_unavailable window_id={window_id} error={error:?}"
                ));
                return Some(false);
            }
        };
        if !allowed {
            return Some(false);
        }
        if self.active_detached_context_is_at_rest() {
            return Some(self.close_current_active_viewer_context(ctx));
        }
        self.begin_active_detached_session_close(reason);
        self.finish_active_detached_session_close(reason);
        if let Err(error) = self.close_fullscreen_to_completion(ctx) {
            crate::logger::log(format!(
                "detached_close_terminal_failed window_id={window_id} error={error:?}"
            ));
        }
        Some(true)
    }

    pub(super) fn accept_viewport_close_request(
        &mut self,
        ctx: &egui::Context,
        viewport_id: egui::ViewportId,
    ) -> eframe::ViewportCloseResponse {
        use eframe::ViewportCloseResponse;
        #[cfg(windows)]
        if let Some(window_id) = self
            .detached_window_manager
            .ids()
            .into_iter()
            .find(|id| Self::detached_image_window_viewport_id(*id) == viewport_id)
        {
            // A real backend request may be the first callback for this existing
            // owner. Capture its existing registration before the owner is removed.
            self.capture_detached_viewport_registration(ctx, window_id);
            return if self.request_detached_image_window_close_by_ids(
                ctx,
                &[window_id],
                "viewport_close_request",
                None,
            ) {
                ViewportCloseResponse::RetirementTransferred
            } else {
                ViewportCloseResponse::Cancel
            };
        }
        // The standalone fullscreen surface has its own existing owner. Other
        // application/tool viewports retain the backend's UI delegation contract.
        #[cfg(windows)]
        if self
            .fullscreen_viewport_binding
            .as_ref()
            .is_some_and(|binding| binding.viewport_id() == viewport_id)
        {
            if self.viewer_mutation_modal_active()
                || self.sidecar_restore_blocks_projected_context()
            {
                return ViewportCloseResponse::Cancel;
            }
            // A Context GC may already have removed the registration. The
            // existing fullscreen owner retains its exact command authority.
            self.transfer_fullscreen_viewport_retirement();
            if let Err(error) = self.close_fullscreen_to_completion(ctx) {
                crate::logger::log(format!("fullscreen_viewport_close_failed error={error:?}"));
            }
            self.fs_viewport_shown = false;
            self.fs_viewport_presentation = None;
            return ViewportCloseResponse::RetirementTransferred;
        }
        ViewportCloseResponse::DelegateToUi
    }
}
