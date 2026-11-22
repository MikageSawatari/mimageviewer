//! Terminal decisions are values captured before teardown can remove a runtime.
use super::detached_window_manager::DetachedWindowPurpose;
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DetachedCloseFocus {
    Existing,
    Skip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DetachedClosePolicy {
    pub(crate) save_placement: bool,
    pub(crate) main_focus: DetachedCloseFocus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DetachedCloseOwner {
    pub(crate) lease: DetachedSessionLease,
    pub(crate) purpose: DetachedWindowPurpose,
    pub(crate) cause: ViewerCloseCause,
}

impl DetachedCloseOwner {
    pub(crate) fn policy(self) -> DetachedClosePolicy {
        DetachedClosePolicy {
            save_placement: self.purpose == DetachedWindowPurpose::Normal,
            // Existing means permission for the existing caller's focus path,
            // not a new focus request during Remote, settings changes or exit.
            main_focus: if self.cause == ViewerCloseCause::BrowserReplacement
                || (self.purpose == DetachedWindowPurpose::BrowserView
                    && matches!(
                        self.cause,
                        ViewerCloseCause::User | ViewerCloseCause::DetachedToggle
                    )) {
                DetachedCloseFocus::Skip
            } else {
                DetachedCloseFocus::Existing
            },
        }
    }
}

impl App {
    pub(crate) fn capture_detached_close_owner(
        &self,
        lease: DetachedSessionLease,
        cause: ViewerCloseCause,
    ) -> Option<DetachedCloseOwner> {
        let runtime = self.detached_window_manager.runtime(lease.window_id)?;
        Some(DetachedCloseOwner {
            lease,
            purpose: runtime.purpose(),
            cause,
        })
    }

    pub(super) fn projected_detached_close_owner(
        &self,
        cause: ViewerCloseCause,
    ) -> Option<DetachedCloseOwner> {
        self.detached_viewer_window_id().and_then(|id| {
            self.capture_detached_close_owner(Self::detached_session_lease(id), cause)
        })
    }

    pub(crate) fn focus_main_after_detached_close_if_idle(
        &self,
        ctx: &egui::Context,
        owner: DetachedCloseOwner,
        reason: &'static str,
    ) {
        if owner.policy().main_focus == DetachedCloseFocus::Existing {
            self.focus_main_after_detached_window_close_if_idle(ctx, reason);
        }
    }

    pub(super) fn begin_detached_close_owner(
        &mut self,
        owner: DetachedCloseOwner,
        reason: &'static str,
    ) {
        if self.active_detached_window_id() == Some(owner.lease.window_id) {
            self.begin_active_detached_session_close(reason);
        } else if self.detached_window_state(owner.lease.window_id).is_some() {
            self.transition_detached_window_state(
                owner.lease.window_id,
                DetachedWindowState::Closing,
                reason,
            );
        }
    }

    pub(super) fn finish_detached_close_owner(
        &mut self,
        owner: DetachedCloseOwner,
        reason: &'static str,
    ) {
        if self.active_detached_window_id() == Some(owner.lease.window_id) {
            self.finish_active_detached_session_close_owned(Some(owner), reason);
        } else {
            self.retire_terminal_detached_viewport_identity_owned(owner, reason);
        }
    }

    /// Resolve the window this action would switch. A mounted viewer owns its
    /// own lease; a root action with no reader targets the active at-rest window.
    pub(super) fn detached_toggle_target_lease(&self) -> Option<DetachedSessionLease> {
        let id = self.detached_viewer_window_id().or_else(|| {
            (self.projected_viewer_context_id() == self.viewer_context_main()
                && self.fullscreen_idx.is_none())
            .then(|| self.active_detached_window_id())
            .flatten()
        })?;
        self.detached_window_manager
            .reusable_session_lease(id)
            .then_some(Self::detached_session_lease(id))
    }

    pub(crate) fn browser_view_blocks_detached_toggle(&self) -> bool {
        self.detached_toggle_target_lease()
            .and_then(|lease| self.detached_window_manager.runtime(lease.window_id))
            .is_some_and(|runtime| runtime.purpose() == DetachedWindowPurpose::BrowserView)
    }

    pub(super) fn browser_view_detached_toggle_warning(&self) -> String {
        let key = self
            .keymap
            .first_chord_label(crate::keymap::KeyAction::ToggleDetachedViewerMode);
        let action = key.as_deref().unwrap_or("別ウィンドウ表示の切り替え");
        format!("ブラウザから開いた窓では {action} は使えません")
    }
}

pub(crate) const BROWSER_VIEW_TOGGLE_DISABLED_REASON: &str =
    "ブラウザから開いた窓では別ウィンドウ表示の切り替えは使えません";
