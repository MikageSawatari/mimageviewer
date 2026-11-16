//! Depth dialog placement and independent-owner admission, derived from existing owners.
use super::{App, ViewerContextId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DepthDialogSurface {
    Main,
    Viewer(ViewerContextId),
}

impl App {
    /// Main's existing admission checks own its image lifetime. An independent
    /// viewer may resume an entry only while it still owns the active live pass.
    pub(crate) fn viewer_context_live(&self, context: ViewerContextId) -> bool {
        if context == self.viewer_context_main() {
            return true;
        }
        #[cfg(windows)]
        {
            self.active_viewer_context_id() == Some(context)
                && self.detached_active_window_alive_wanted()
        }
        #[cfg(not(windows))]
        false
    }

    pub(crate) fn depth_dialog_owner_surface(
        &self,
        owner: Option<ViewerContextId>,
    ) -> DepthDialogSurface {
        match owner {
            Some(context)
                if context != self.viewer_context_main() && self.viewer_context_live(context) =>
            {
                DepthDialogSurface::Viewer(context)
            }
            _ => DepthDialogSurface::Main,
        }
    }

    fn depth_dialog_in_mounted_viewer(&self, owner: Option<ViewerContextId>) -> bool {
        match self.depth_dialog_owner_surface(owner) {
            DepthDialogSurface::Main => {
                self.edit_request_owner_context() == self.viewer_context_main()
            }
            DepthDialogSurface::Viewer(context) => self.edit_request_owner_context() == context,
        }
    }

    pub(crate) fn depth_export_dialog_in_mounted_viewer(&self) -> bool {
        self.depth_dialog_in_mounted_viewer(self.depth_export_owner_context())
    }

    pub(crate) fn depth_addon_dialog_in_mounted_viewer(&self) -> bool {
        self.depth_dialog_in_mounted_viewer(self.depth_setup.dialog_owner_context())
    }

    pub(crate) fn depth_export_dialog_on_main(&self) -> bool {
        self.depth_dialog_owner_surface(self.depth_export_owner_context())
            == DepthDialogSurface::Main
    }

    pub(crate) fn depth_addon_dialog_on_main(&self) -> bool {
        self.depth_dialog_owner_surface(self.depth_setup.dialog_owner_context())
            == DepthDialogSurface::Main
    }

    pub(crate) fn depth_independent_addon_modal_owner(&self) -> Option<ViewerContextId> {
        self.depth_setup
            .entry_modal_owner_context()
            .filter(|context| *context != self.viewer_context_main())
    }

    pub(crate) fn depth_independent_modal_owned_by(&self, context: ViewerContextId) -> bool {
        context != self.viewer_context_main()
            && (self.depth_export_owner_context() == Some(context)
                || self.depth_independent_addon_modal_owner() == Some(context))
    }

    /// Resolve the ordinary accepted transition at modal admission, before an
    /// independent viewer can acquire a modal that would block its own reopen.
    pub(crate) fn depth_modal_entry_waiting_for_transition(&self) -> bool {
        #[cfg(windows)]
        {
            self.edit_request_owner_context() != self.viewer_context_main()
                && self.active_detached_transition_outstanding()
        }
        #[cfg(not(windows))]
        false
    }

    pub(crate) fn depth_panel_rect_id(&self) -> egui::Id {
        egui::Id::unique(("depth_panel_rect", self.edit_request_owner_context()))
    }
}
