//! The local editor's spread pivot travels with its viewer context and keeps
//! content identity when an adopted list changes slot numbers.
use super::*;

pub(crate) struct LocalPageEditSpreadPivot {
    saved_mode: crate::settings::SpreadMode,
    context: ViewerContextId,
    keys: [crate::snapshot::SnapshotKey; 3],
    occurrences: [usize; 3],
    positions: [Option<usize>; 3],
    generation: u64,
}

impl LocalPageEditSpreadPivot {
    pub(super) fn pair(&self) -> Option<(usize, usize)> {
        Some((self.positions[0]?, self.positions[1]?))
    }
    pub(super) fn anchor(&self) -> Option<usize> {
        self.positions[2]
    }
}

impl App {
    pub(crate) fn capture_local_page_edit_pivot(
        &self,
        pivot: PageEditSpreadPivot,
    ) -> Option<LocalPageEditSpreadPivot> {
        let slots = [pivot.pair.0, pivot.pair.1, pivot.navigation_anchor_idx];
        let key_at = |idx| {
            self.items
                .get(idx)
                .and_then(crate::snapshot::snapshot_key_from_grid_item)
        };
        let keys = [key_at(slots[0])?, key_at(slots[1])?, key_at(slots[2])?];
        let occurrences = std::array::from_fn(|n| {
            (0..slots[n])
                .filter(|i| key_at(*i).as_ref() == Some(&keys[n]))
                .count()
        });
        Some(LocalPageEditSpreadPivot {
            saved_mode: pivot.saved_mode,
            context: self.projected_viewer_context_id(),
            keys,
            occurrences,
            positions: slots.map(Some),
            generation: self.items_generation,
        })
    }

    pub(crate) fn reproject_local_page_edit_pivot(&mut self) {
        let Some(pivot) = self.local_adjust_spread_ctx.as_mut() else {
            return;
        };
        if pivot.generation == self.items_generation {
            return;
        }
        pivot.positions = [None; 3];
        let mut occurrences = [0usize; 3];
        for (idx, item) in self.items.iter().enumerate() {
            let Some(key) = crate::snapshot::snapshot_key_from_grid_item(item) else {
                continue;
            };
            for n in 0..3 {
                if key == pivot.keys[n] {
                    if occurrences[n] == pivot.occurrences[n] {
                        pivot.positions[n] = Some(idx);
                    }
                    occurrences[n] += 1;
                }
            }
        }
        pivot.generation = self.items_generation;
    }

    fn leave_local_page_edit_single_view(&mut self, pivot: LocalPageEditSpreadPivot) {
        assert_eq!(pivot.context, self.projected_viewer_context_id());
        self.spread_mode = pivot.saved_mode;
        if let Some(anchor) = pivot.anchor() {
            self.cancel_superseded_fs_navigation_display_target(anchor);
            self.fullscreen_idx = Some(anchor);
            #[cfg(windows)]
            self.publish_effetune_auto_fullscreen();
        }
        self.fs_zoom = 1.0;
        self.fs_pan = egui::Vec2::ZERO;
    }

    pub(super) fn reconcile_local_page_edit_spread_pivot(&mut self) {
        if !self.local_adjust_mode {
            self.reproject_local_page_edit_pivot();
            if let Some(pivot) = self.local_adjust_spread_ctx.take() {
                self.leave_local_page_edit_single_view(pivot);
            }
        }
    }
}
