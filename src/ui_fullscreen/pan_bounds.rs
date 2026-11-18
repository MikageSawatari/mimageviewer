//! Pan constraints at the material-selection boundary (no persistent state).
use super::*;

#[cfg(test)]
mod projection_tests;
#[cfg(test)]
mod tests;

pub(super) fn minimum_pan_correction(a: egui::Vec2, b: egui::Vec2) -> egui::Vec2 {
    if b.length_sq() < a.length_sq() { b } else { a }
}

/// Include the painted unit as well as every real-page candidate. This accepts
/// both input clamping and ReadingStart's placement of white companion slots.
pub(super) fn draw_pan_correction(pages: &[DisplayedImageTransform]) -> egui::Vec2 {
    let Some(first) = pages.first() else {
        return egui::Vec2::ZERO;
    };
    let viewport = first.viewport_rect;
    let mut unit = egui::Rect::NOTHING;
    let mut best = None;
    for page in pages {
        let rect = fs_navigator_source_rect_aabb(page, page.uv_rect);
        unit = unit.union(rect);
        if let Some(white) = white_companion_rect(page) {
            unit = unit.union(white);
        }
        let correction = pan_correction_for_minimum_overlap(rect, viewport);
        best = Some(best.map_or(correction, |best| minimum_pan_correction(best, correction)));
    }
    let correction = minimum_pan_correction(
        best.expect("nonempty real pages"),
        pan_correction_for_minimum_overlap(unit, viewport),
    );
    // Layout-origin snap, sampler-origin snap and joint gap alignment can
    // change the re-resolved edge by up to two physical pixels. Accept only
    // that residual; a larger correction still goes all the way to the edge.
    let snap_tolerance = 2.0 * physical_pixel_scale(first.pixels_per_point);
    if correction.abs().max_elem() <= snap_tolerance {
        egui::Vec2::ZERO
    } else {
        correction
    }
}

impl App {
    pub(super) fn fs_draw_pan_eligible(
        &self,
        selected_current: bool,
        original_preview_active: bool,
    ) -> bool {
        selected_current
            && !original_preview_active
            && self
                .fullscreen_idx
                .and_then(|idx| self.items.get(idx))
                .is_some_and(GridItem::has_page_data)
            && self.fs_zoom_owner() == FsZoomOwner::Image
            && self.reading_flow.is_paged()
            && matches!(self.compare_view_mode, crate::app::CompareViewMode::Off)
            && !self.is_overlay_edit_mode_active()
            && !self.view_trim_mode
    }

    /// Publish only after the complete unit's actual materials are selected.
    /// Initial placement has already committed, retaining its origin identity.
    pub(super) fn commit_fs_draw_pan(
        &mut self,
        pan: egui::Vec2,
        initial_origin_automatic: Option<bool>,
    ) {
        if self.fs_pan == pan {
            return;
        }
        let automatic = initial_origin_automatic.unwrap_or_else(|| {
            !self.fs_view_has_moved(self.fs_zoom, self.fs_pan, self.fs_free_rotation)
        });
        self.fs_pan = pan;
        if automatic {
            self.record_fs_automatic_initial_view(pan);
        }
    }
}
