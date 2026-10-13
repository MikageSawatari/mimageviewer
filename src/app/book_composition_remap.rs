//! Deletion prepares the immutable topology from original rows, then adopts it
//! in the same context and generation as the grid's dense item-removal result.
use super::{App, book_composition_runtime::ViewerBookComposition};
use crate::book_composition::{
    AcceptedCompositionProjection, CompositionProjectionOutcome, composition_page_from_identity,
};
use std::sync::Arc;

impl App {
    pub(crate) fn prepare_book_composition_item_removal(
        &self,
        sorted_asc: &[usize],
    ) -> Result<Option<Arc<AcceptedCompositionProjection>>, String> {
        let ViewerBookComposition::Accepted {
            layout: CompositionProjectionOutcome::Composed(projection),
            ..
        } = &self.book_composition
        else {
            return Ok(None);
        };
        if sorted_asc.is_empty() {
            return Ok(None);
        }
        if sorted_asc.windows(2).any(|pair| pair[0] >= pair[1])
            || sorted_asc
                .last()
                .is_some_and(|index| *index >= self.items.len())
        {
            return Err("invalid composition removal indices".into());
        }
        let mut removed_pages = Vec::with_capacity(sorted_asc.len());
        for index in sorted_asc {
            if let Some(identity) = crate::book_resume_db::PageIdentity::for_item(
                &self.items[*index],
                Some(&projection.scope.logical_root),
            ) {
                let page = composition_page_from_identity(&projection.scope, &identity)
                    .ok_or("removed page does not belong to the accepted composition")?;
                removed_pages.push(page);
            }
        }
        let mut removed_cursor = 0;
        let mut next = 0;
        let old_to_new = (0..self.items.len())
            .map(|old| {
                if sorted_asc.get(removed_cursor) == Some(&old) {
                    removed_cursor += 1;
                    None
                } else {
                    let new = next;
                    next += 1;
                    Some(new)
                }
            })
            .collect::<Vec<_>>();
        projection
            .remap_removed_items(&old_to_new, &removed_pages)
            .map(|projection| Some(Arc::new(projection)))
            .map_err(|error| error.to_string())
    }

    /// Called immediately after the grid's generation bump. No persistence is
    /// admitted: the saved record still describes absent/hidden source identities.
    pub(crate) fn adopt_book_composition_item_removal(
        &mut self,
        prepared: Option<Arc<AcceptedCompositionProjection>>,
    ) {
        let Some(projection) = prepared else {
            return;
        };
        let ViewerBookComposition::Accepted {
            scope,
            token,
            layout,
            generation,
            ..
        } = &mut self.book_composition
        else {
            return;
        };
        if scope != &projection.scope || token != &projection.token {
            return;
        }
        *layout = CompositionProjectionOutcome::Composed(projection);
        *generation = self.items_generation;
        // Integer slots can repeat after deletion. Drop every fitted/source
        // measurement and permission proof tied to the previous identity binding.
        self.spread_display_units_cache = Default::default();
        self.fs_continuous_fit_reference =
            crate::ui_fullscreen::ContinuousSpecifiedFitReference::Legacy;
        self.retire_book_presentation_permission();
        self.invalidate_fullscreen_display_unit_owners();
    }
}

#[cfg(test)]
mod tests {
    include!("book_composition_remap_tests.rs");
}
