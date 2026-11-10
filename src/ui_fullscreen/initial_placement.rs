//! Page-entry positioning: provisional paint aligns until the first complete unit.
use super::*;
use crate::gpu_lanczos::FullscreenPageMaterialKind;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FsInitialPlacementRequest {
    pub(crate) items_generation: u64,
    pub(crate) anchor_idx: usize,
    /// Immutable placement basis captured at accepted page entry.
    pub(crate) origin_zoom: f32,
    pub(crate) origin_pan: egui::Vec2,
    /// Last automatic placement, used only to detect an intervening writer.
    pub(crate) applied_zoom: f32,
    pub(crate) applied_pan: egui::Vec2,
}

/// `unit` is the union of the canonical, trimmed paint rects at pan zero.
fn reading_start_pan(
    unit: egui::Rect,
    viewport: egui::Rect,
    direction: ReadingDirection,
    origin_pan: egui::Vec2,
) -> egui::Vec2 {
    let mut pan = origin_pan;
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
                    origin_zoom: self.fs_zoom,
                    origin_pan: self.fs_pan,
                    applied_zoom: self.fs_zoom,
                    applied_pan: self.fs_pan,
                });
    }

    /// Accepted composition changes use the zoom selected by the existing path.
    pub(crate) fn request_fs_initial_placement_after_composition_change(&mut self) {
        if !self.reading_flow.is_paged()
            || self.settings.fullscreen_page_start_position.normalized()
                != crate::settings::FullscreenPageStartPosition::ReadingStart
        {
            return;
        }
        if let Some(anchor_idx) = self.fullscreen_idx {
            self.fs_pan = egui::Vec2::ZERO;
            self.request_fs_initial_placement(anchor_idx);
        }
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

    /// Propose without publishing a transform from a material that preparation
    /// may still reject. Every fallback starts from the unchanged request.
    pub(super) fn fs_initial_placement_pan(
        &mut self,
        anchor_idx: usize,
        selected_current: bool,
        viewport: egui::Rect,
        zero_pan_unit: Option<egui::Rect>,
    ) -> Option<egui::Vec2> {
        let Some(request) = self.fs_initial_placement_request else {
            return None;
        };
        if request.items_generation != self.items_generation
            || request.anchor_idx != anchor_idx
            || self.fs_zoom != request.applied_zoom
            || self.fs_pan != request.applied_pan
        {
            self.clear_fs_initial_placement_request();
            return None;
        }
        if !selected_current {
            return None;
        }
        let Some(unit) = zero_pan_unit else {
            return None;
        };
        Some(reading_start_pan(
            unit,
            viewport,
            self.spread_mode
                .reading_direction()
                .unwrap_or(self.reading_direction),
            request.origin_pan,
        ))
    }

    /// The sole consumer, after preparation selected the actual paint material.
    pub(super) fn resolve_fs_initial_placement(
        &mut self,
        anchor_idx: usize,
        selected_current: bool,
        viewport: egui::Rect,
        zero_pan_unit: Option<egui::Rect>,
        material_kind: FullscreenPageMaterialKind,
    ) -> bool {
        let Some(pan) =
            self.fs_initial_placement_pan(anchor_idx, selected_current, viewport, zero_pan_unit)
        else {
            return false;
        };
        let request = self
            .fs_initial_placement_request
            .expect("validated placement request");
        self.fs_pan = pan;
        self.fs_initial_placement_request = match material_kind {
            FullscreenPageMaterialKind::Complete => None,
            FullscreenPageMaterialKind::Provisional => Some(FsInitialPlacementRequest {
                applied_zoom: self.fs_zoom,
                applied_pan: pan,
                ..request
            }),
        };
        true
    }
}
