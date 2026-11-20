//! Page-entry positioning and automatic-view identity for canvas clicks.
use super::*;
use crate::gpu_lanczos::FullscreenPageMaterialKind;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FsInitialPlacementKind {
    Setting,
    LandingStart,
    LandingEnd,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FsInitialPlacementRequest {
    pub(crate) items_generation: u64,
    pub(crate) anchor_idx: usize,
    pub(crate) kind: FsInitialPlacementKind,
    /// Immutable placement basis captured at accepted page entry.
    pub(crate) origin_zoom: f32,
    pub(crate) origin_pan: egui::Vec2,
    /// Whether the origin matched its automatic view before this request.
    pub(crate) origin_automatic: bool,
    /// Last automatic placement, used only to detect an intervening writer.
    pub(crate) applied_zoom: f32,
    pub(crate) applied_pan: egui::Vec2,
}

/// Pure automatic placement, retained after the placement request completes.
/// It follows the view transform, not the items namespace or its current index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FsAutomaticInitialView {
    zoom: f32,
    pan: egui::Vec2,
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

/// The last reading corner, preserving the origin on axes that already fit.
fn reading_end_pan(
    unit: egui::Rect,
    viewport: egui::Rect,
    direction: ReadingDirection,
    origin_pan: egui::Vec2,
) -> egui::Vec2 {
    let mut pan = origin_pan;
    if unit.height() > viewport.height() {
        pan.y = viewport.bottom() - unit.bottom();
    }
    if unit.width() > viewport.width() {
        pan.x = match direction {
            ReadingDirection::Ltr => viewport.right() - unit.right(),
            ReadingDirection::Rtl => viewport.left() - unit.left(),
        };
    }
    pan
}

impl FsInitialPlacementKind {
    fn pan(
        self,
        unit: egui::Rect,
        viewport: egui::Rect,
        direction: ReadingDirection,
        origin_pan: egui::Vec2,
    ) -> egui::Vec2 {
        match self {
            Self::Setting | Self::LandingStart => {
                reading_start_pan(unit, viewport, direction, origin_pan)
            }
            Self::LandingEnd => reading_end_pan(unit, viewport, direction, origin_pan),
        }
    }
}

impl App {
    pub(super) fn record_fs_automatic_initial_view(&mut self, pan: egui::Vec2) {
        self.fs_automatic_initial_view = Some(FsAutomaticInitialView {
            zoom: self.fs_zoom,
            pan,
        });
    }

    /// Whether the user has moved the view away from its last automatic placement.
    ///
    /// The 2026-08-15 rule deliberately excluded pan: counting a nudge would
    /// stop click navigation without a marked way back. On 2026-11-15 the
    /// user reversed that decision after §1.745: a double-click cannot both
    /// reset the view and turn pages. The manual now documents double-click
    /// reset as the way back. Automatic reading-start/end placement is not a move.
    pub(super) fn fs_view_has_moved(&self, zoom: f32, pan: egui::Vec2, free_rotation: f32) -> bool {
        zoom_differs_from_fit(zoom)
            || free_rotation.abs() > TRANSFORM_EPSILON
            || (pan.length_sq() > PAN_EPSILON_SQ
                && !self
                    .fs_automatic_initial_view
                    .is_some_and(|initial| initial.zoom == zoom && initial.pan == pan))
    }

    pub(crate) fn clear_fs_initial_placement_request(&mut self) {
        self.fs_initial_placement_request = None;
    }

    pub(crate) fn request_fs_initial_placement(&mut self, anchor_idx: usize) {
        self.request_fs_initial_placement_with_kind(anchor_idx, FsInitialPlacementKind::Setting);
    }

    pub(crate) fn request_fs_initial_placement_with_kind(
        &mut self,
        anchor_idx: usize,
        kind: FsInitialPlacementKind,
    ) {
        self.fs_initial_placement_request = (kind != FsInitialPlacementKind::Setting
            || self.settings.fullscreen_page_start_position.normalized()
                == crate::settings::FullscreenPageStartPosition::ReadingStart)
            .then_some(FsInitialPlacementRequest {
                items_generation: self.items_generation,
                anchor_idx,
                kind,
                origin_zoom: self.fs_zoom,
                origin_pan: self.fs_pan,
                origin_automatic: !self.fs_view_has_moved(
                    self.fs_zoom,
                    self.fs_pan,
                    self.fs_free_rotation,
                ),
                applied_zoom: self.fs_zoom,
                applied_pan: self.fs_pan,
            });
    }

    /// Accepted composition changes use the zoom selected by the existing path.
    pub(crate) fn request_fs_initial_placement_after_composition_change(&mut self) {
        let kind = self
            .fs_initial_placement_request
            .map_or(FsInitialPlacementKind::Setting, |request| request.kind);
        if !self.reading_flow.is_paged()
            || (kind == FsInitialPlacementKind::Setting
                && self.settings.fullscreen_page_start_position.normalized()
                    != crate::settings::FullscreenPageStartPosition::ReadingStart)
        {
            return;
        }
        if let Some(anchor_idx) = self.fullscreen_idx {
            self.fs_pan = egui::Vec2::ZERO;
            self.fs_automatic_initial_view = None;
            self.request_fs_initial_placement_with_kind(anchor_idx, kind);
        }
    }

    /// Run after this context's input, including on native media's early return.
    pub(super) fn retire_fs_initial_placement_request(&mut self) {
        if self.fs_initial_placement_request.is_some_and(|request| {
            (request.kind == FsInitialPlacementKind::Setting
                && self.settings.fullscreen_page_start_position.normalized()
                    != crate::settings::FullscreenPageStartPosition::ReadingStart)
                || self.fs_zoom_owner() != FsZoomOwner::Image
                || !self.reading_flow.is_paged()
                || !matches!(self.compare_view_mode, crate::app::CompareViewMode::Off)
                || self.is_overlay_edit_mode_active()
                || self.view_trim_mode
                || !self
                    .fullscreen_idx
                    .and_then(|idx| self.items.get(idx))
                    .is_some_and(GridItem::has_page_data)
        }) {
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
        Some(
            request.kind.pan(
                unit,
                viewport,
                self.spread_mode
                    .reading_direction()
                    .unwrap_or(self.reading_direction),
                request.origin_pan,
            ),
        )
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
        self.fs_automatic_initial_view = Some(FsAutomaticInitialView {
            zoom: self.fs_zoom,
            // Half navigation retains pan on non-overflowing axes. Preserve
            // automatic origins there, but keep manual components distinct.
            pan: if request.origin_automatic {
                pan
            } else {
                request.kind.pan(
                    zero_pan_unit.expect("validated placement material"),
                    viewport,
                    self.spread_mode
                        .reading_direction()
                        .unwrap_or(self.reading_direction),
                    egui::Vec2::ZERO,
                )
            },
        });
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
