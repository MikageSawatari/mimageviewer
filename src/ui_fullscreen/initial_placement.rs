//! One-shot page-entry positioning. Geometry is resolved by the actual paint path.
use super::*;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FsInitialPlacementRequest {
    pub(crate) items_generation: u64,
    pub(crate) anchor_idx: usize,
    pub(crate) base_zoom: f32,
    pub(crate) base_pan: egui::Vec2,
}

/// `unit` is the union of the canonical, trimmed paint rects at pan zero.
fn reading_start_pan(
    unit: egui::Rect,
    viewport: egui::Rect,
    direction: ReadingDirection,
    base_pan: egui::Vec2,
) -> egui::Vec2 {
    let mut pan = base_pan;
    if unit.height() > viewport.height() {
        pan.y = viewport.top() - unit.top();
    }
    if unit.width() > viewport.width() {
        pan.x = match direction {
            ReadingDirection::Ltr => viewport.left() - unit.left(),
            ReadingDirection::Rtl => viewport.right() - unit.right(),
        };
    }
    pan
}

impl App {
    pub(crate) fn clear_fs_initial_placement_request(&mut self) {
        self.fs_initial_placement_request = None;
    }

    pub(crate) fn request_fs_initial_placement(&mut self, anchor_idx: usize) {
        self.fs_initial_placement_request =
            (self.settings.fullscreen_page_start_position.normalized()
                == crate::settings::FullscreenPageStartPosition::ReadingStart)
                .then_some(FsInitialPlacementRequest {
                    items_generation: self.items_generation,
                    anchor_idx,
                    base_zoom: self.fs_zoom,
                    base_pan: self.fs_pan,
                });
    }

    /// Run after this context's input, including on native media's early return.
    pub(super) fn retire_fs_initial_placement_request(&mut self) {
        if self.fs_initial_placement_request.is_some()
            && (self.settings.fullscreen_page_start_position.normalized()
                != crate::settings::FullscreenPageStartPosition::ReadingStart
                || self.fs_zoom_owner() != FsZoomOwner::Image
                || !self.reading_flow.is_paged()
                || !matches!(self.compare_view_mode, crate::app::CompareViewMode::Off)
                || self.is_overlay_edit_mode_active()
                || self.view_trim_mode
                || !self
                    .fullscreen_idx
                    .and_then(|idx| self.items.get(idx))
                    .is_some_and(GridItem::has_page_data))
        {
            self.clear_fs_initial_placement_request();
        }
    }

    /// The sole consumer. An unavailable selected-material geometry or holdover
    /// retains the request; changed identity or transform retires it.
    pub(super) fn resolve_fs_initial_placement(
        &mut self,
        anchor_idx: usize,
        selected_current: bool,
        viewport: egui::Rect,
        zero_pan_unit: Option<egui::Rect>,
    ) -> bool {
        let Some(request) = self.fs_initial_placement_request else {
            return false;
        };
        if request.items_generation != self.items_generation
            || request.anchor_idx != anchor_idx
            || self.fs_zoom != request.base_zoom
            || self.fs_pan != request.base_pan
        {
            self.clear_fs_initial_placement_request();
            return false;
        }
        if !selected_current {
            return false;
        }
        let Some(unit) = zero_pan_unit else {
            return false;
        };
        self.fs_pan = reading_start_pan(
            unit,
            viewport,
            self.spread_mode
                .reading_direction()
                .unwrap_or(self.reading_direction),
            request.base_pan,
        );
        self.clear_fs_initial_placement_request();
        true
    }
}
