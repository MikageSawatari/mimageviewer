//! Frame-local Native demand. These records own values, never textures or jobs.
//! Ordinary paint and speculative preparation share the same geometry functions.
use super::*;
use crate::displayed_image_transform::VisibleRegionRequest;
use crate::gpu_lanczos::FullscreenPaintSourceGeneration;
use crate::settings::AnimeUpscaleSourceLimit;

#[cfg(test)]
std::thread_local! {
    static RESIDENT_COLLECTION: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn assert_ordinary_metadata_allowed() {
    RESIDENT_COLLECTION.with(|depth| {
        assert_eq!(
            depth.get(),
            0,
            "canonical demand called an ordinary metadata producer"
        )
    });
}

#[cfg(test)]
struct ResidentCollectionGuard;

#[cfg(test)]
impl ResidentCollectionGuard {
    fn enter() -> Self {
        RESIDENT_COLLECTION.with(|depth| depth.set(depth.get() + 1));
        Self
    }
}

#[cfg(test)]
impl Drop for ResidentCollectionGuard {
    fn drop(&mut self) {
        RESIDENT_COLLECTION.with(|depth| depth.set(depth.get() - 1));
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CanonicalViewInputs {
    full_rect: egui::Rect,
    base_clip: egui::Rect,
    pixels_per_point: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CanonicalPageDemand {
    pub(crate) occurrence: SpreadPageOccurrence,
    pub(crate) source_id: egui::TextureId,
    pub(crate) source_size: [usize; 2],
    pub(crate) source_generation: FullscreenPaintSourceGeneration,
    pub(crate) transform: DisplayedImageTransform,
    pub(crate) clip_rect: egui::Rect,
    pub(crate) visible_region: Option<VisibleRegionRequest>,
    pub(crate) post_filter: PostFilter,
    pub(crate) smoothing_percent: u32,
    pub(crate) anime_source_limit: AnimeUpscaleSourceLimit,
    edit_key: crate::app::EditResultKey,
    params: crate::adjustment::AdjustParams,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CanonicalUnitDemand {
    pub(crate) destination: PresentationDestination,
    pub(crate) pages: Vec<CanonicalPageDemand>,
    view: CanonicalViewInputs,
    current_anchor: usize,
    items_generation: u64,
    composition: SpreadDisplayComposition,
    media_rect: egui::Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CanonicalDemandUnavailable {
    Ineligible,
    Topology,
    Boundary,
    Chrome,
    Rotation,
    AutoTrim,
    Material,
    FitBasis,
    Geometry,
}

impl CanonicalDemandUnavailable {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ineligible => "view_ineligible",
            Self::Topology => "topology_unavailable",
            Self::Boundary => "next_unit_boundary",
            Self::Chrome => "chrome_unavailable",
            Self::Rotation => "rotation_unavailable",
            Self::AutoTrim => "auto_trim_unavailable",
            Self::Material => "material_unavailable",
            Self::FitBasis => "fit_basis_unavailable",
            Self::Geometry => "geometry_unavailable",
        }
    }
}

/// The clip test is shared with ordinary Single painting, including its
/// deliberate unclipped Page-fit case. ROI is derived only after this decision.
pub(super) fn single_clip_rect(
    base: egui::Rect,
    viewport: egui::Rect,
    zoom_pan: Option<(f32, egui::Vec2)>,
    free_rotation: f32,
    content: Option<egui::Rect>,
    mode: FullscreenFitMode,
    limits: FullscreenFitScaleLimits,
) -> egui::Rect {
    let fit_bbox = crate::displayed_image_transform::effective_bbox(free_rotation, content);
    if zoom_pan.is_some()
        || free_rotation.abs() > TRANSFORM_EPSILON
        || fit_bbox.is_some()
        || !matches!(mode, FullscreenFitMode::Page)
        || limits.active()
    {
        base.intersect(viewport)
    } else {
        base
    }
}

pub(super) fn spread_clip_rect(
    base: egui::Rect,
    viewport: egui::Rect,
    zoom_pan: Option<(f32, egui::Vec2)>,
    content: [Option<egui::Rect>; 2],
    mode: FullscreenFitMode,
    limits: FullscreenFitScaleLimits,
    zoom_engaged: bool,
) -> egui::Rect {
    if zoom_pan.is_some()
        || content.iter().any(Option::is_some)
        || !matches!(mode, FullscreenFitMode::Page)
        || limits.active()
        || zoom_engaged
    {
        base.intersect(viewport)
    } else {
        base
    }
}

/// Common fit/bbox stage, before optional interactive zoom. Each page retains
/// its own size; physical snapping and the final inner-edge alignment follow.
pub(super) fn spread_auto_layout(
    sizes: [egui::Vec2; 2],
    content: [Option<egui::Rect>; 2],
    mode: FullscreenFitMode,
    specified: crate::settings::FullscreenSpecifiedScale,
    specified_max: u32,
    viewport: egui::Rect,
    gap: f32,
    limits: FullscreenFitScaleLimits,
) -> Option<(
    SpreadLayoutGeometry,
    crate::fullscreen_specified_scale::ResolvedSpecifiedScale,
)> {
    let full = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
    let geometry = spread_layout_geometry(
        sizes[0],
        sizes[1],
        content[0].unwrap_or(full),
        content[1].unwrap_or(full),
        spread_should_match_page_heights(mode),
    );
    let auto_fit = resolve_paged_auto_fit(
        mode,
        specified,
        viewport,
        geometry.fit_size,
        gap,
        limits,
        specified_max,
    )?;
    Some((geometry, auto_fit))
}

/// This is also the ordinary Pair page transform, after resource/fit provenance
/// has been resolved by its existing owner. No cache reads or generation occur.
pub(super) fn spread_page_transform(
    input: DisplayedImageTransformInput,
    layout_size: egui::Vec2,
    rect: egui::Rect,
) -> Option<DisplayedImageTransform> {
    let display_size = rotated_display_size(layout_size, input.rotation);
    let page_rect = fit_display_size_in_rect(rect, display_size);
    resolve_fs_transform_in_layout_rect(input, page_rect)
}

impl App {
    pub(crate) fn record_next_canonical_native_demand(&mut self, ui: &egui::Ui) {
        if !self.settings.page_turn_animation_enabled
            || !self.reading_flow.is_paged()
            || self.spread_mode.is_split()
        {
            return;
        }
        let view = CanonicalViewInputs {
            full_rect: ui.max_rect(),
            base_clip: ui.painter().clip_rect(),
            pixels_per_point: ui.ctx().pixels_per_point(),
        };
        match self.collect_next_canonical_demand(ui.ctx(), view) {
            Ok(demand) => self.observe_canonical_native_demand(demand),
            Err(reason) => self.observe_canonical_native_unavailable(reason.as_str()),
        }
    }

    /// Replay uses the owning callback's view, never ROOT's ambient ppp. A resize
    /// or discard callback replaces the record before the final output hook.
    pub(crate) fn canonical_unit_demand_is_current(
        &self,
        ctx: &egui::Context,
        demand: &CanonicalUnitDemand,
    ) -> bool {
        if !self.canonical_native_demand_was_observed(demand) {
            return false;
        }
        self.collect_next_canonical_demand(ctx, demand.view)
            .is_ok_and(|current| current == *demand)
    }

    pub(crate) fn canonical_demand_resource_if_current(
        &self,
        ctx: &egui::Context,
        unit: &CanonicalUnitDemand,
        page: &CanonicalPageDemand,
    ) -> Option<FullscreenPaintResource> {
        if unit.destination != self.page_turn_destination()
            || unit.items_generation != self.items_generation
            || unit.current_anchor != self.fullscreen_idx?
            || !unit.pages.contains(page)
        {
            return None;
        }
        let resource = self.resident_canonical_resource(ctx, page.occurrence.idx)?;
        (resource.source_texture_id() == page.source_id
            && resource.source_texture().size() == page.source_size
            && resource.source_generation() == Some(page.source_generation)
            && self.current_edit_result_key(page.occurrence.idx) == page.edit_key
            && *self.effective_params(page.occurrence.idx) == page.params)
            .then_some(resource)
    }

    fn resident_canonical_resource(
        &self,
        ctx: &egui::Context,
        idx: usize,
    ) -> Option<FullscreenPaintResource> {
        let texture = self.resident_final_prepare_texture(ctx, idx)?;
        if self.resolve_fs_display_tex(idx, false)?.id() != texture.id() {
            return None;
        }
        Some(self.fullscreen_paint_resource_for_texture(idx, texture))
    }

    fn resident_canonical_trim(
        &self,
        idx: usize,
        side: Option<crate::view_trim::ViewTrimSpreadSide>,
    ) -> Result<Option<egui::Rect>, CanonicalDemandUnavailable> {
        use crate::view_trim::ViewTrimApplyMode;
        match self.effective_view_trim_apply_mode_for_idx(idx) {
            ViewTrimApplyMode::None => Ok(None),
            ViewTrimApplyMode::Book | ViewTrimApplyMode::Page => Ok(match side {
                Some(side) => self.view_trim_spread_bbox(idx, side),
                None => self.view_trim_single_bbox(idx),
            }),
            ViewTrimApplyMode::Auto => self.resident_canonical_auto_trim(idx),
        }
    }

    fn resident_canonical_auto_trim(
        &self,
        idx: usize,
    ) -> Result<Option<egui::Rect>, CanonicalDemandUnavailable> {
        #[cfg(windows)]
        if self.viewer_session_is_detached()
            && let Some(window) = self.detached_viewer_window_id()
        {
            // A baked None is a resident answer. Missing is not None.
            return self
                .detached_window_runtime_trim_bbox(window, idx)
                .ok_or(CanonicalDemandUnavailable::AutoTrim);
        }
        let (seq, ptr) = match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static {
                load_seq, pixels, ..
            }) => (*load_seq, Arc::as_ptr(pixels) as usize),
            Some(FsCacheEntry::RawPreview {
                load_seq,
                preview: Some(preview),
                ..
            }) => (*load_seq, Arc::as_ptr(&preview.pixels) as usize),
            _ => return Err(CanonicalDemandUnavailable::AutoTrim),
        };
        self.fs_margin_bbox_cache
            .get(&idx)
            .filter(|(old_seq, old_ptr, _)| *old_seq == seq && *old_ptr == ptr)
            .map(|(_, _, bbox)| *bbox)
            .ok_or(CanonicalDemandUnavailable::AutoTrim)
    }

    fn collect_next_canonical_demand(
        &self,
        ctx: &egui::Context,
        view: CanonicalViewInputs,
    ) -> Result<CanonicalUnitDemand, CanonicalDemandUnavailable> {
        #[cfg(test)]
        let _resident_only = ResidentCollectionGuard::enter();
        use CanonicalDemandUnavailable as Missing;
        let anchor = self.fullscreen_idx.ok_or(Missing::Ineligible)?;
        if self.remote_session_blocks_local_control()
            || !self.page_turn_current_unit_presented()
            || !self.page_turn_view_eligible(anchor)
            || self.spread_mode.is_split()
        {
            return Err(Missing::Ineligible);
        }
        #[cfg(windows)]
        if self.slideshow_playing
            && matches!(
                self.navigation_scope,
                crate::app::ViewerNavigationScope::CollectionRoot
            )
        {
            return Err(Missing::Ineligible);
        }
        let composition = self
            .resident_final_unit_composition_from_snapshot(anchor, FinalUnitPosition::Next)
            .ok_or(Missing::Topology)?
            .ok_or(Missing::Boundary)?;
        let occurrences = composition.pages_in_screen_order(self.spread_mode);
        if occurrences.is_empty()
            || occurrences
                .iter()
                .any(|page| !self.page_turn_view_eligible(page.idx))
        {
            return Err(Missing::Ineligible);
        }
        let next = composition.navigation_anchor_idx;
        let seek_allowed = self.fullscreen_seek_overlay_allowed(next, false);
        // Match ordinary seek geometry's short circuit: hidden/ineligible strips
        // have known geometry without looking up their source-wide content.
        // Never populate navigation caches or synchronously resolve cold metadata.
        let strip_has_content = if self.settings.still_seek_strip_visible && seek_allowed {
            self.viewer_navigation_caches
                .fs_seek_info
                .as_ref()
                .filter(|(cached_anchor, _)| *cached_anchor == anchor)
                .map(|(_, info)| info.has_page_strip_content())
                .ok_or(Missing::Chrome)?
        } else {
            false
        };
        let chrome = FsNavigationStillChromeInputs {
            top_bar_eligible: self.fullscreen_top_bar_chrome_allowed(next),
            seek_eligible: seek_allowed,
            strip_has_content: seek_allowed && strip_has_content,
            info_panel_eligible: self.still_info_panel_eligible_for_idx(next, false),
        };
        let seek = self.still_seek_geometry_for_navigation_chrome(view.full_rect, chrome);
        let right = self.locked_info_panel_reserved_width_for_eligibility(
            view.full_rect,
            seek.total_height,
            chrome.info_panel_eligible,
        );
        let media = seek.media_rect(view.full_rect, right);
        let mode = self.effective_fullscreen_fit_mode();
        let limits = self.fullscreen_fit_scale_limits(view.pixels_per_point);
        let mut resources = Vec::with_capacity(occurrences.len());
        let mut inputs = Vec::with_capacity(occurrences.len());
        let mut layouts = Vec::with_capacity(occurrences.len());
        for occurrence in &occurrences {
            let idx = occurrence.idx;
            let rotation = self
                .rotation_cache
                .get(&idx)
                .copied()
                .or_else(|| self.prepared_composed_rotation(idx))
                .ok_or(Missing::Rotation)?;
            let resource = self
                .resident_canonical_resource(ctx, idx)
                .ok_or(Missing::Material)?;
            // A thumbnail/direct surface is not Native input. No alternate
            // selection is invented when the ordinary source has not arrived.
            resource.source_generation().ok_or(Missing::Material)?;
            let fit = self
                .fs_fit_input_for_resource(idx, mode, resource.source_texture())
                .ok_or(Missing::FitBasis)?;
            resources.push(resource);
            inputs.push((rotation, fit));
            layouts.push(self.fs_page_layout_source_size(idx));
        }
        let legacy_auto = matches!(
            self.settings.fullscreen_fit_mode,
            FullscreenFitMode::MarginFit
        ) || self.settings.margin_fit_enabled;
        let base_trim_mode = crate::view_trim::effective_view_trim_base_apply_mode(
            self.view_trim_apply_mode,
            legacy_auto,
        );
        let linked_pair_trim =
            occurrences.len() == 2 && inputs.iter().all(|(rotation, _)| rotation.is_none());
        let mut content = Vec::with_capacity(occurrences.len());
        for (position, occurrence) in occurrences.iter().enumerate() {
            let side = if occurrences.len() == 2 {
                Some(if position == 0 {
                    crate::view_trim::ViewTrimSpreadSide::Left
                } else {
                    crate::view_trim::ViewTrimSpreadSide::Right
                })
            } else {
                // Explicit BlankOpposite uses spread margins on its real side.
                self.composed_blank_spread_side(occurrence.idx)
            };
            content.push(if linked_pair_trim {
                match base_trim_mode {
                    crate::view_trim::ViewTrimApplyMode::None => None,
                    crate::view_trim::ViewTrimApplyMode::Auto => {
                        self.resident_canonical_auto_trim(occurrence.idx)?
                    }
                    _ => self.view_trim_spread_bbox(occurrence.idx, side.ok_or(Missing::Geometry)?),
                }
            } else if occurrences.len() == 2 && !inputs[position].0.is_none() {
                None
            } else {
                self.resident_canonical_trim(occurrence.idx, side)?
            });
        }
        if linked_pair_trim {
            // Preserve the ordinary Pair's *base* apply-mode policy, including
            // its existing handling of page rows and auto vertical harmonization.
            match base_trim_mode {
                crate::view_trim::ViewTrimApplyMode::None => content = vec![None, None],
                crate::view_trim::ViewTrimApplyMode::Auto => {
                    let pair =
                        crate::view_trim::harmonize_spread_auto_bboxes(content[0], content[1]);
                    content = vec![pair.0, pair.1];
                }
                _ => {}
            }
        }
        let gap = quantize_points_to_physical_pixels(
            self.settings.spread_page_gap_px.min(200) as f32,
            view.pixels_per_point,
        );
        let (transforms, clip) = if occurrences.len() == 1 {
            let resource = &resources[0];
            let placement = if composition.singleton_placement == SingletonSpreadPlacement::Center {
                ResolvedDisplayPlacement::Normal { zoom_pan: None }
            } else {
                ResolvedDisplayPlacement::SingletonSpread {
                    side: composition.singleton_placement,
                    gap,
                    zoom_pan: None,
                }
            };
            let input = DisplayedImageTransformInput {
                pixel_fit: RectPixelFit::Texels,
                page_idx: next,
                viewport_rect: media,
                source_size: self
                    .fs_page_coordinate_source_size(next)
                    .unwrap_or(resource.size_vec2()),
                texture_size: resource.size_vec2(),
                rotation: inputs[0].0,
                free_rotation_rad: 0.0,
                content_bbox: content[0],
                fit_mode: inputs[0].1,
                fit_scale_limits: limits,
                pixels_per_point: view.pixels_per_point,
                placement,
            };
            let layout = self
                .fs_page_layout_uses_source_id(next, resource.source_texture_id())
                .then_some(layouts[0])
                .flatten();
            let transform = resolve_fs_image_transform(input, layout).ok_or(Missing::Geometry)?;
            (
                vec![transform],
                single_clip_rect(view.base_clip, media, None, 0.0, content[0], mode, limits),
            )
        } else if occurrences.len() == 2 {
            let mut sizes = [egui::Vec2::ZERO; 2];
            let pdf_layout_unit = occurrences.iter().any(|page| {
                self.is_raw_page(page.idx)
                    || matches!(self.items.get(page.idx), Some(GridItem::PdfPage { .. }))
            });
            for i in 0..2 {
                sizes[i] = if mode == FullscreenFitMode::SpecifiedScale {
                    rotated_display_size(
                        fit_input_layout_size(inputs[i].1, egui::Vec2::ZERO),
                        inputs[i].0,
                    )
                } else if pdf_layout_unit {
                    passthrough_layout_display_size(
                        layouts[i],
                        Some(resources[i].size_vec2()),
                        inputs[i].0,
                    )
                    .ok_or(Missing::Geometry)?
                } else {
                    rotated_display_size(resources[i].size_vec2(), inputs[i].0)
                };
            }
            let (geometry, fit) = spread_auto_layout(
                sizes,
                [content[0], content[1]],
                mode,
                self.settings.fullscreen_specified_scale,
                self.settings.fullscreen_specified_scale_max_tenths,
                media,
                gap,
                limits,
            )
            .ok_or(Missing::Geometry)?;
            let full = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
            let rects = layout_spread_page_rects(
                media.center() - geometry.content_center_offset * fit.scale,
                geometry,
                fit.scale,
                fit.gap,
                view.pixels_per_point,
                content[0].unwrap_or(full),
                content[1].unwrap_or(full),
                content.iter().any(Option::is_some),
            );
            let mut transforms = Vec::with_capacity(2);
            for (i, rect) in [rects.left_rect, rects.right_rect].into_iter().enumerate() {
                let idx = occurrences[i].idx;
                let resource = &resources[i];
                let use_layout = self
                    .fs_page_layout_uses_source_id(idx, resource.source_texture_id())
                    || matches!(self.items.get(idx), Some(GridItem::PdfPage { .. }));
                let layout = use_layout
                    .then_some(layouts[i])
                    .flatten()
                    .unwrap_or(resource.size_vec2());
                transforms.push(
                    spread_page_transform(
                        DisplayedImageTransformInput {
                            pixel_fit: RectPixelFit::Texels,
                            page_idx: idx,
                            viewport_rect: media,
                            source_size: self
                                .fs_page_coordinate_source_size(idx)
                                .unwrap_or(resource.size_vec2()),
                            texture_size: resource.size_vec2(),
                            rotation: inputs[i].0,
                            free_rotation_rad: 0.0,
                            content_bbox: content[i],
                            fit_mode: inputs[i].1,
                            fit_scale_limits: FullscreenFitScaleLimits {
                                pixels_per_point: view.pixels_per_point,
                                ..Default::default()
                            },
                            pixels_per_point: view.pixels_per_point,
                            placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
                        },
                        layout,
                        rect,
                    )
                    .ok_or(Missing::Geometry)?,
                );
            }
            let offsets = align_spread_pages_for_gap(
                &transforms[0],
                &transforms[1],
                (fit.gap.max(0.0) * view.pixels_per_point).round(),
                view.pixels_per_point,
            );
            transforms[0] = transforms[0].translated_by(offsets.0);
            transforms[1] = transforms[1].translated_by(offsets.1);
            (
                transforms,
                spread_clip_rect(
                    view.base_clip,
                    media,
                    None,
                    [content[0], content[1]],
                    mode,
                    limits,
                    false,
                ),
            )
        } else {
            return Err(Missing::Geometry);
        };
        let pages = occurrences
            .iter()
            .zip(resources.iter())
            .zip(transforms)
            .map(|((occurrence, resource), transform)| {
                Ok(CanonicalPageDemand {
                    occurrence: *occurrence,
                    source_id: resource.source_texture_id(),
                    source_size: resource.source_texture().size(),
                    source_generation: resource.source_generation().ok_or(Missing::Material)?,
                    transform,
                    clip_rect: clip,
                    visible_region: transform.visible_region_request(clip),
                    post_filter: self.fullscreen_paint_post_filter(resource),
                    smoothing_percent: self.settings.downscale_smoothing_percent,
                    anime_source_limit: self.settings.anime_upscale_source_limit,
                    edit_key: self.current_edit_result_key(occurrence.idx),
                    params: self.effective_params(occurrence.idx).clone(),
                })
            })
            .collect::<Result<Vec<_>, Missing>>()?;
        Ok(CanonicalUnitDemand {
            destination: self.page_turn_destination(),
            pages,
            view,
            current_anchor: anchor,
            items_generation: self.items_generation,
            composition,
            media_rect: media,
        })
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod topology_tests;

#[cfg(test)]
mod draw_key_tests;
