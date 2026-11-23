//! Request-owned creation values; existing runtimes remain the sole placement store.
use super::detached_window_manager::DetachedWindowPurpose;
use super::*;

#[derive(Clone, Copy, Debug)]
pub(crate) struct DetachedWindowSeed {
    pub(crate) purpose: DetachedWindowPurpose,
    pub(crate) placement: Option<crate::detached_window_geometry::PhysicalWindowPlacement>,
}

impl DetachedWindowSeed {
    pub(crate) fn normal(
        placement: Option<crate::detached_window_geometry::PhysicalWindowPlacement>,
    ) -> Self {
        Self {
            purpose: DetachedWindowPurpose::Normal,
            placement,
        }
    }

    pub(crate) fn for_adoption(adoption: ViewerOpenAdoption) -> Self {
        match adoption.request().map(|request| request.intent) {
            Some(ViewerOpenIntent::BrowserOverlay { placement }) => Self {
                purpose: DetachedWindowPurpose::BrowserView,
                placement: Some(placement),
            },
            _ => Self::normal(None),
        }
    }
}

impl App {
    pub(super) fn install_detached_window_seed(
        &mut self,
        id: u64,
        linked: bool,
        seed: DetachedWindowSeed,
    ) {
        self.detached_window_manager
            .create_with_purpose(id, linked, seed.purpose);
        if let Some(placement) = seed.placement {
            let placement = if Self::detached_window_runtime_placement_is_usable(placement) {
                placement
            } else {
                self.detached_viewer_window_placement()
            };
            self.set_detached_window_runtime_placement(id, placement, "request_creation_seed");
        }
    }

    pub(super) fn browser_linked_viewer_session(&self) -> bool {
        self.viewer_session_is_detached()
            && self
                .detached_viewer_window_id()
                .and_then(|id| self.detached_window_manager.runtime(id))
                .is_some_and(|runtime| {
                    runtime.linked && runtime.purpose() == DetachedWindowPurpose::BrowserView
                })
    }

    pub(super) fn browser_overlay_adoption(adoption: ViewerOpenAdoption) -> bool {
        adoption
            .request()
            .is_some_and(|request| request.intent.is_browser_overlay())
    }

    /// Finish the old reader without inspecting an incoming list index. DirectMedia callers
    /// retain the prepared target path/request until this handoff completes, then install it.
    pub(super) fn prepare_browser_overlay_open(&mut self, ctx: &egui::Context) -> bool {
        self.restore_for_browser_overlay(ctx);
        if !self.settings.detached_viewer_open_images_in_window {
            let linked = self
                .detached_window_manager
                .ids()
                .into_iter()
                .filter(|id| {
                    self.detached_window_manager
                        .runtime(*id)
                        .is_some_and(|runtime| runtime.linked)
                })
                .collect::<Vec<_>>();
            for id in linked {
                let Some(owner) = self.capture_detached_close_owner(
                    Self::detached_session_lease(id),
                    ViewerCloseCause::BrowserReplacement,
                ) else {
                    continue;
                };
                if self.detached_viewer_window_id() == Some(id) {
                    self.begin_detached_close_owner(owner, "browser_replacement");
                    if self.close_fullscreen_to_completion(ctx).is_err() {
                        return false;
                    }
                    self.finish_detached_close_owner(owner, "browser_replacement");
                } else {
                    self.close_detached_image_windows_by_ids_with_cause(
                        ctx,
                        &[id],
                        "browser_replacement",
                        None,
                        ViewerCloseCause::BrowserReplacement,
                    );
                }
            }
        }
        // Browser targets are still images. Use the existing independent/media
        // handoff before their reader is built, without looking up an incoming idx.
        if self.active_detached_context_exists()
            && !self.park_and_close_current_active_detached_viewer(ctx)
        {
            return false;
        }
        if !self.viewer_session_is_detached()
            && (self.fullscreen_idx.is_some() || self.direct_media_reader_active())
            && self.close_fullscreen_to_completion(ctx).is_err()
        {
            return false;
        }
        // Browser requests always create a decorated window, never inherit F11.
        self.clear_detached_viewer_borderless_fullscreen_state("browser_creation");
        true
    }

    pub(super) fn foreground_browser_open_refusal(
        &mut self,
        intent: ViewerOpenIntent,
        ctx: &egui::Context,
    ) {
        if intent.is_browser_overlay() {
            if let (Some(hwnd), Some(slot)) = (self.main_hwnd, self.placement_slot.as_ref()) {
                let state = crate::window_activation::activate_main_window(
                    hwnd,
                    slot,
                    ctx,
                    "browser open refusal",
                );
                if state == crate::window_activation::ActivationState::TrayHidden {
                    self.sync_after_restore(ctx);
                }
            }
            // The main UI command belongs to this same refusal, including headless tests.
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        }
    }

    pub(super) fn restore_for_browser_overlay(&mut self, ctx: &egui::Context) {
        use crate::window_activation::MainRestorePolicy;
        for id in self.detached_window_manager.ids() {
            self.detached_window_manager
                .relinquish_pending_show_focus(id);
        }
        if let (Some(hwnd), Some(slot)) = (self.main_hwnd, self.placement_slot.as_ref()) {
            crate::window_activation::restore_main_window_with_policy(
                hwnd,
                slot,
                ctx,
                "browser overlay",
                MainRestorePolicy::PreserveForeground,
            );
        }
        self.sync_after_restore_with_policy(ctx, MainRestorePolicy::PreserveForeground);
    }
}
