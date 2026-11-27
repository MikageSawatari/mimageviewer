//! End foreground page drafts while their outgoing item/index space still exists (§1.920).
use super::*;

impl App {
    pub(crate) fn end_page_edit_tools_for_viewer_item_change(&mut self, reason: &'static str) {
        if !self.page_edit_tools_owned_by_projected_context() {
            return;
        }
        let was_editing = self.is_overlay_edit_mode_active();
        // Each exit saves against the still-current page, then consumes its spread pivot.
        // Inactive exits must not touch the active tool's post-filter/preview state.
        if self.conceal_mode {
            self.reset_conceal_mode();
        }
        if self.erase_mode {
            self.reset_erase_mode();
        }
        if self.text_mode {
            self.reset_text_mode();
        }
        if self.local_adjust_mode {
            self.cancel_original_material_entry_intents(None);
            self.cache_current_edit_preview_if_ready();
            self.local_adjust_mode = false;
            self.fold_local_adjust_edit_state();
            self.local_adjust_repair_point_pick_active = None;
            self.local_adjust_add_layer_dialog_open = false;
            self.local_adjust_change_mask_dialog_open = false;
            self.local_adjust_effect_picker_dialog_open = false;
        }
        if self.export_crop_mode {
            self.reset_export_crop_mode();
        }
        if self.sns_split.is_some() {
            self.reset_sns_split_mode();
        }
        // Local layers normally restore on the next frame; the old index space ends now.
        self.reconcile_page_edit_spread_pivots();
        if was_editing {
            crate::logger::log(format!("page_edit: ended outgoing tools reason={reason}"));
        }
    }
}

#[cfg(test)]
mod tests;
