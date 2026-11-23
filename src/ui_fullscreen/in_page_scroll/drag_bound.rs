//! Canvas-only drag bounds from the owning display unit's painted geometry.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FsBoundedDragPanOutcome {
    OutsideSurface,
    WaitingForLayout,
    Position(Vec2),
}

/// Snap the unit edge's integer residue before truncating toward zero.
pub(super) fn trunc_drag_bound_edge_delta(delta: f32, pixel: f32) -> f32 {
    let pixels = delta / pixel;
    let integer = pixels.round();
    let pixels = if (pixels - integer).abs() <= 1.0e-3 {
        integer
    } else {
        pixels
    };
    pixels.trunc() * pixel
}

/// Never expand a visibility box, even near an integer physical pixel.
pub(super) fn inward_drag_visibility_bounds(bounds: Rect, pixel: f32) -> Rect {
    Rect::from_min_max(
        egui::pos2(
            (bounds.min.x / pixel).ceil() * pixel,
            (bounds.min.y / pixel).ceil() * pixel,
        ),
        egui::pos2(
            (bounds.max.x / pixel).floor() * pixel,
            (bounds.max.y / pixel).floor() * pixel,
        ),
    )
}

pub(super) fn bounded_drag_pan(layout: &FullscreenPageLayout, cur: Vec2, proposed: Vec2) -> Vec2 {
    if !cur.is_finite() || !proposed.is_finite() {
        return cur;
    }
    let Some(pages) = flat_navigator_main_pages(layout) else {
        return cur;
    };
    let Some(first) = pages.first() else {
        return cur;
    };
    let viewport = first.viewport_rect;
    let pixel = physical_pixel_scale(first.pixels_per_point);
    let mut elements = Vec::with_capacity(pages.len() + 1);
    for page in pages {
        let Some((_, drawn_pan)) = normal_zoom_pan(page) else {
            return cur;
        };
        let Some(rect) = page_rect_at_pan_zero(&page) else {
            return cur;
        };
        elements.push(rect.translate(cur));
        if let Some(white) = white_companion_rect(&page) {
            elements.push(white.translate(cur - drawn_pan));
        }
    }
    let unit = elements
        .iter()
        .copied()
        .reduce(Rect::union)
        .expect("nonempty pages");
    let mut candidate = cur;
    for axis in 0..2 {
        if unit.size()[axis] > viewport.size()[axis] {
            let min =
                cur[axis] + trunc_drag_bound_edge_delta(viewport.max[axis] - unit.max[axis], pixel);
            let max =
                cur[axis] + trunc_drag_bound_edge_delta(viewport.min[axis] - unit.min[axis], pixel);
            candidate[axis] = proposed[axis].clamp(min.min(cur[axis]), max.max(cur[axis]));
        } else if unit.min[axis] < viewport.min[axis] {
            // A fitting unit outside the viewport may return only inward (§8.2).
            let max =
                cur[axis] + trunc_drag_bound_edge_delta(viewport.min[axis] - unit.min[axis], pixel);
            candidate[axis] = proposed[axis].clamp(cur[axis], max);
        } else if unit.max[axis] > viewport.max[axis] {
            let min =
                cur[axis] + trunc_drag_bound_edge_delta(viewport.max[axis] - unit.max[axis], pixel);
            candidate[axis] = proposed[axis].clamp(min, cur[axis]);
        }
    }

    let visibility = |rect: Rect| {
        let reference = minimum_pan_overlap(rect, viewport);
        let overlap = viewport.max.min(rect.max) - viewport.min.max(rect.min);
        Vec2::new(overlap.x / reference.x, overlap.y / reference.y)
    };
    let level = elements
        .iter()
        .map(|rect| visibility(*rect).min_elem())
        .fold(f32::NEG_INFINITY, f32::max)
        .min(1.0);
    let movement = Rect::from_two_pos(cur.to_pos2(), candidate.to_pos2());
    let mut nearest = None;
    for rect in elements {
        let mut bounds = pan_translation_bounds_for_overlap(
            rect,
            viewport,
            minimum_pan_overlap(rect, viewport) * level,
        );
        let current_visibility = visibility(rect);
        for axis in 0..2 {
            if current_visibility[axis] >= level {
                // Dividing then multiplying the reference may add one ULP.
                // Preserve the known feasible origin before inward rounding.
                bounds.min[axis] = bounds.min[axis].min(0.0);
                bounds.max[axis] = bounds.max[axis].max(0.0);
            }
        }
        // Round into the visibility box, including boxes not containing cur.
        let bounds = inward_drag_visibility_bounds(bounds, pixel);
        let min = cur + bounds.min.to_vec2();
        let max = cur + bounds.max.to_vec2();
        let allowed = Rect::from_min_max(min.to_pos2(), max.to_pos2()).intersect(movement);
        // A zero-width axis is valid: unrequested axes must remain unchanged.
        if allowed.min.x <= allowed.max.x && allowed.min.y <= allowed.max.y {
            let pan = allowed.clamp(candidate.to_pos2()).to_vec2();
            let distance = (pan - candidate).length_sq();
            if nearest.is_none_or(|(_, best_distance)| distance < best_distance) {
                nearest = Some((pan, distance));
            }
        }
    }
    nearest.map_or(cur, |(pan, _)| pan)
}

impl App {
    pub(crate) fn fs_bounded_drag_pan_surface(&self, layout_matches: bool) -> bool {
        self.fs_in_page_scroll_surface()
            && !(self.adjustment_mode.is_open()
                && self.settings.fullscreen_left_panel_tab
                    != crate::settings::FullscreenLeftPanelTab::Bookmarks)
            // A stale receipt must wait for a matching paint, not fall through
            // to the unrestricted writer because an old page was tilted.
            && (!layout_matches
                || self
                    .fullscreen_page_layout
                    .displayed_pages()
                    .all(|page| !crate::displayed_image_transform::free_rotation_is_applied(page.transform.free_rotation_rad)))
    }

    /// Only a matched position may rebase the owning canvas drag's origin.
    pub(crate) fn apply_fs_bounded_drag_pan(
        &mut self,
        ctx: &egui::Context,
        proposed: Vec2,
    ) -> FsBoundedDragPanOutcome {
        // Check the static domain before building the geometry key.
        if !self.fs_bounded_drag_pan_surface(false) {
            return FsBoundedDragPanOutcome::OutsideSurface;
        }
        let layout_matches = self.fs_scroll_layout_matches(ctx);
        if !self.fs_bounded_drag_pan_surface(layout_matches) {
            return FsBoundedDragPanOutcome::OutsideSurface;
        }
        self.clear_fs_initial_placement_request();
        if !layout_matches {
            return FsBoundedDragPanOutcome::WaitingForLayout;
        }
        let pan = bounded_drag_pan(&self.fullscreen_page_layout, self.fs_pan, proposed);
        if pan != self.fs_pan {
            self.fs_zoom = self.fs_page_scroll_drawn_zoom();
            self.fs_pan = pan;
        }
        FsBoundedDragPanOutcome::Position(pan)
    }
}
