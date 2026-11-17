//! A non-persistent exact origin in the one effective spread topology.
use super::*;
use crate::app::spread_shift_owner::SpreadStep;

impl App {
    pub(crate) fn spread_step_start_idx(&self) -> Option<usize> {
        self.spread_step.map(|step| step.start_idx)
    }

    pub(crate) fn set_spread_step(&mut self, start_idx: usize) {
        if self.spread_step_start_idx() != Some(start_idx) {
            self.similar_panel.reset_info_target();
        }
        self.spread_step = Some(SpreadStep { start_idx });
        self.adjust_spread_target = None;
        self.invalidate_fullscreen_display_unit_owners();
    }

    /// Admission is separate from pairing edits: a composed book is eligible.
    /// The mounted collection session, rather than a window/scope predicate,
    /// distinguishes its root from a book opened from that collection.
    fn admit_spread_step(&mut self) -> bool {
        if self
            .top_level_grid_view
            .collection_session()
            .is_some_and(|session| {
                session.position == crate::app::top_level_grid_view::CollectionGridPosition::Root
            })
        {
            return false;
        }
        use crate::spread_shift::PairingEditAvailability;
        match self.pairing_edit_availability() {
            PairingEditAvailability::Automatic | PairingEditAvailability::Composed => {}
            PairingEditAvailability::Unreadable => {
                self.show_feedback_warning(
                    "本の構成を読み込めないため、1 ページ送りは使えません".into(),
                );
                return false;
            }
            PairingEditAvailability::ExtractedResult => {
                self.show_feedback_warning("抽出結果では 1 ページ送りは使えません".into());
                return false;
            }
        }
        self.extracted_result_surface().is_none()
    }

    /// Resolve without changing the origin. The barrier retains this resolved
    /// command and the exact topology allocation, never a direction to replay.
    fn compute_spread_step(&mut self, fs_idx: usize, dir: i32) -> Option<(usize, usize)> {
        let nav = self.get_nav_indices();
        let units = self.build_spread_display_units_for_nav(&nav);
        let position = self.spread_display_units_cache.unit_position(fs_idx)?;
        let start = units.get(position)?.nav_start;
        let next = start.checked_add_signed(dir.signum() as isize)?;
        let start_idx = *nav.get(next)?;
        let empty = HashMap::new();
        let overrides = self
            .composed_book_layout()
            .map(|layout| &layout.page_overrides)
            .unwrap_or(&empty);
        let units = build_spread_display_units_with_overrides(
            &nav,
            self.spread_mode,
            self.spread_shift_anchor_idx(),
            Some(start_idx),
            self.spread_display_units_cache.token.as_ref()?.page_alone,
            overrides,
            |_, idx| {
                self.spread_display_units_cache
                    .landscape_indices
                    .contains(&idx)
            },
            |idx| is_spread_pairable_item(self.items.get(idx)),
        );
        let target = find_spread_display_unit(&units, start_idx)?.1.anchor_idx();
        Some((target, start_idx))
    }

    pub(crate) fn queue_spread_step_action(
        &mut self,
        fs_idx: usize,
        dir: i32,
        action: &mut FsKeyAction,
    ) {
        if dir == 0 || !action.page_nav.is_none() || action.jump_to.is_some() {
            return;
        }
        if !self.spread_mode.is_spread() || !self.reading_flow.is_paged() {
            action.page_nav = self.spread_page_nav(dir);
            return;
        }
        if !self.admit_spread_step() || self.fs_navigation_sequence_blocks_new_target() {
            return;
        }
        if let Some((target, start)) = self.compute_spread_step(fs_idx, dir) {
            self.set_spread_step(start);
            action.page_nav = FsPageNav::Target(target);
        }
    }

    pub(crate) fn queue_spread_step_action_for_frame(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        dir: i32,
        action: &mut FsKeyAction,
        operation: Option<ViewerOperationId>,
    ) {
        if dir == 0 || !action.page_nav.is_none() || action.jump_to.is_some() {
            return;
        }
        // Leave ordinary modes on precisely the FsPagePrev/FsPageNext path,
        // including its split slice, wrapping and normal barrier admission.
        if !self.spread_mode.is_spread() || !self.reading_flow.is_paged() {
            action.page_nav = self.spread_page_nav(dir);
            return;
        }
        if self.page_turn_barrier_active() {
            if self.admit_spread_step()
                && let Some((target, start)) = self.compute_spread_step(fs_idx, dir)
            {
                self.page_turn_defer_spread_step(ctx, target, operation, start);
            }
            return;
        }
        self.queue_spread_step_action(fs_idx, dir, action);
        if matches!(action.page_nav, FsPageNav::Target(_)) {
            self.page_turn_excluded_navigation_with_operation(ctx, "stage1_spread_step", operation);
        }
    }
}
