//! Page-local scrolling uses the drawn geometry and the normal pan constraint.
use super::*;
use egui::emath::GuiRounding;
use egui::{Rect, Vec2};

#[cfg(test)]
mod context_scroll_tests;
mod drag_bound;
#[cfg(test)]
mod drag_bound_handler_tests;
#[cfg(test)]
mod drag_bound_input_tests;
#[cfg(test)]
mod drag_bound_tests;
#[cfg(test)]
mod handler_tests;
#[cfg(test)]
mod interaction_tests;
#[cfg(test)]
mod landing_tests;
#[cfg(test)]
mod n_scroll_report_850_tests;
#[cfg(test)]
mod navigation_landing_tests;
#[cfg(test)]
mod overflow_tests;
#[cfg(test)]
mod review_regression_tests;
#[cfg(test)]
mod scroll_then_turn_tests;
#[cfg(test)]
mod scroll_turn_handler_tests;
#[cfg(test)]
mod settings_input_tests;
#[cfg(test)]
mod smooth_render_tests;
#[cfg(test)]
mod smooth_scroll_tests;
#[cfg(test)]
mod wheel_zoom_tests;

/// 描画側の CentralPanel / `Context::content_rect()` と同じ丸めにする。
/// 丸めないと、1/32 point の倍数でない窓で記録と入力の viewport が一致しない (§1.815)。
fn rounded_content_rect_for_viewport(ctx: &egui::Context, viewport: egui::ViewportId) -> Rect {
    ctx.input_for(viewport, |input| input.content_rect())
        .round_ui()
}

/// Remove only the pan used for the actual paint. The App boundary already
/// checked the painted effective zoom; no rectangle is projected to another scale.
fn page_rect_at_pan_zero(page: &DisplayedImageTransform) -> Option<Rect> {
    let (_, base_pan) = normal_zoom_pan(*page)?;
    if !base_pan.is_finite() {
        return None;
    }
    let rect = page
        .paint_quad()
        .positions
        .iter()
        .fold(Rect::NOTHING, |rect, point| {
            rect.union(Rect::from_min_max(*point, *point))
        })
        .translate(-base_pan);
    (rect.is_finite() && rect.is_positive()).then_some(rect)
}

#[derive(Clone, Copy)]
struct AxisRange {
    min: f32,
    max: f32,
}

impl AxisRange {
    /// A manually dragged position can lie beyond either aligned edge. Moving
    /// inward advances by the requested step; moving farther outward is inert.
    fn step(self, current: f32, delta: f32) -> f32 {
        if delta > 0.0 && current < self.max {
            (current + delta).min(self.max)
        } else if delta < 0.0 && current > self.min {
            (current + delta).max(self.min)
        } else {
            current
        }
    }

    fn start(self, forward_sign: f32) -> f32 {
        if forward_sign < 0.0 {
            self.max
        } else {
            self.min
        }
    }
}

struct ScrollGeometry {
    viewport: Rect,
    axes: [Option<AxisRange>; 2],
}

fn scroll_geometry(layout: &FullscreenPageLayout) -> Option<ScrollGeometry> {
    let pages = flat_navigator_main_pages(layout)?;
    let viewport = pages.first()?.viewport_rect;
    let mut unit: Option<Rect> = None;
    for page in pages {
        let rect = page_rect_at_pan_zero(&page)?;
        unit = Some(unit.map_or(rect, |unit| unit.union(rect)));
        if let ResolvedDisplayPlacement::SingletonSpread { side, gap, .. } = page.placement {
            // White is an unrotated painted decoration, with a fixed point gap.
            if side.has_white_companion() {
                let (_, base_pan) = normal_zoom_pan(page)?;
                let paint = page.paint_rect.translate(-base_pan);
                let white = white_companion_rect_for_real(paint, side, gap)?;
                unit = Some(unit.expect("real page was added").union(white));
            }
        }
    }
    let unit = unit?;
    Some(ScrollGeometry {
        viewport,
        axes: [
            (unit.width() > viewport.width()).then_some(AxisRange {
                min: viewport.right() - unit.right(),
                max: viewport.left() - unit.left(),
            }),
            (unit.height() > viewport.height()).then_some(AxisRange {
                min: viewport.bottom() - unit.bottom(),
                max: viewport.top() - unit.top(),
            }),
        ],
    })
}

fn step_axis(range: Option<AxisRange>, current: f32, delta: f32) -> f32 {
    range.map_or(current, |range| range.step(current, delta))
}

/// One wheel segment, including diagonal input, forms one candidate. The normal
/// clamp may stop an axis, but must not reverse a requested axis or move another.
pub(super) fn scroll_pan(layout: &FullscreenPageLayout, zoom: f32, pan: Vec2, delta: Vec2) -> Vec2 {
    if !pan.is_finite() || !delta.is_finite() || delta == Vec2::ZERO {
        return pan;
    }
    let Some(geometry) = scroll_geometry(layout) else {
        return pan;
    };
    let target = Vec2::new(
        step_axis(geometry.axes[0], pan.x, delta.x),
        step_axis(geometry.axes[1], pan.y, delta.y),
    );
    if target == pan {
        return pan;
    }
    let candidate = clamp_fullscreen_pan_from_layout(layout, zoom, target);
    let movement = candidate - pan;
    let compatible = |axis: usize| {
        if delta[axis] == 0.0 || geometry.axes[axis].is_none() {
            movement[axis] == 0.0
        } else {
            movement[axis] * delta[axis].signum() >= 0.0
        }
    };
    if candidate.is_finite()
        && compatible(0)
        && compatible(1)
        && (0..2).any(|axis| movement[axis] * delta[axis].signum() > 0.0)
    {
        candidate
    } else {
        pan
    }
}

/// N and Z share the same progression rule; only the primary axis changes.
/// A wrapped move is accepted only if the secondary axis really advances after
/// clamping both coordinates together. This prevents asymmetric spreads from
/// resetting the primary axis forever in the same column/row.
pub(super) fn ordered_scroll_pan(
    layout: &FullscreenPageLayout,
    zoom: f32,
    pan: Vec2,
    direction: ReadingDirection,
    order: crate::settings::FullscreenNScrollOrder,
    next: bool,
    step_fraction: f32,
) -> Vec2 {
    ordered_scroll_pan_with_steps(
        layout,
        zoom,
        pan,
        direction,
        order,
        next,
        step_fraction,
        None,
        true,
    )
}

/// Wheel input changes only the primary step; a discrete segment may wrap.
fn ordered_scroll_pan_with_steps(
    layout: &FullscreenPageLayout,
    zoom: f32,
    pan: Vec2,
    direction: ReadingDirection,
    order: crate::settings::FullscreenNScrollOrder,
    next: bool,
    step_fraction: f32,
    primary_fraction: Option<f32>,
    allow_wrap: bool,
) -> Vec2 {
    if !pan.is_finite() {
        return pan;
    }
    let Some(geometry) = scroll_geometry(layout) else {
        return pan;
    };
    let primary = match order.normalized() {
        crate::settings::FullscreenNScrollOrder::HorizontalFirst => 0,
        _ => 1,
    };
    let secondary = 1 - primary;
    let horizontal_sign = match direction {
        ReadingDirection::Ltr => -1.0,
        ReadingDirection::Rtl => 1.0,
    };
    let forward = Vec2::new(horizontal_sign, -1.0);
    let signs = forward * if next { 1.0 } else { -1.0 };
    let steps = geometry.viewport.size() * step_fraction;
    let mut target = pan;
    target[primary] = step_axis(
        geometry.axes[primary],
        pan[primary],
        signs[primary]
            * geometry.viewport.size()[primary]
            * primary_fraction.unwrap_or(step_fraction),
    );
    let candidate = clamp_fullscreen_pan_from_layout(layout, zoom, target);
    if candidate.is_finite()
        && (candidate[primary] - pan[primary]) * signs[primary] > 0.0
        && candidate[secondary] == pan[secondary]
    {
        return candidate;
    }

    if !allow_wrap {
        return pan;
    }

    target = pan;
    target[secondary] = step_axis(
        geometry.axes[secondary],
        pan[secondary],
        signs[secondary] * steps[secondary],
    );
    if target[secondary] == pan[secondary] {
        return pan;
    }
    if let Some(range) = geometry.axes[primary] {
        target[primary] = range.start(signs[primary]);
    }
    let candidate = clamp_fullscreen_pan_from_layout(layout, zoom, target);
    if candidate.is_finite() && (candidate[secondary] - pan[secondary]) * signs[secondary] > 0.0 {
        candidate
    } else {
        pan
    }
}

/// Shared prediction result for a scroll-then-turn input. No edge is retained.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum FsPageScrollOutcome {
    Blocked,
    Move(Vec2),
    Turn,
}

/// The logical pan is already `to`; this is a paint-only, context-owned effect.
#[derive(Clone, Debug)]
pub(crate) struct FsPageScrollAnimation {
    pub(crate) from: Vec2,
    pub(crate) to: Vec2,
    pub(crate) start_time: Option<f64>,
    pub(crate) last_painted_pan: Vec2,
    /// Paired with the pan; absent until an actual unit paint records a sample.
    pub(crate) last_painted_time: Option<f64>,
    pub(crate) origin: FsDrawnUnitStamp,
    pub(crate) viewport: egui::ViewportId,
}

impl FsPageScrollAnimation {
    fn pan_at(&self, time: f64) -> Vec2 {
        let start_time = self
            .start_time
            .expect("destination paint starts the animation");
        let t = ((time - start_time) / 0.2).clamp(0.0, 1.0) as f32;
        let eased = 1.0 - (1.0 - t).powi(3);
        self.from + (self.to - self.from) * eased
    }
}

fn ordered_scroll_outcome(
    layout: &FullscreenPageLayout,
    zoom: f32,
    pan: Vec2,
    direction: ReadingDirection,
    order: crate::settings::FullscreenNScrollOrder,
    next: bool,
    step_fraction: f32,
    primary_fraction: Option<f32>,
    allow_wrap: bool,
    allow_turn: bool,
) -> FsPageScrollOutcome {
    let candidate = ordered_scroll_pan_with_steps(
        layout,
        zoom,
        pan,
        direction,
        order,
        next,
        step_fraction,
        primary_fraction,
        allow_wrap,
    );
    if candidate != pan {
        FsPageScrollOutcome::Move(candidate)
    } else if allow_turn {
        FsPageScrollOutcome::Turn
    } else {
        FsPageScrollOutcome::Blocked
    }
}

/// Proof that the last paint completed the current ordinary paged unit.
/// Owned only by FullscreenPageLayout and retired by begin/clear.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FsDrawnUnitStamp {
    items_generation: u64,
    drawn_zoom: f32,
    geometry: FsScrollGeometryKey,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FsScrollGeometryKey {
    pages: Vec<FsScrollPageGeometry>,
    slice: crate::page_split::PageSlice,
    singleton: SingletonSpreadPlacement,
    direction: ReadingDirection,
    gap: f32,
    viewport: Rect,
    pixels_per_point: f32,
    limits: FullscreenFitScaleLimits,
    free_rotation: f32,
}

#[derive(Clone, Debug, PartialEq)]
struct FsScrollPageGeometry {
    occurrence: SpreadPageOccurrence,
    fit: FullscreenFitInput,
    texture_size: Vec2,
    layout_size: Vec2,
    source_size: Vec2,
    rotation: crate::rotation_db::Rotation,
    trim: Option<Rect>,
}

impl App {
    /// Same ordinary-image domain as click navigation / initial placement, with
    /// capture excluded as it owns pointer input rather than automatic placement.
    pub(crate) fn fs_in_page_scroll_surface(&self) -> bool {
        self.fullscreen_idx
            .and_then(|idx| self.items.get(idx))
            .is_some_and(GridItem::has_page_data)
            && self.reading_flow.is_paged()
            && self.fs_zoom_owner() == FsZoomOwner::Image
            && matches!(self.compare_view_mode, crate::app::CompareViewMode::Off)
            && !self.view_trim_mode
            && !self.is_overlay_edit_mode_active()
            && self.capture_region_selection.is_none()
    }

    /// Build both paint and input keys from the actual rectangle inputs. Missing
    /// material metadata never borrows values from a previous transform.
    fn fs_scroll_geometry_key(
        &self,
        composition: &SpreadDisplayComposition,
        viewport: Rect,
        pixels_per_point: f32,
        pages: Vec<FsScrollPageGeometry>,
    ) -> Option<FsScrollGeometryKey> {
        if !viewport.is_finite()
            || !viewport.is_positive()
            || !pixels_per_point.is_finite()
            || pixels_per_point <= 0.0
            || pages.len() != composition.pages_in_screen_order(self.spread_mode).len()
            || pages
                .iter()
                .zip(composition.pages_in_screen_order(self.spread_mode))
                .any(|(page, expected)| {
                    page.occurrence != expected
                        || [page.texture_size, page.layout_size, page.source_size]
                            .iter()
                            .any(|size| !size.is_finite() || size.x <= 0.0 || size.y <= 0.0)
                })
        {
            return None;
        }
        Some(FsScrollGeometryKey {
            pages,
            slice: self.fullscreen_page_slice,
            singleton: composition.singleton_placement(),
            direction: self
                .spread_mode
                .reading_direction()
                .unwrap_or(self.reading_direction),
            gap: quantize_points_to_physical_pixels(
                self.settings.spread_page_gap_px.min(200) as f32,
                pixels_per_point,
            ),
            viewport,
            pixels_per_point,
            limits: self.fullscreen_fit_scale_limits(pixels_per_point),
            free_rotation: self.fs_free_rotation,
        })
    }

    pub(super) fn record_fs_scroll_drawn_unit(
        &mut self,
        ctx: &egui::Context,
        viewport: Rect,
        composition: &SpreadDisplayComposition,
        sources: &FsNavigatorTextureSources,
    ) {
        self.fullscreen_page_layout.drawn_unit = None;
        if !self.fs_in_page_scroll_surface() {
            return;
        }
        let expected = composition.pages_in_screen_order(self.spread_mode);
        let pages = expected
            .iter()
            .map(|occurrence| {
                let transform = self
                    .fullscreen_page_layout
                    .page_by_occurrence(*occurrence)?
                    .transform;
                let source = sources
                    .pages
                    .iter()
                    .find(|source| source.occurrence == *occurrence)?;
                if source.provenance != FsDisplayUnitPageProvenance::Live {
                    return None;
                }
                let view = if expected.len() == 2 {
                    crate::material_display_basis::MaterialDisplayView::Spread
                } else {
                    crate::material_display_basis::MaterialDisplayView::Single
                };
                Some(FsScrollPageGeometry {
                    occurrence: *occurrence,
                    fit: transform.fit_input,
                    texture_size: transform.texture_size,
                    layout_size: Self::fs_material_layout_size(&source.resource, view)?,
                    source_size: transform.source_size,
                    rotation: transform.rotation,
                    trim: transform.content_bbox,
                })
            })
            .collect::<Option<Vec<_>>>();
        let Some(pages) = pages else {
            return;
        };
        if self.fullscreen_page_layout.displayed_pages().count() != expected.len() {
            return;
        }
        let Some(drawn_zoom) = self
            .fullscreen_page_layout
            .displayed_pages()
            .next()
            .and_then(|page| normal_zoom_pan(page.transform).map(|(zoom, _)| zoom))
        else {
            return;
        };
        if !drawn_zoom.is_finite()
            || drawn_zoom <= 0.0
            || self.fullscreen_page_layout.displayed_pages().any(|page| {
                normal_zoom_pan(page.transform).is_none_or(|(zoom, _)| zoom != drawn_zoom)
            })
        {
            return;
        }
        if let Some(geometry) =
            self.fs_scroll_geometry_key(composition, viewport, ctx.pixels_per_point(), pages)
        {
            self.fullscreen_page_layout.drawn_unit = Some(FsDrawnUnitStamp {
                items_generation: self.items_generation,
                drawn_zoom,
                geometry,
            });
            // Retargeting uses the pan actually submitted by the owning paint,
            // never a sample of a newer or older input pass's clock.
            if ctx.viewport_id() == self.fs_page_scroll_destination_viewport()
                && let Some(animation) = self.fs_page_scroll_animation.as_mut()
                && animation.viewport == ctx.viewport_id()
                && animation.to == self.fs_pan
                && animation.start_time.is_some()
                && self.fullscreen_page_layout.drawn_unit.as_ref() == Some(&animation.origin)
                && let Some((_, pan)) = self
                    .fullscreen_page_layout
                    .displayed_pages()
                    .next()
                    .and_then(|page| normal_zoom_pan(page.transform))
            {
                animation.last_painted_pan = pan;
                animation.last_painted_time = Some(ctx.input(|input| input.time));
            }
        }
    }

    fn fs_scroll_input_composition(&self) -> Option<SpreadDisplayComposition> {
        let idx = self.fullscreen_idx?;
        let base = self.peek_spread_display_composition_for_anchor(idx)?;
        let demand = base.presentation_demand(self.spread_mode, self.fullscreen_page_slice);
        let crate::book_composition::PresentationDecision::Allowed(allowed) =
            self.book_presentation_allowed(&demand)
        else {
            return None;
        };
        base.permitted_projection(self.spread_mode, &allowed)
    }

    #[cfg(test)]
    fn fs_scroll_input_geometry(&self, ctx: &egui::Context) -> Option<FsScrollGeometryKey> {
        let composition = self.fs_scroll_input_composition()?;
        self.fs_scroll_input_geometry_for_unit(ctx, &composition)
    }

    fn fs_scroll_resident_auto_trim(&self, idx: usize) -> Option<Option<Rect>> {
        #[cfg(windows)]
        if self.viewer_session_is_detached()
            && let Some(window) = self.detached_viewer_window_id()
            && let Some(bbox) = self.detached_window_runtime_trim_bbox(window, idx)
        {
            return Some(bbox);
        }
        // RAW previews are analyzed by ordinary paint but are not baked into
        // detached runtime. The pixel cache proves the exact painted answer.
        self.resident_pixel_auto_trim(idx).ok().or_else(|| {
            // Ordinary auto trim deliberately does not analyze animated
            // entries. This resident variant proves no trim; it is not a
            // missing static-pixel answer disguised as None.
            matches!(self.fs_cache.get(&idx), Some(FsCacheEntry::Animated { .. })).then_some(None)
        })
    }

    pub(super) fn fs_navigation_front_menu_presented(&mut self, ctx: &egui::Context) -> bool {
        let viewport = self.fs_page_scroll_destination_viewport();
        // Preserve the ingress viewport's existing front-UI gate as well as
        // checking the fullscreen destination of a ROOT-forwarded key.
        if viewport != ctx.viewport_id() && self.fs_front_menu_presented(ctx, ctx.content_rect()) {
            return true;
        }
        let rect = rounded_content_rect_for_viewport(ctx, viewport);
        self.fs_front_menu_presented_for_viewport(ctx, rect, viewport)
    }

    pub(super) fn fs_page_scroll_destination_viewport(&self) -> egui::ViewportId {
        if self.fullscreen_embedded_still_active() {
            egui::ViewportId::ROOT
        } else {
            self.fullscreen_viewport_id()
        }
    }

    fn fs_scroll_resident_trim(
        &self,
        idx: usize,
        side: Option<crate::view_trim::ViewTrimSpreadSide>,
    ) -> Option<Option<Rect>> {
        if self.effective_view_trim_apply_mode_for_idx(idx)
            == crate::view_trim::ViewTrimApplyMode::Auto
        {
            self.fs_scroll_resident_auto_trim(idx)
        } else {
            self.resident_canonical_trim(idx, side).ok()
        }
    }

    fn fs_scroll_input_geometry_for_unit(
        &self,
        ctx: &egui::Context,
        composition: &SpreadDisplayComposition,
    ) -> Option<FsScrollGeometryKey> {
        let idx = self.fullscreen_idx?;
        // ROOT can dispatch keys for another viewport. Read the destination's
        // current size and DPI, never the handler Context's current viewport.
        let target = self.fs_page_scroll_destination_viewport();
        let full_rect = rounded_content_rect_for_viewport(ctx, target);
        let ppp = ctx.input_for(target, |input| input.pixels_per_point());
        if !full_rect.is_finite() || !full_rect.is_positive() {
            return None;
        }
        // Match the ordinary viewport using only its already prepared chrome.
        // fullscreen_media_rect can populate seek/navigation caches on a miss.
        let seek_allowed = self.fullscreen_seek_overlay_allowed(idx, false);
        let strip_has_content = if self.settings.still_seek_strip_visible && seek_allowed {
            let navigation = &self.viewer_navigation_caches;
            if navigation.source != Some((self.items_generation, self.reading_flow)) {
                return None;
            }
            navigation
                .fs_seek_info
                .as_ref()
                .filter(|(anchor, _)| *anchor == idx)?
                .1
                .has_page_strip_content()
        } else {
            false
        };
        let chrome = FsNavigationStillChromeInputs {
            top_bar_eligible: self.fullscreen_top_bar_chrome_allowed(idx),
            seek_eligible: seek_allowed,
            strip_has_content,
            info_panel_eligible: self.still_info_panel_eligible_for_idx(idx, false),
        };
        let seek = self.still_seek_geometry_for_navigation_chrome(full_rect, chrome);
        let right = self.locked_info_panel_reserved_width_for_eligibility(
            full_rect,
            seek.total_height,
            chrome.info_panel_eligible,
        );
        let viewport = seek.media_rect(full_rect, right);
        let occurrences = composition.pages_in_screen_order(self.spread_mode);
        let spread = occurrences.len() == 2;
        let mode = self.effective_fullscreen_fit_mode();
        let mut trims = Vec::new();
        if spread {
            let left = occurrences[0].idx;
            let right = occurrences[1].idx;
            let left_rot = self.peek_rotation(left)?;
            let right_rot = self.peek_rotation(right)?;
            if left_rot.is_none() && right_rot.is_none() {
                let (l, r) = match self.resolved_view_trim_base_mode() {
                    crate::view_trim::ViewTrimApplyMode::None => (None, None),
                    crate::view_trim::ViewTrimApplyMode::Auto => {
                        crate::view_trim::harmonize_spread_auto_bboxes(
                            self.fs_scroll_resident_auto_trim(left)?,
                            self.fs_scroll_resident_auto_trim(right)?,
                        )
                    }
                    _ => (
                        self.view_trim_spread_bbox(
                            left,
                            crate::view_trim::ViewTrimSpreadSide::Left,
                        ),
                        self.view_trim_spread_bbox(
                            right,
                            crate::view_trim::ViewTrimSpreadSide::Right,
                        ),
                    ),
                };
                trims.extend([l, r]);
            } else {
                trims.extend([
                    if left_rot.is_none() {
                        self.fs_scroll_resident_trim(
                            left,
                            Some(crate::view_trim::ViewTrimSpreadSide::Left),
                        )?
                    } else {
                        None
                    },
                    if right_rot.is_none() {
                        self.fs_scroll_resident_trim(
                            right,
                            Some(crate::view_trim::ViewTrimSpreadSide::Right),
                        )?
                    } else {
                        None
                    },
                ]);
            }
        }
        let pages = occurrences
            .into_iter()
            .enumerate()
            .map(|(side, occurrence)| {
                let idx = occurrence.idx;
                let resource = self.fs_layout_material_for_idx(
                    idx,
                    !self.original_preview_active_for_frame(ctx, idx),
                )?;
                let view = if spread {
                    crate::material_display_basis::MaterialDisplayView::Spread
                } else {
                    crate::material_display_basis::MaterialDisplayView::Single
                };
                let texture_size = resource.full_source_size_vec2();
                let rotation = self.peek_rotation(idx)?;
                Some(FsScrollPageGeometry {
                    occurrence,
                    fit: self.fs_material_fit_input(mode, &resource, view).ok()?,
                    texture_size,
                    layout_size: Self::fs_material_layout_size(&resource, view)?,
                    source_size: self
                        .fs_page_coordinate_source_size(idx)
                        .or_else(|| {
                            resource.source_coverage().coordinate_dims().map(|dims| {
                                let (w, h) = dims.as_tuple();
                                egui::vec2(w as f32, h as f32)
                            })
                        })
                        .unwrap_or(texture_size),
                    rotation,
                    trim: if spread {
                        trims[side]
                    } else {
                        let trim = self
                            .fs_scroll_resident_trim(idx, self.composed_blank_spread_side(idx))?;
                        self.fs_page_content_bbox(idx, rotation, trim)
                    },
                })
            })
            .collect::<Option<Vec<_>>>()?;
        self.fs_scroll_geometry_key(composition, viewport, ppp, pages)
    }

    pub(super) fn fs_scroll_layout_matches(&self, ctx: &egui::Context) -> bool {
        self.fs_scroll_layout_matches_at_zoom(ctx, self.fs_zoom_pan().map_or(1.0, |(zoom, _)| zoom))
    }

    fn fs_scroll_layout_matches_at_zoom(&self, ctx: &egui::Context, zoom: f32) -> bool {
        if !self
            .fullscreen_page_layout
            .drawn_unit
            .as_ref()
            .is_some_and(|stamp| {
                stamp.drawn_zoom == zoom && stamp.items_generation == self.items_generation
            })
            || !self.fs_in_page_scroll_surface()
            || self.fs_navigation_sequence_blocks_new_target()
            || self.page_turn_barrier_active()
        {
            return false;
        }
        let Some(composition) = self.fs_scroll_input_composition() else {
            return false;
        };
        let stamp = self
            .fullscreen_page_layout
            .drawn_unit
            .as_ref()
            .expect("checked stamp");
        if stamp.geometry.slice != self.fullscreen_page_slice
            || stamp.geometry.singleton != composition.singleton_placement()
            || stamp
                .geometry
                .pages
                .iter()
                .map(|page| page.occurrence)
                .collect::<Vec<_>>()
                != composition.pages_in_screen_order(self.spread_mode)
        {
            return false;
        }
        let Some(key) = self.fs_scroll_input_geometry_for_unit(ctx, &composition) else {
            return false;
        };
        self.fullscreen_page_layout
            .drawn_unit
            .as_ref()
            .is_some_and(|stamp| {
                stamp.items_generation == self.items_generation && stamp.geometry == key
            })
    }

    pub(super) fn apply_fs_page_scroll_key(&mut self, ctx: &egui::Context, action: KeyAction) {
        if !self.fs_scroll_layout_matches(ctx) {
            return;
        }
        let layout = &self.fullscreen_page_layout;
        let zoom = layout
            .drawn_unit
            .as_ref()
            .expect("matched layout")
            .drawn_zoom;
        let pan = match action {
            KeyAction::FsPageNScrollNext | KeyAction::FsPageNScrollPrev => ordered_scroll_pan(
                layout,
                zoom,
                self.fs_pan,
                self.spread_mode
                    .reading_direction()
                    .unwrap_or(self.reading_direction),
                self.settings.fullscreen_n_scroll_order,
                action == KeyAction::FsPageNScrollNext,
                self.settings
                    .fullscreen_n_scroll_step_percent
                    .clamp(10, 100) as f32
                    / 100.0,
            ),
            _ => {
                let viewport = layout
                    .drawn_unit
                    .as_ref()
                    .expect("matched layout")
                    .geometry
                    .viewport;
                let step = viewport.size()
                    * (self
                        .settings
                        .continuous_reading_key_scroll_percent
                        .clamp(1, 100) as f32
                        / 100.0);
                let delta = match action {
                    KeyAction::FsPageScrollUp => egui::vec2(0.0, step.y),
                    KeyAction::FsPageScrollDown => egui::vec2(0.0, -step.y),
                    KeyAction::FsPageScrollLeft => egui::vec2(step.x, 0.0),
                    KeyAction::FsPageScrollRight => egui::vec2(-step.x, 0.0),
                    _ => return,
                };
                scroll_pan(layout, zoom, self.fs_pan, delta)
            }
        };
        self.write_fs_page_scroll_pan_with_animation(ctx, pan, true);
    }

    /// Common admission also applies to terminal-load failures: failure replaces
    /// only the drawn-unit proof, never a paper-turn or navigation barrier.
    fn fs_scroll_turn_composition(&self) -> Option<SpreadDisplayComposition> {
        if !self.fs_in_page_scroll_surface()
            || self.fs_navigation_sequence_blocks_new_target()
            || self.page_turn_barrier_active()
        {
            return None;
        }
        self.fs_scroll_input_composition()
    }

    fn fs_scroll_unit_load_failed(&self, composition: &SpreadDisplayComposition) -> bool {
        composition
            .pages_in_screen_order(self.spread_mode)
            .iter()
            .any(|page| {
                self.fs_page_load_state(page.idx) == crate::app::FsPageLoadState::LoadFailed
            })
    }

    pub(super) fn fs_page_scroll_or_turn_key(
        &mut self,
        ctx: &egui::Context,
        next: bool,
        kind: crate::keymap::PageTurnPressKind,
    ) -> FsPageNavRequest {
        let Some(composition) = self.fs_scroll_turn_composition() else {
            return FsPageNavRequest::default();
        };
        let allow_turn = kind == crate::keymap::PageTurnPressKind::InitialPress;
        let outcome = if self.fs_scroll_layout_matches(ctx) {
            ordered_scroll_outcome(
                &self.fullscreen_page_layout,
                self.fs_page_scroll_drawn_zoom(),
                self.fs_pan,
                self.spread_mode
                    .reading_direction()
                    .unwrap_or(self.reading_direction),
                self.settings.fullscreen_n_scroll_order,
                next,
                self.settings
                    .fullscreen_n_scroll_step_percent
                    .clamp(10, 100) as f32
                    / 100.0,
                None,
                true,
                allow_turn,
            )
        } else if allow_turn && self.fs_scroll_unit_load_failed(&composition) {
            FsPageScrollOutcome::Turn
        } else {
            FsPageScrollOutcome::Blocked
        };
        match outcome {
            FsPageScrollOutcome::Move(pan) => {
                self.write_fs_page_scroll_pan_with_animation(ctx, pan, true);
                FsPageNavRequest::default()
            }
            FsPageScrollOutcome::Turn => FsPageNavRequest {
                nav: self.spread_page_nav(if next { 1 } else { -1 }),
                landing: Some(if next {
                    FsPageLanding::Start
                } else {
                    FsPageLanding::End
                }),
            },
            FsPageScrollOutcome::Blocked => FsPageNavRequest::default(),
        }
    }

    /// Used unchanged by frame prediction and actual segment application.
    pub(super) fn fs_page_scroll_or_turn_wheel(
        &self,
        ctx: &egui::Context,
        zoom: f32,
        pan: Vec2,
        delta: Vec2,
        moved_in_frame: bool,
    ) -> FsPageScrollOutcome {
        if !pan.is_finite() || !delta.is_finite() || delta.y == 0.0 {
            return FsPageScrollOutcome::Blocked;
        }
        let Some(composition) = self.fs_scroll_turn_composition() else {
            return FsPageScrollOutcome::Blocked;
        };
        let discrete = delta.y.abs() > 0.5;
        let allow_turn = discrete && !moved_in_frame;
        let effective_zoom = Self::fs_zoom_pan_for(zoom, pan).map_or(1.0, |(zoom, _)| zoom);
        if !self.fs_scroll_layout_matches_at_zoom(ctx, effective_zoom) {
            return if allow_turn && self.fs_scroll_unit_load_failed(&composition) {
                FsPageScrollOutcome::Turn
            } else {
                FsPageScrollOutcome::Blocked
            };
        }
        ordered_scroll_outcome(
            &self.fullscreen_page_layout,
            effective_zoom,
            pan,
            self.spread_mode
                .reading_direction()
                .unwrap_or(self.reading_direction),
            self.settings.fullscreen_n_scroll_order,
            delta.y < 0.0,
            self.settings
                .fullscreen_n_scroll_step_percent
                .clamp(10, 100) as f32
                / 100.0,
            Some(
                delta.y.abs() / CONTINUOUS_READING_WHEEL_REFERENCE_DELTA
                    * self
                        .settings
                        .continuous_reading_wheel_scroll_percent
                        .clamp(1, 100) as f32
                    / 100.0,
            ),
            discrete,
            allow_turn,
        )
    }

    /// Input admission is structural. Only the destination paint owns time.
    fn fs_page_scroll_animation_matches(
        &self,
        ctx: &egui::Context,
        animation: &FsPageScrollAnimation,
    ) -> bool {
        self.settings.fullscreen_smooth_page_scroll
            && animation.to == self.fs_pan
            && animation.viewport == self.fs_page_scroll_destination_viewport()
            && self.fullscreen_page_layout.drawn_unit.as_ref() == Some(&animation.origin)
            && self.fs_scroll_layout_matches(ctx)
    }

    /// Called once after input, before the destination paint invalidates its old
    /// stamp. Other passes neither start nor expire this context-owned effect.
    pub(super) fn retire_fs_page_scroll_animation(&mut self, ctx: &egui::Context) {
        let viewport = self.fs_page_scroll_destination_viewport();
        if ctx.viewport_id() != viewport {
            return;
        }
        let Some(animation) = self.fs_page_scroll_animation.as_ref() else {
            return;
        };
        if !self.fs_page_scroll_animation_matches(ctx, animation) {
            self.fs_page_scroll_animation = None;
            return;
        }
        let now = ctx.input(|input| input.time);
        if !now.is_finite() {
            self.fs_page_scroll_animation = None;
            return;
        }
        let animation = self.fs_page_scroll_animation.as_mut().unwrap();
        let start_time = *animation.start_time.get_or_insert(now);
        if now < start_time || now - start_time >= 0.2 {
            self.fs_page_scroll_animation = None;
            return;
        }
        ctx.request_repaint_of(viewport);
    }

    pub(super) fn fs_page_scroll_display_pan(&self, ctx: &egui::Context) -> Vec2 {
        self.fs_page_scroll_animation
            .as_ref()
            .filter(|animation| {
                animation.to == self.fs_pan
                    && animation.viewport == self.fs_page_scroll_destination_viewport()
                    && animation.viewport == ctx.viewport_id()
            })
            .map_or(self.fs_pan, |animation| {
                if animation.start_time.is_some() {
                    animation.pan_at(ctx.input(|input| input.time))
                } else {
                    animation.from
                }
            })
    }

    pub(super) fn write_fs_page_scroll_pan_with_animation(
        &mut self,
        ctx: &egui::Context,
        pan: Vec2,
        smooth_allowed: bool,
    ) {
        if pan == self.fs_pan {
            return;
        }
        let viewport = self.fs_page_scroll_destination_viewport();
        let previous = self
            .fs_page_scroll_animation
            .as_ref()
            .filter(|animation| self.fs_page_scroll_animation_matches(ctx, animation));
        let from = previous.map_or(self.fs_pan, |animation| {
            if animation.start_time.is_some() {
                animation.last_painted_pan
            } else {
                animation.from
            }
        });
        // Reuse a recorded paint sample, not the input pass's clock. A request
        // that has not painted yet stays unstarted when its target changes.
        let start_time =
            previous.and_then(|animation| animation.start_time.and(animation.last_painted_time));
        let last_painted_time = previous.and_then(|animation| animation.last_painted_time);
        let origin = self.fullscreen_page_layout.drawn_unit.clone();
        self.write_fs_page_scroll_pan(pan);
        self.fs_page_scroll_animation =
            if smooth_allowed && self.settings.fullscreen_smooth_page_scroll && from != self.fs_pan
            {
                origin.map(|origin| FsPageScrollAnimation {
                    from,
                    to: self.fs_pan,
                    start_time,
                    last_painted_pan: from,
                    last_painted_time,
                    origin,
                    viewport,
                })
            } else {
                None
            };
        ctx.request_repaint_of(viewport);
    }

    pub(super) fn fs_page_scroll_drawn_zoom(&self) -> f32 {
        self.fullscreen_page_layout
            .drawn_unit
            .as_ref()
            .expect("matched scroll layout")
            .drawn_zoom
    }

    /// Keep the painted scale when a new pan crosses fs_zoom_pan_for's near-fit
    /// normalization boundary. This setter belongs only to page-local scrolling.
    pub(super) fn write_fs_page_scroll_pan(&mut self, pan: Vec2) {
        if pan == self.fs_pan {
            return;
        }
        let zoom = self
            .fullscreen_page_layout
            .drawn_unit
            .as_ref()
            .expect("matched scroll layout")
            .drawn_zoom;
        self.fs_zoom = zoom;
        self.set_fs_pan_from_input(zoom, pan);
    }

    pub(super) fn fs_wheel_segment_in_page_scroll(
        &self,
        wheel: crate::wheel_input::WheelInput,
        seek_hover: bool,
    ) -> FsWheelInPageClass {
        use crate::settings::FullscreenWheelAction;
        if wheel.modifiers.ctrl
            || wheel.modifiers.alt
            || wheel.modifiers.command
            || seek_hover
            || !(self.fs_context_menu_idx.is_some() || self.fs_in_page_scroll_surface())
        {
            return FsWheelInPageClass::Legacy;
        }
        match self.settings.fullscreen_wheel_action.normalized() {
            FullscreenWheelAction::InPageScroll => FsWheelInPageClass::InPageScroll(wheel.delta),
            FullscreenWheelAction::ScrollThenPageTurn if wheel.delta.y != 0.0 => {
                FsWheelInPageClass::ScrollThenPageTurn(wheel.delta)
            }
            FullscreenWheelAction::Zoom if !wheel.modifiers.shift => {
                FsWheelInPageClass::SettingZoom
            }
            _ => FsWheelInPageClass::Legacy,
        }
    }

    pub(super) fn fs_page_scroll_wheel_pan(
        &mut self,
        ctx: &egui::Context,
        zoom: f32,
        pan: Vec2,
        delta: Vec2,
    ) -> Vec2 {
        let effective_zoom = Self::fs_zoom_pan_for(zoom, pan).map_or(1.0, |(zoom, _)| zoom);
        if !self.fs_scroll_layout_matches_at_zoom(ctx, effective_zoom) {
            return pan;
        }
        let viewport = self
            .fullscreen_page_layout
            .drawn_unit
            .as_ref()
            .expect("matched layout")
            .geometry
            .viewport;
        let delta = delta / CONTINUOUS_READING_WHEEL_REFERENCE_DELTA
            * viewport.size()
            * (self
                .settings
                .continuous_reading_wheel_scroll_percent
                .clamp(1, 100) as f32
                / 100.0);
        scroll_pan(&self.fullscreen_page_layout, effective_zoom, pan, delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rotation_db::Rotation;
    use crate::settings::FullscreenNScrollOrder::{HorizontalFirst, VerticalFirst};

    pub(super) fn viewport() -> Rect {
        Rect::from_min_size(egui::pos2(30.0, 50.0), Vec2::splat(1000.0))
    }

    pub(super) fn page_input(
        idx: usize,
        rect: Rect,
        zoom: f32,
        pan: Vec2,
    ) -> DisplayedImageTransformInput {
        DisplayedImageTransformInput {
            page_idx: idx,
            viewport_rect: viewport(),
            source_size: rect.size(),
            texture_size: rect.size(),
            rotation: Rotation::None,
            free_rotation_rad: 0.0,
            content_bbox: None,
            fit_mode: FullscreenFitInput::legacy(FullscreenFitMode::Original),
            fit_scale_limits: FullscreenFitScaleLimits::default(),
            pixels_per_point: 1.0,
            placement: ResolvedDisplayPlacement::Normal {
                zoom_pan: Some((zoom, pan)),
            },
            pixel_fit: RectPixelFit::Proportional,
        }
    }

    pub(super) fn layout_from_pages(pages: &[DisplayedImageTransform]) -> FullscreenPageLayout {
        let mut layout = FullscreenPageLayout::default();
        layout.begin(if pages.len() == 1 {
            FullscreenPageLayoutKind::Single
        } else {
            FullscreenPageLayoutKind::Spread
        });
        for page in pages {
            layout.push(*page);
        }
        layout
    }

    pub(super) fn single(size: Vec2) -> FullscreenPageLayout {
        let rect = Rect::from_center_size(viewport().center(), size);
        layout_from_pages(&[DisplayedImageTransform::from_resolved_rect(
            page_input(0, rect, 1.0, Vec2::ZERO),
            rect,
        )
        .unwrap()])
    }

    pub(super) fn close(actual: Vec2, expected: Vec2) {
        assert!(
            (actual - expected).abs().max_elem() < 0.001,
            "actual={actual:?}, expected={expected:?}"
        );
    }

    /// Check the actual public input writer, not only a second pure clamp.
    fn written(app: &mut App, layout: &FullscreenPageLayout, zoom: f32, pan: Vec2) -> Vec2 {
        app.fullscreen_page_layout = layout.clone();
        app.set_fs_pan_from_input(zoom, pan);
        assert_eq!(app.fs_pan, pan, "accepted pan must be clamp-idempotent");
        app.fs_pan
    }

    fn scroll(
        app: &mut App,
        layout: &FullscreenPageLayout,
        zoom: f32,
        pan: Vec2,
        delta: Vec2,
    ) -> Vec2 {
        written(app, layout, zoom, scroll_pan(layout, zoom, pan, delta))
    }

    fn ordered(
        app: &mut App,
        layout: &FullscreenPageLayout,
        pan: Vec2,
        direction: ReadingDirection,
        order: crate::settings::FullscreenNScrollOrder,
        next: bool,
    ) -> Vec2 {
        written(
            app,
            layout,
            1.0,
            ordered_scroll_pan(layout, 1.0, pan, direction, order, next, 0.85),
        )
    }

    #[test]
    fn axes_overflow_independently_and_stop_at_aligned_edges() {
        let mut app = crate::app::setup_app_for_test();
        for (size, expected) in [
            (Vec2::new(800.0, 3000.0), Vec2::new(0.0, -1000.0)),
            (Vec2::new(3000.0, 800.0), Vec2::new(-1000.0, 0.0)),
            (Vec2::splat(3000.0), Vec2::splat(-1000.0)),
            (Vec2::splat(800.0), Vec2::ZERO),
        ] {
            let layout = single(size);
            let end = scroll(&mut app, &layout, 1.0, Vec2::ZERO, Vec2::splat(-5000.0));
            close(end, expected);
            close(
                scroll(&mut app, &layout, 1.0, end, Vec2::splat(-160.0)),
                end,
            );
            close(
                scroll(&mut app, &layout, 1.0, end, Vec2::splat(5000.0)),
                -expected,
            );
        }
    }

    #[test]
    fn outside_edge_moves_only_inward_without_snapping_to_near_edge() {
        let mut app = crate::app::setup_app_for_test();
        let layout = single(Vec2::splat(3000.0));
        for sign in [-1.0, 1.0] {
            let pan = Vec2::new(sign * 1200.0, 0.0);
            close(
                scroll(&mut app, &layout, 1.0, pan, Vec2::new(sign * 160.0, 0.0)),
                pan,
            );
            close(
                scroll(&mut app, &layout, 1.0, pan, Vec2::new(-sign * 50.0, 0.0)),
                Vec2::new(sign * 1150.0, 0.0),
            );
            close(
                scroll(&mut app, &layout, 1.0, pan, Vec2::new(-sign * 5000.0, 0.0)),
                Vec2::new(-sign * 1000.0, 0.0),
            );
        }
    }

    #[test]
    fn diagonal_wheel_keeps_moving_the_axis_that_has_not_reached_its_edge() {
        let mut app = crate::app::setup_app_for_test();
        let layout = single(Vec2::splat(3000.0));
        close(
            scroll(
                &mut app,
                &layout,
                1.0,
                Vec2::new(-1000.0, 0.0),
                Vec2::new(-200.0, -0.1),
            ),
            Vec2::new(-1000.0, -0.1),
        );
        close(
            scroll(&mut app, &layout, 1.0, Vec2::ZERO, Vec2::new(0.1, -0.5)),
            Vec2::new(0.1, -0.5),
        );
    }

    #[test]
    fn scroll_geometry_removes_drawn_pan_without_scaling_the_unit() {
        let mut app = crate::app::setup_app_for_test();
        let base_pan = Vec2::new(110.0, 70.0);
        let rect = Rect::from_center_size(viewport().center() + base_pan, Vec2::splat(2000.0));
        let page =
            DisplayedImageTransform::from_resolved_rect(page_input(0, rect, 2.0, base_pan), rect)
                .unwrap();
        let layout = layout_from_pages(&[page]);
        let unit = page_rect_at_pan_zero(&page).unwrap();
        close(unit.size(), Vec2::splat(2000.0));
        close(unit.center().to_vec2(), viewport().center().to_vec2());
        close(
            scroll(&mut app, &layout, 2.0, Vec2::ZERO, Vec2::splat(-1000.0)),
            Vec2::splat(-500.0),
        );
    }

    #[test]
    fn ordered_primary_and_wrap_moves_share_n_and_z_rules_in_both_directions() {
        let mut app = crate::app::setup_app_for_test();
        let layout = single(Vec2::splat(3000.0));
        for direction in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
            let horizontal = if direction == ReadingDirection::Ltr {
                -1.0
            } else {
                1.0
            };
            for order in [VerticalFirst, HorizontalFirst] {
                let primary = if order == VerticalFirst { 1 } else { 0 };
                let secondary = 1 - primary;
                let forward = Vec2::new(horizontal, -1.0);
                for next in [true, false] {
                    let signs = forward * if next { 1.0 } else { -1.0 };
                    let start = -signs * 1000.0;
                    let mut expected = start;
                    expected[primary] += signs[primary] * 850.0;
                    close(
                        ordered(&mut app, &layout, start, direction, order, next),
                        expected,
                    );
                    let mut edge = start;
                    edge[primary] = signs[primary] * 1000.0;
                    expected = start;
                    expected[secondary] += signs[secondary] * 850.0;
                    close(
                        ordered(&mut app, &layout, edge, direction, order, next),
                        expected,
                    );
                    let end = signs * 1000.0;
                    close(ordered(&mut app, &layout, end, direction, order, next), end);
                }
            }
        }
    }

    #[test]
    fn ordered_scroll_without_primary_overflow_uses_only_secondary_axis() {
        let mut app = crate::app::setup_app_for_test();
        for order in [VerticalFirst, HorizontalFirst] {
            let layout = single(if order == VerticalFirst {
                Vec2::new(3000.0, 800.0)
            } else {
                Vec2::new(800.0, 3000.0)
            });
            let expected = if order == VerticalFirst {
                Vec2::new(-850.0, 0.0)
            } else {
                Vec2::new(0.0, -850.0)
            };
            close(
                ordered(
                    &mut app,
                    &layout,
                    Vec2::ZERO,
                    ReadingDirection::Ltr,
                    order,
                    true,
                ),
                expected,
            );
            // The opposite order sees that same overflowing axis as primary.
            let opposite = if order == VerticalFirst {
                HorizontalFirst
            } else {
                VerticalFirst
            };
            close(
                ordered(
                    &mut app,
                    &layout,
                    Vec2::ZERO,
                    ReadingDirection::Ltr,
                    opposite,
                    true,
                ),
                expected,
            );
        }
    }

    pub(super) fn asymmetric_spread(trim_right: bool, transpose: bool) -> FullscreenPageLayout {
        let center = viewport().center();
        let mut left = Rect::from_min_size(
            center + Vec2::new(-1010.0, -1500.0),
            Vec2::new(1000.0, 3000.0),
        );
        let mut right =
            Rect::from_min_size(center + Vec2::new(10.0, -505.0), Vec2::new(1000.0, 1010.0));
        if transpose {
            let transpose_rect = |rect: Rect| {
                Rect::from_min_max(
                    center + (rect.min - center).yx(),
                    center + (rect.max - center).yx(),
                )
            };
            left = transpose_rect(left);
            right = transpose_rect(right);
        }
        let left_page =
            DisplayedImageTransform::from_resolved_rect(page_input(0, left, 1.0, Vec2::ZERO), left)
                .unwrap();
        let mut right_input = page_input(1, right, 1.0, Vec2::ZERO);
        if trim_right {
            right_input.content_bbox = Some(if transpose {
                Rect::from_min_max(egui::pos2(0.2, 0.0), egui::pos2(1.0, 1.0))
            } else {
                Rect::from_min_max(egui::pos2(0.0, 0.2), egui::pos2(1.0, 1.0))
            });
        }
        let right_page = DisplayedImageTransform::from_resolved_rect(right_input, right).unwrap();
        layout_from_pages(&[left_page, right_page])
    }

    #[test]
    fn asymmetric_original_spread_never_repeats_a_column_or_row_after_clamp() {
        let mut app = crate::app::setup_app_for_test();
        for trim in [false, true] {
            for transpose in [false, true] {
                let layout = asymmetric_spread(trim, transpose);
                let order = if transpose {
                    HorizontalFirst
                } else {
                    VerticalFirst
                };
                let primary = if transpose { 0 } else { 1 };
                let secondary = 1 - primary;
                let geometry = scroll_geometry(&layout).unwrap();
                let mut pan =
                    Vec2::new(geometry.axes[0].unwrap().max, geometry.axes[1].unwrap().max);
                let mut stopped = false;
                for _ in 0..40 {
                    let result =
                        ordered(&mut app, &layout, pan, ReadingDirection::Ltr, order, true);
                    assert!(result[secondary] <= pan[secondary]);
                    if result[primary] > pan[primary] {
                        assert!(
                            result[secondary] < pan[secondary],
                            "a wrap must advance to a new column/row"
                        );
                    }
                    if result == pan {
                        stopped = true;
                        break;
                    }
                    pan = result;
                }
                assert!(
                    stopped,
                    "asymmetric spread must reach a fixed terminal position"
                );
                close(
                    ordered(&mut app, &layout, pan, ReadingDirection::Ltr, order, true),
                    pan,
                );
            }
        }
    }

    #[test]
    fn clamp_cannot_reverse_wheel_or_correct_an_unrequested_axis() {
        let mut app = crate::app::setup_app_for_test();
        let layout = asymmetric_spread(true, false);
        // Only the short right page is visible horizontally; its bottom has
        // almost left the viewport. The closest clamp correction is sideways.
        let pan = Vec2::new(-450.0, -950.0);
        let target = Vec2::new(-450.0, -1000.0);
        let clamped = clamp_fullscreen_pan_from_layout(&layout, 1.0, target);
        assert!(clamped.x > pan.x || clamped.y > target.y);
        close(
            scroll(&mut app, &layout, 1.0, pan, Vec2::new(0.0, -50.0)),
            pan,
        );
        close(
            scroll(&mut app, &layout, 1.0, pan, Vec2::new(-10.0, -50.0)),
            pan,
        );
    }

    #[test]
    fn white_companion_counts_for_edges_but_vacant_slot_does_not() {
        let mut app = crate::app::setup_app_for_test();
        for side in [
            SingletonSpreadPlacement::Left,
            SingletonSpreadPlacement::LeftWhite,
        ] {
            let rect = Rect::from_center_size(
                viewport().center() - Vec2::new(410.0, 0.0),
                Vec2::splat(800.0),
            );
            let mut input = page_input(0, rect, 1.0, Vec2::ZERO);
            input.placement = ResolvedDisplayPlacement::SingletonSpread {
                side,
                gap: 20.0,
                zoom_pan: Some((1.0, Vec2::ZERO)),
            };
            let layout =
                layout_from_pages(&[
                    DisplayedImageTransform::from_resolved_rect(input, rect).unwrap()
                ]);
            let result = scroll(&mut app, &layout, 1.0, Vec2::ZERO, Vec2::new(-1000.0, 0.0));
            close(
                result,
                if side.has_white_companion() {
                    Vec2::new(-310.0, 0.0)
                } else {
                    Vec2::ZERO
                },
            );
            if side.has_white_companion() {
                // A fresh paint at doubled zoom retains the fixed 20-point gap.
                let zoomed_rect = Rect::from_center_size(
                    viewport().center() - Vec2::new(810.0, 0.0),
                    Vec2::splat(1600.0),
                );
                let mut zoomed = input;
                zoomed.placement = ResolvedDisplayPlacement::SingletonSpread {
                    side,
                    gap: 20.0,
                    zoom_pan: Some((2.0, Vec2::ZERO)),
                };
                let zoomed_layout =
                    layout_from_pages(&[DisplayedImageTransform::from_resolved_rect(
                        zoomed,
                        zoomed_rect,
                    )
                    .unwrap()]);
                let geometry = scroll_geometry(&zoomed_layout).unwrap();
                close(
                    Vec2::new(geometry.axes[0].unwrap().min, geometry.axes[0].unwrap().max),
                    Vec2::new(-1110.0, 1110.0),
                );
            }
        }
    }

    #[test]
    fn saved_and_free_rotation_use_the_painted_quad_including_negative_angles_and_trim() {
        let mut app = crate::app::setup_app_for_test();
        for rotation in [Rotation::None, Rotation::Cw90] {
            for angle in [0.0, std::f32::consts::FRAC_PI_4, -0.37] {
                let rect = Rect::from_center_size(viewport().center(), Vec2::new(1300.0, 1900.0));
                let mut input = page_input(0, rect, 1.0, Vec2::ZERO);
                input.rotation = rotation;
                input.free_rotation_rad = angle;
                input.content_bbox = Some(Rect::from_min_max(
                    egui::pos2(0.1, 0.2),
                    egui::pos2(0.9, 0.95),
                ));
                let page = DisplayedImageTransform::from_resolved_rect(input, rect).unwrap();
                let quad = page.paint_quad();
                let unit = quad.positions.iter().fold(Rect::NOTHING, |rect, point| {
                    rect.union(Rect::from_min_max(*point, *point))
                });
                let projected = page_rect_at_pan_zero(&page).unwrap();
                close(projected.min.to_vec2(), unit.min.to_vec2());
                close(projected.max.to_vec2(), unit.max.to_vec2());
                let layout = layout_from_pages(&[page]);
                let result = scroll(&mut app, &layout, 1.0, Vec2::ZERO, Vec2::splat(-10000.0));
                close(
                    result,
                    Vec2::new(
                        if unit.width() > viewport().width() {
                            viewport().right() - unit.right()
                        } else {
                            0.0
                        },
                        if unit.height() > viewport().height() {
                            viewport().bottom() - unit.bottom()
                        } else {
                            0.0
                        },
                    ),
                );
            }
        }
    }

    #[test]
    fn empty_continuous_and_z_layouts_do_not_scroll() {
        let mut app = crate::app::setup_app_for_test();
        let pan = Vec2::new(37.0, -29.0);
        let mut layouts = vec![FullscreenPageLayout::default(), single(Vec2::splat(3000.0))];
        layouts[1].begin(FullscreenPageLayoutKind::Continuous);
        let rect = Rect::from_center_size(viewport().center(), Vec2::splat(3000.0));
        let mut input = page_input(0, rect, 1.0, Vec2::ZERO);
        input.placement = ResolvedDisplayPlacement::Z {
            active: true,
            factor: 3.0,
            zoom_pan: Some((1.0, Vec2::ZERO)),
            singleton_side: SingletonSpreadPlacement::Center,
            singleton_gap: 0.0,
        };
        layouts.push(layout_from_pages(&[
            DisplayedImageTransform::from_resolved_rect(input, rect).unwrap(),
        ]));
        for layout in layouts {
            assert_eq!(
                scroll(&mut app, &layout, 1.0, pan, Vec2::splat(-500.0)),
                pan
            );
            assert_eq!(
                ordered(
                    &mut app,
                    &layout,
                    pan,
                    ReadingDirection::Ltr,
                    VerticalFirst,
                    true
                ),
                pan
            );
        }
    }
}
