//! Admission to App-wide page edit tools belongs to the foreground viewer.

use super::App;

pub(crate) const PAGE_EDIT_NAVIGATION_PENDING_NOTICE: &str = "移動が終わってから編集してください";

impl App {
    /// Accepted navigation must land before a draft can acquire the displayed page.
    /// These owners are projected with the viewer; passive permission reads and
    /// ordinary image preparation do not constitute a navigation request.
    pub(crate) fn page_edit_navigation_pending(&self) -> bool {
        self.fs_nav_is_locked()
            || self.folder_nav_pending.is_some()
            || self.folder_pane_open_pending.is_some()
            || self.top_level_grid_view.collection_navigation_pending()
            || self.main_list_folder_navigation_adoption_pending()
            || self.fs_nav_after_pdf_enumerate.is_some()
            || self.fs_nav_deferred_reopen_wait_active()
            || self.book_composition_permission.open_pending()
            || self.page_turn_barrier_active()
    }

    /// Recheck at both the initial request and its asynchronous completion.
    pub(crate) fn page_edit_mode_navigation_entry_allowed(&mut self) -> bool {
        if !self.page_edit_tools_owned_by_projected_context() {
            return false;
        }
        if self.page_edit_navigation_pending() {
            self.show_feedback_toast(PAGE_EDIT_NAVIGATION_PENDING_NOTICE.to_owned());
            return false;
        }
        true
    }
}

#[cfg(test)]
#[path = "page_edit_admission_tests.rs"]
mod tests;
