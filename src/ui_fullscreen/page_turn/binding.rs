//! Value identity of one resolved presentation, independent of book allocation/ordinal.
use super::*;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PageTurnLayoutBinding {
    pub(super) anchor: usize,
    pub(super) pages: Vec<usize>,
    pub(super) formation: SpreadDisplayUnitFormation,
    pub(super) composition: SpreadDisplayComposition,
    pub(super) automatic_front_cover: bool,
}

impl PageTurnLayoutBinding {
    /// Side/center placement is rebindable in Completing; blank slots are topology.
    pub(super) fn same_meaning(&self, other: &Self) -> bool {
        self.anchor == other.anchor
            && self.pages == other.pages
            && self.formation == other.formation
            && self.automatic_front_cover == other.automatic_front_cover
            && self.composition.navigation_anchor_idx == other.composition.navigation_anchor_idx
            && self.composition.pages == other.composition.pages
            && self.composition.supplemented_navigation_side
                == other.composition.supplemented_navigation_side
            && self.composition.composed_direction == other.composition.composed_direction
            && self.composition.singleton_placement.has_white_companion()
                == other.composition.singleton_placement.has_white_companion()
    }
}

#[derive(Clone, PartialEq)]
pub(in crate::ui_fullscreen) struct PageTurnUnitBinding {
    pub(super) destination: PresentationDestination,
    pub(super) items: u64,
    pub(super) mode: SpreadMode,
    pub(super) reading_flow: ReadingFlow,
    pub(super) reading_rtl: bool,
    pub(super) slice: crate::page_split::PageSlice,
    pub(super) layout: PageTurnLayoutBinding,
    pub(super) content: Vec<PageTurnContent>,
    pub(super) prepare_policy: PageTurnPreparePolicy,
    page_views: Vec<PageTurnPageViewPolicy>,
    pub(super) output: PageTurnBindingOutput,
}

#[derive(Clone, PartialEq)]
pub(super) struct PageTurnPreparePolicy {
    pub(super) post_filter_bypassed: bool,
    pub(super) smoothing: u32,
    pub(super) upscale_source_limit: crate::settings::AnimeUpscaleSourceLimit,
    underlay_mode: u8,
    fit_mode: FullscreenFitMode,
    no_upscale: bool,
    no_downscale: bool,
    specified: Option<(crate::settings::FullscreenSpecifiedScale, u32)>,
}

#[derive(Clone, PartialEq)]
struct PageTurnPageViewPolicy {
    trim_mode: crate::view_trim::ViewTrimApplyMode,
    stored_bbox: [Option<egui::Rect>; 3],
}

impl PageTurnPageViewPolicy {
    fn current(app: &App, idx: usize) -> Self {
        Self {
            trim_mode: app.effective_view_trim_apply_mode_for_idx(idx),
            stored_bbox: [
                app.view_trim_single_bbox(idx),
                app.view_trim_spread_bbox(idx, crate::view_trim::ViewTrimSpreadSide::Left),
                app.view_trim_spread_bbox(idx, crate::view_trim::ViewTrimSpreadSide::Right),
            ],
        }
    }
}

#[derive(Clone, PartialEq)]
pub(super) enum PageTurnBindingOutput {
    Accepted,
    Pending {
        viewport: egui::Rect,
        pixels_per_point: f32,
    },
    Prepared {
        viewport: egui::Rect,
        pixels_per_point: f32,
        pages: Vec<PageTurnPreparedBinding>,
        white: Option<(egui::Rect, egui::Rect)>,
    },
}

#[derive(Clone, PartialEq)]
pub(super) struct PageTurnPreparedBinding {
    pub(super) occurrence: SpreadPageOccurrence,
    pub(super) transform: DisplayedImageTransform,
    pub(super) crop: Option<crate::export_crop::CropSettings>,
    pub(super) clip: egui::Rect,
    pub(super) underlay: PageTurnUnderlayBinding,
    pub(super) spine: Option<(f32, f32)>,
    pub(super) source: egui::TextureId,
    pub(super) paint: egui::TextureId,
    pub(super) generation: Option<crate::gpu_lanczos::FullscreenPaintSourceGeneration>,
    pub(super) coverage: crate::source_coverage::SourceCoverage,
    pub(super) roi: Option<egui::Rect>,
}

#[derive(Clone, PartialEq)]
pub(super) enum PageTurnUnderlayBinding {
    Solid(egui::Color32),
    Checker(egui::TextureId),
}

impl PageTurnPreparePolicy {
    pub(super) fn current(app: &App) -> Self {
        Self {
            post_filter_bypassed: app.post_filter_bypassed,
            smoothing: app.settings.downscale_smoothing_percent,
            upscale_source_limit: app.settings.anime_upscale_source_limit,
            underlay_mode: app.fs_transparent_bg_mode,
            fit_mode: app.effective_fullscreen_fit_mode(),
            no_upscale: app.settings.fullscreen_fit_no_upscale,
            no_downscale: app.settings.fullscreen_fit_no_downscale,
            specified: (app.effective_fullscreen_fit_mode() == FullscreenFitMode::SpecifiedScale)
                .then_some((
                    app.settings.fullscreen_specified_scale,
                    app.settings.fullscreen_specified_scale_max_tenths,
                )),
        }
    }
}

impl PageTurnUnitBinding {
    pub(super) fn same_unit_content(&self, other: &Self) -> bool {
        self.same_context(other)
            && self.layout.same_meaning(&other.layout)
            && self.content == other.content
    }

    pub(super) fn same_prepared_output(&self, other: &Self) -> bool {
        self.same_context(other)
            && self.layout == other.layout
            && self.same_view_policy(other)
            && self.output == other.output
    }

    pub(super) fn same_view_policy(&self, other: &Self) -> bool {
        self.layout.composition.singleton_placement == other.layout.composition.singleton_placement
            && self.prepare_policy == other.prepare_policy
            && self.page_views == other.page_views
    }

    pub(super) fn same_context(&self, other: &Self) -> bool {
        self.destination == other.destination
            && self.items == other.items
            && self.mode == other.mode
            && self.reading_flow == other.reading_flow
            && self.reading_rtl == other.reading_rtl
            && self.slice == other.slice
    }

    pub(super) fn matches_layout(&self, app: &App) -> bool {
        self.destination == app.page_turn_destination()
            && self.items == app.items_generation
            && self.mode == app.spread_mode
            && self.reading_flow == app.reading_flow
            && self.reading_rtl == app.page_turn_reading_rtl()
            && self.slice == app.fullscreen_page_slice
            && app
                .page_turn_resolved_layout(self.layout.anchor)
                .is_some_and(|layout| self.layout.same_meaning(&layout))
    }

    /// A resolved command preserves its meaning and item identities, not capture-time edits/view.
    pub(super) fn matches_navigation(&self, app: &App) -> bool {
        self.matches_layout(app)
            && self.content.len() == self.layout.composition.pages.len()
            && self
                .content
                .iter()
                .zip(self.layout.composition.pages_in_screen_order(self.mode))
                .all(|(content, page)| {
                    content.edit.item_id == app.current_edit_result_key(page.idx).item_id
                })
    }

    pub(in crate::ui_fullscreen) fn matches_current(&self, app: &App) -> bool {
        self.matches_layout(app)
            && self.content.len()
                == self
                    .layout
                    .composition
                    .pages_in_screen_order(self.mode)
                    .len()
            && self
                .content
                .iter()
                .zip(self.layout.composition.pages_in_screen_order(self.mode))
                .all(|(content, page)| content.matches_raw(app, page.idx))
    }

    pub(super) fn matches_prepared_occurrences(&self, pages: &[PreparedPagedPage]) -> bool {
        self.layout
            .composition
            .pages_in_screen_order(self.mode)
            .iter()
            .copied()
            .eq(pages.iter().map(|page| page.occurrence))
    }

    pub(super) fn with_prepared(
        mut self,
        pages: &[PreparedPagedPage],
        white: Option<(egui::Rect, egui::Rect)>,
    ) -> Self {
        self.output = PageTurnBindingOutput::Prepared {
            viewport: pages[0].painter.ctx().content_rect(),
            pixels_per_point: pages[0].painter.ctx().pixels_per_point(),
            pages: pages
                .iter()
                .map(|page| PageTurnPreparedBinding {
                    occurrence: page.occurrence,
                    transform: page.transform,
                    crop: page.crop,
                    clip: page.painter.clip_rect(),
                    underlay: match &page.underlay {
                        FsBgStyle::Solid(color) => PageTurnUnderlayBinding::Solid(*color),
                        FsBgStyle::Checker(texture) => {
                            PageTurnUnderlayBinding::Checker(texture.id())
                        }
                    },
                    spine: page.spine,
                    source: page.resource.source_texture_id(),
                    paint: page.resource.paint_texture_id(),
                    generation: page.resource.source_generation(),
                    coverage: page.resource.source_coverage(),
                    roi: page.resource.visible_source_uv_rect(),
                })
                .collect(),
            white,
        };
        self
    }

    pub(in crate::ui_fullscreen) fn for_pending(mut self, ctx: &egui::Context) -> Self {
        self.output = PageTurnBindingOutput::Pending {
            viewport: ctx.content_rect(),
            pixels_per_point: ctx.pixels_per_point(),
        };
        self
    }

    pub(super) fn reanchored(
        &self,
        mut current: Self,
        occurrences: &[SpreadPageOccurrence],
    ) -> Option<Self> {
        if !self.same_context(&current)
            || self.content != current.content
            || self.prepare_policy != current.prepare_policy
            || self.page_views != current.page_views
            || current
                .layout
                .composition
                .pages_in_screen_order(current.mode)
                != occurrences
        {
            return None;
        }
        let PageTurnBindingOutput::Prepared { pages, .. } = &self.output else {
            return None;
        };
        if pages.len() != occurrences.len()
            || !pages.iter().zip(occurrences).all(|(page, next)| {
                page.occurrence.idx == next.idx && page.occurrence.role == next.role
            })
        {
            return None;
        }
        current.output = self.output.clone();
        if let PageTurnBindingOutput::Prepared { pages, .. } = &mut current.output {
            for (page, occurrence) in pages.iter_mut().zip(occurrences) {
                page.occurrence = *occurrence;
            }
        }
        Some(current)
    }

    pub(in crate::ui_fullscreen) fn matches_viewport_geometry(&self, ctx: &egui::Context) -> bool {
        matches!(&self.output, PageTurnBindingOutput::Prepared { viewport, pixels_per_point, .. } | PageTurnBindingOutput::Pending { viewport, pixels_per_point }
            if *viewport == ctx.content_rect() && *pixels_per_point == ctx.pixels_per_point())
    }

    pub(in crate::ui_fullscreen) fn matches_prepare_policy(&self, app: &App) -> bool {
        app.page_turn_resolved_layout(self.layout.anchor)
            .is_some_and(|layout| {
                self.layout.composition.singleton_placement
                    == layout.composition.singleton_placement
            })
            && self.prepare_policy == PageTurnPreparePolicy::current(app)
            && self.page_views.iter().eq(self
                .layout
                .composition
                .pages_in_screen_order(self.mode)
                .iter()
                .map(|page| PageTurnPageViewPolicy::current(app, page.idx))
                .collect::<Vec<_>>()
                .iter())
    }

    pub(super) fn prepared_matches_view(&self, app: &App) -> bool {
        self.matches_prepare_policy(app)
            && match &self.output {
                PageTurnBindingOutput::Accepted | PageTurnBindingOutput::Pending { .. } => false,
                PageTurnBindingOutput::Prepared { pages, .. } => {
                    let current = app
                        .fullscreen_page_layout
                        .displayed_pages()
                        .collect::<Vec<_>>();
                    current.len() == pages.len()
                        && pages.iter().zip(current).all(|(saved, current)| {
                            saved.occurrence == current.occurrence()
                                && same_page_turn_view(&saved.transform, &current.transform)
                                && saved.transform.paint_quad() == current.transform.paint_quad()
                                && saved.transform.uv_rect == current.transform.uv_rect
                        })
                }
            }
    }

    /// Qualification of A must not read the last drawn layout of B. Resolve A's
    /// policy at its own anchor and keep its accepted raster/fit basis.
    pub(super) fn matches_unit_view(&self, app: &App) -> bool {
        self.matches_prepare_policy(app)
            && match &self.output {
                PageTurnBindingOutput::Prepared { pages, .. } => {
                    pages.iter().all(|page| match page.transform.placement {
                        ResolvedDisplayPlacement::Normal { zoom_pan } => {
                            zoom_pan == app.fs_zoom_pan()
                        }
                        ResolvedDisplayPlacement::SingletonSpread { zoom_pan, .. } => {
                            zoom_pan == app.fs_zoom_pan()
                        }
                        ResolvedDisplayPlacement::Z { .. } => false,
                    })
                }
                _ => false,
            }
    }
}

pub(super) fn same_page_turn_view(
    a: &DisplayedImageTransform,
    b: &DisplayedImageTransform,
) -> bool {
    a.viewport_rect == b.viewport_rect
        && a.placement == b.placement
        && a.pixels_per_point == b.pixels_per_point
        && a.rotation == b.rotation
        && a.free_rotation_rad == b.free_rotation_rad
        && a.content_bbox == b.content_bbox
        && a.fit_mode == b.fit_mode
        && a.fit_scale_limits == b.fit_scale_limits
        && same_fit_policy(a.fit_input, b.fit_input)
}

impl App {
    pub(super) fn page_turn_reading_rtl(&self) -> bool {
        self.spread_mode
            .canonical_reading_direction()
            .unwrap_or(self.reading_direction)
            == ReadingDirection::Rtl
    }

    /// Resolve once at the existing adoption/lookup prefix, never from a read-only matcher.
    pub(in crate::ui_fullscreen) fn resolve_page_turn_layout_before_lookup(&mut self) {
        if let Some(anchor) = self.fullscreen_idx
            && (self.settings.page_turn_animation_enabled
                || self.page_turn_completion_active()
                || self.pending_fs_page_turn_ready.is_some())
        {
            self.spread_display_composition_for_anchor(anchor);
        }
    }

    /// The normal resolver has already installed the canonical topology and completeness proof.
    /// Only the inverse unit index and first/last endpoints are read here; no book scan or producer.
    pub(super) fn page_turn_resolved_layout(&self, anchor: usize) -> Option<PageTurnLayoutBinding> {
        let cache = &self.spread_display_units_cache;
        if !self.spread_mode.is_spread() {
            return Some(PageTurnLayoutBinding {
                anchor,
                pages: vec![anchor],
                formation: SpreadDisplayUnitFormation::Singleton(
                    if is_spread_pairable_item(self.items.get(anchor)) {
                        SpreadDisplaySingletonCause::UnpairedSlot
                    } else {
                        SpreadDisplaySingletonCause::NonPairableBoundary
                    },
                ),
                composition: SpreadDisplayComposition::from_navigation_pages(anchor, &[anchor])?,
                automatic_front_cover: false,
            });
        }
        cache.token.as_ref()?;
        if let Some(position) = cache.unit_position(anchor) {
            let unit = cache.units.get(position)?;
            let final_cover = self.final_cover_spread_enabled_for_current_book();
            let placement = self.spread_singleton_display_endpoint_settings();
            let alone = self.page_alone_enabled_for_current_book();
            let needs_complete = (self.spread_mode.has_cover() && final_cover)
                || placement.first
                || placement.last
                || (self.spread_mode.has_cover() && (alone.after_cover || alone.last));
            let eligible = needs_complete
                && self.spread_complete_book_context_eligible()
                && cache.complete_page_permutation?;
            let composition = self.spread_display_composition_for_cached_unit(
                &cache.units,
                position,
                anchor,
                final_cover,
                placement,
                eligible,
            )?;
            Some(PageTurnLayoutBinding {
                anchor,
                pages: unit.pages.clone(),
                formation: unit.formation,
                composition,
                automatic_front_cover: self.spread_mode.has_cover() && position == 0,
            })
        } else {
            // This is the normal facet-outside singleton, not a cache-miss equality fallback.
            let unit = self.facet_outside_unit(anchor);
            let formation = unit.formation;
            let composition = resolve_spread_display_composition(
                std::slice::from_ref(&unit),
                0,
                self.spread_mode,
                false,
                self.spread_singleton_display_endpoint_settings(),
                false,
            )?;
            Some(PageTurnLayoutBinding {
                anchor,
                pages: vec![anchor],
                formation,
                composition,
                automatic_front_cover: false,
            })
        }
    }

    pub(in crate::ui_fullscreen) fn page_turn_unit_binding(
        &self,
        anchor: usize,
    ) -> Option<PageTurnUnitBinding> {
        let layout = self.page_turn_resolved_layout(anchor)?;
        let occurrences = layout.composition.pages_in_screen_order(self.spread_mode);
        let content = occurrences
            .iter()
            .map(|page| self.page_turn_content(page.idx))
            .collect();
        let page_views = occurrences
            .iter()
            .map(|page| PageTurnPageViewPolicy::current(self, page.idx))
            .collect();
        Some(PageTurnUnitBinding {
            destination: self.page_turn_destination(),
            items: self.items_generation,
            mode: self.spread_mode,
            reading_flow: self.reading_flow,
            reading_rtl: self.page_turn_reading_rtl(),
            slice: self.fullscreen_page_slice,
            layout,
            content,
            prepare_policy: PageTurnPreparePolicy::current(self),
            page_views,
            output: PageTurnBindingOutput::Accepted,
        })
    }
}

impl PageTurnContent {
    pub(super) fn matches_raw(&self, app: &App, idx: usize) -> bool {
        let mut edit = app.current_edit_result_key(idx);
        edit.source_gen = 0;
        let rotation = app
            .rotation_cache
            .get(&idx)
            .copied()
            .or_else(|| app.prepared_composed_rotation(idx))
            .unwrap_or(crate::rotation_db::Rotation::None);
        let crop_matches = match (self.crop, app.export_crop_page_settings.get(&idx).copied()) {
            (None, None) => true,
            (Some(saved), Some(raw)) => {
                saved.rect == raw.rect
                    && saved.aspect_mode == raw.aspect_mode
                    && (raw.valid_source_size().is_none() || saved == raw)
            }
            _ => false,
        };
        crop_matches
            && self.edit == edit
            && self.params == *app.effective_params(idx)
            && self.comic_generation == app.comic_generation
            && self.gap == app.settings.spread_page_gap_px
            && self.rotation == rotation
            && self.free_rotation_rad == app.fs_free_rotation
    }
}

#[cfg(test)]
impl PageTurnUnitBinding {
    pub(in crate::ui_fullscreen) fn for_test(destination: PresentationDestination) -> Self {
        Self {
            destination,
            items: 1,
            mode: SpreadMode::Single,
            reading_flow: ReadingFlow::Paged,
            reading_rtl: false,
            slice: crate::page_split::PageSlice::Full,
            layout: PageTurnLayoutBinding {
                anchor: 0,
                pages: vec![0],
                formation: SpreadDisplayUnitFormation::Singleton(
                    SpreadDisplaySingletonCause::UnpairedSlot,
                ),
                composition: SpreadDisplayComposition::from_navigation_pages(0, &[0]).unwrap(),
                automatic_front_cover: false,
            },
            content: Vec::new(),
            page_views: Vec::new(),
            prepare_policy: PageTurnPreparePolicy {
                post_filter_bypassed: false,
                smoothing: 0,
                upscale_source_limit: Default::default(),
                underlay_mode: 0,
                fit_mode: FullscreenFitMode::Page,
                no_upscale: false,
                no_downscale: false,
                specified: None,
            },
            output: PageTurnBindingOutput::Accepted,
        }
    }
}
