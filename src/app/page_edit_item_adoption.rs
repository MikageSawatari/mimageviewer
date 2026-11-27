//! Decide outgoing editor lifetime before an item adoption captures or validates state.
use super::*;

impl App {
    fn local_page_edit_owner_before_adoption(&self) -> Option<PageEditOwner> {
        let owner = match &self.local_page_edit_document {
            LocalPageEditDocument::Selection { owner, .. }
            | LocalPageEditDocument::Editing { owner, .. } => Some(owner),
            LocalPageEditDocument::Idle => self
                .local_adjust_shape_key_edit
                .as_ref()
                .map(|session| &session.owner)
                .or_else(|| {
                    self.local_adjust_shape_drag
                        .as_ref()
                        .map(|session| &session.owner)
                })
                .or_else(|| {
                    self.local_adjust_canvas_drag
                        .as_ref()
                        .map(|session| &session.owner)
                })
                .or_else(|| {
                    self.local_adjust_mask_brush_stroke
                        .as_ref()
                        .map(|session| &session.owner)
                })
                .or_else(|| {
                    self.adjustment_drag_session
                        .as_ref()
                        .map(|session| &session.owner)
                })
                .or_else(|| {
                    self.local_adjust_lut_pending
                        .as_ref()
                        .map(|pending| &pending.owner)
                })
                .or_else(|| {
                    self.local_adjust_segmentation_pending
                        .as_ref()
                        .map(|pending| &pending.owner)
                }),
        };
        if let Some(owner) = owner {
            return self
                .page_edit_owner_is_current(owner)
                .then(|| owner.clone());
        }
        self.fullscreen_idx
            .and_then(|idx| self.capture_page_edit_owner(idx))
    }

    /// Only local layers already have an identity-owned document and spread pivot.
    /// Keep that existing owner through reindexing; the other tools must exit in
    /// the outgoing index space even when their page remains in the new list.
    pub(crate) fn end_page_edit_tools_before_items_adoption(
        &mut self,
        reason: &'static str,
        items: &[GridItem],
    ) {
        let continue_local = self.local_page_edit_survives_items_adoption(items);
        self.end_page_edit_tools_for_item_adoption(reason, continue_local);
    }

    pub(crate) fn local_page_edit_survives_items_adoption<'a>(
        &self,
        items: impl IntoIterator<Item = &'a GridItem>,
    ) -> bool {
        if !self.local_adjust_mode || !self.page_edit_tools_owned_by_projected_context() {
            return false;
        }
        self.local_page_edit_owner_before_adoption()
            .is_some_and(|owner| {
                items.into_iter().any(|item| {
                    crate::edit_source::page_key_for_grid_item(item).as_deref()
                        == Some(owner.target.page_key.as_str())
                })
            })
    }

    pub(crate) fn end_page_edit_tools_before_item_removal(&mut self, removed: &[usize]) {
        let continue_local = self.local_adjust_mode
            && self.page_edit_tools_owned_by_projected_context()
            && self
                .local_page_edit_owner_before_adoption()
                .is_some_and(|owner| {
                    // If the reader is one of the owner's aliases, deleting that exact
                    // occurrence ends its edit even when another alias remains.
                    let reader = self.fullscreen_idx.filter(|idx| owner.slots.contains(idx));
                    reader.map_or_else(
                        || owner.slots.iter().any(|idx| !removed.contains(idx)),
                        |idx| !removed.contains(&idx),
                    )
                });
        self.end_page_edit_tools_for_item_adoption("remove_items_batch", continue_local);
    }
}

#[cfg(test)]
mod tests;
