//! User metadata writes share the book-append admission boundary, regardless of
//! whether they came from the grid, native video, egui video, or music UI.
use super::App;

impl App {
    // These are borrowed write entrances, not additional pending owners. Call
    // after resolving a valid target, before changing the DB or optimistic UI.
    // Read-only lookup and automatic marker-thumbnail maintenance use the
    // existing stores directly; they are not later user mutations.
    pub(crate) fn user_folder_thumb_pin_db(
        &mut self,
    ) -> Option<&crate::folder_thumb_pins::FolderThumbPinDb> {
        self.folder_thumb_pin_db.as_ref()?;
        if !self.admit_book_append_followup_mutation() {
            return None;
        }
        self.folder_thumb_pin_db.as_deref()
    }

    pub(crate) fn user_video_bookmark_db(
        &mut self,
    ) -> Option<&mut crate::video_bookmarks::VideoBookmarkDb> {
        self.video_bookmark_db.as_ref()?;
        if !self.admit_book_append_followup_mutation() {
            return None;
        }
        self.video_bookmark_db.as_mut()
    }

    pub(crate) fn user_video_pin_db(&mut self) -> Option<&crate::video_pins::VideoPinDb> {
        self.video_pin_db.as_ref()?;
        if !self.admit_book_append_followup_mutation() {
            return None;
        }
        self.video_pin_db.as_ref()
    }
}
