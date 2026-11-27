//! End foreground page drafts while their outgoing item/index space still exists (§1.920).
use super::*;

#[derive(Clone, Copy)]
pub(crate) enum LocalAdjustmentExitPreview {
    CacheCurrent,
    FullscreenCloseOwnsSnapshot,
}

#[derive(Clone, Copy)]
enum PageEditExit {
    ItemChange { continue_local_adjustment: bool },
    Fullscreen(LocalAdjustmentExitPreview),
}

impl App {
    pub(crate) fn end_page_edit_tools_for_viewer_item_change(&mut self, reason: &'static str) {
        self.end_page_edit_tools(
            reason,
            PageEditExit::ItemChange {
                continue_local_adjustment: false,
            },
        );
    }

    pub(super) fn end_page_edit_tools_for_item_adoption(
        &mut self,
        reason: &'static str,
        continue_local_adjustment: bool,
    ) {
        self.end_page_edit_tools(
            reason,
            PageEditExit::ItemChange {
                continue_local_adjustment,
            },
        );
    }

    pub(super) fn end_page_edit_tools_for_fullscreen_exit(
        &mut self,
        preview: LocalAdjustmentExitPreview,
    ) {
        self.end_page_edit_tools("fullscreen_exit", PageEditExit::Fullscreen(preview));
    }

    /// The ordinary panel exit, also shared by item changes and viewer close.
    pub(crate) fn exit_local_adjust_mode(&mut self, preview: LocalAdjustmentExitPreview) {
        self.cancel_mask_detect_for_tool(MaskDetectTool::LocalAdjust);
        self.cancel_original_material_entry_intents(None);
        if matches!(preview, LocalAdjustmentExitPreview::CacheCurrent) {
            self.cache_current_edit_preview_if_ready();
        }
        self.local_adjust_mode = false;
        self.fold_local_adjust_edit_state();
        self.local_adjust_repair_point_pick_active = None;
        self.local_adjust_add_layer_dialog_open = false;
        self.local_adjust_change_mask_dialog_open = false;
        self.local_adjust_effect_picker_dialog_open = false;
    }

    fn end_page_edit_tools(&mut self, reason: &'static str, exit: PageEditExit) {
        if !self.page_edit_tools_owned_by_projected_context() {
            return;
        }
        let was_editing = self.is_overlay_edit_mode_active();
        if matches!(exit, PageEditExit::Fullscreen(_)) {
            // True close keeps these tools' released discard-without-restoration behavior.
            // Its caller already captured the grid landing from the outgoing pivots.
            self.text_spread_ctx = None;
            self.export_crop_spread_ctx = None;
            self.sns_split_spread_ctx = None;
        }
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
        let continue_local_adjustment = matches!(
            exit,
            PageEditExit::ItemChange {
                continue_local_adjustment: true
            }
        );
        // A surviving local document keeps its tool and detection request. Item
        // adoption advances items_generation; mask_detect_request_is_current rejects
        // that old request before adding any result, even if the page survives.
        if self.local_adjust_mode && !continue_local_adjustment {
            let preview = match exit {
                PageEditExit::Fullscreen(preview) => preview,
                PageEditExit::ItemChange { .. } => LocalAdjustmentExitPreview::CacheCurrent,
            };
            self.exit_local_adjust_mode(preview);
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

#[cfg(test)]
std::thread_local! {
    static PREVIEW_SAVE_COUNT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(super) fn record_edit_preview_save_for_test() {
    PREVIEW_SAVE_COUNT.with(|count| {
        if let Some(value) = count.get() {
            count.set(Some(value + 1));
        }
    });
}
