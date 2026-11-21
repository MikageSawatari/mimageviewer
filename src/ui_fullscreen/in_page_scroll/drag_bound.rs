//! Canvas-only drag boundary; routing and geometry arrive in the next stage.
use super::*;

#[allow(dead_code)] // Stage two connects the production drag branch.
pub(super) fn bounded_drag_pan(layout: &FullscreenPageLayout, cur: Vec2, proposed: Vec2) -> Vec2 {
    let _ = (layout, cur);
    proposed
}

impl App {
    #[allow(dead_code)] // Stage two connects the production drag branch.
    pub(crate) fn fs_bounded_drag_pan_surface(&self, ctx: &egui::Context) -> bool {
        self.fs_in_page_scroll_surface()
            && !(self.adjustment_mode.is_open()
                && self.settings.fullscreen_left_panel_tab
                    != crate::settings::FullscreenLeftPanelTab::Bookmarks)
            // A stale receipt must wait for a matching paint, not fall through
            // to the unrestricted writer because an old page was tilted.
            && (!self.fs_scroll_layout_matches(ctx)
                || self
                    .fullscreen_page_layout
                    .displayed_pages()
                    .all(|page| page.transform.free_rotation_rad.abs() <= TRANSFORM_EPSILON))
    }

    #[allow(dead_code)] // Stage two connects the production drag branch.
    pub(crate) fn apply_fs_bounded_drag_pan(&mut self, ctx: &egui::Context, proposed: Vec2) {
        self.clear_fs_initial_placement_request();
        if !self.fs_scroll_layout_matches(ctx) {
            return;
        }
        let pan = bounded_drag_pan(&self.fullscreen_page_layout, self.fs_pan, proposed);
        if pan != self.fs_pan {
            self.fs_zoom = self.fs_page_scroll_drawn_zoom();
            self.fs_pan = pan;
        }
    }
}
