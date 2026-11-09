//! Cancellation-only diagnostics. These comparisons do not decide validity.
use super::*;

impl PaintedUnit {
    pub(super) fn different_fields(&self, other: &Self) -> Vec<String> {
        let mut fields = Vec::new();
        macro_rules! diff {
            ($name:literal, $same:expr) => {
                if !$same {
                    fields.push($name.to_owned());
                }
            };
        }
        diff!("content", self.content == other.content);
        diff!("destination", self.destination == other.destination);
        diff!("items", self.items == other.items);
        diff!("unit_binding", self.binding.layout == other.binding.layout);
        diff!("reading_flow", self.reading_flow == other.reading_flow);
        diff!(
            "prepare_policy",
            self.binding.same_view_policy(&other.binding)
        );
        diff!("mode", self.mode == other.mode);
        diff!("reading_rtl", self.reading_rtl == other.reading_rtl);
        diff!("slice", self.slice == other.slice);
        diff!("white", self.white == other.white);
        diff!("pages.len", self.pages.len() == other.pages.len());
        for (i, (a, b)) in self.pages.iter().zip(&other.pages).enumerate() {
            macro_rules! page_diff {
                ($name:literal, $same:expr) => {
                    if !$same {
                        fields.push(format!("pages[{i}].{}", $name));
                    }
                };
            }
            page_diff!("occurrence", a.occurrence == b.occurrence);
            page_diff!("crop", a.crop == b.crop);
            page_diff!(
                "source_texture_id",
                a.resource.source_texture_id() == b.resource.source_texture_id()
            );
            page_diff!(
                "paint_texture_id",
                a.resource.paint_texture_id() == b.resource.paint_texture_id()
            );
            page_diff!(
                "source_generation",
                a.resource.source_generation() == b.resource.source_generation()
            );
            page_diff!(
                "visible_source_uv_rect",
                a.resource.visible_source_uv_rect() == b.resource.visible_source_uv_rect()
            );
            page_diff!(
                "source_coverage",
                a.resource.source_coverage() == b.resource.source_coverage()
            );
            page_diff!(
                "paint_quad",
                a.transform.paint_quad() == b.transform.paint_quad()
            );
            page_diff!("uv_rect", a.transform.uv_rect == b.transform.uv_rect);
            page_diff!("fit_input", a.transform.fit_input == b.transform.fit_input);
            page_diff!(
                "fit_scale_limits",
                a.transform.fit_scale_limits == b.transform.fit_scale_limits
            );
            page_diff!(
                "pixels_per_point",
                a.transform.pixels_per_point == b.transform.pixels_per_point
            );
            page_diff!("spine", a.spine == b.spine);
            page_diff!("clip_rect", a.painter.clip_rect() == b.painter.clip_rect());
            page_diff!("underlay", same_underlay(&a.underlay, &b.underlay));
        }
        if self.binding != other.binding && fields.is_empty() {
            fields.push("prepared_binding".to_owned());
        }
        fields
    }

    fn layout_comparison(
        &self,
        layout: &PageTurnLayoutBinding,
        pages: &[PreparedPagedPage],
    ) -> serde_json::Value {
        serde_json::json!({
            "occurrences_equal": self.binding.layout.pages == layout.pages
                && self.pages.iter().map(|p| p.occurrence).eq(pages.iter().map(|p| p.occurrence)),
            "formation_equal": self.binding.layout.formation == layout.formation,
            "placement_equal": self.binding.layout.composition.singleton_placement == layout.composition.singleton_placement
                && self.pages.len() == pages.len()
                && self.pages.iter().zip(pages).all(|(a,b)| a.transform.placement == b.transform.placement),
        })
    }

    pub(super) fn cancel_comparison_fields(
        &self,
        other: &Self,
        origin: &'static str,
    ) -> Vec<(&'static str, serde_json::Value)> {
        let mut fields = vec![
            ("origin", origin.into()),
            ("different_fields", self.different_fields(other).into()),
        ];
        if self.binding.layout != other.binding.layout {
            fields.push((
                "layout",
                self.layout_comparison(&other.binding.layout, &other.pages),
            ));
        }
        fields
    }
}

impl App {
    pub(in crate::ui_fullscreen) fn page_turn_prepare_cancel_fields(
        &self,
        resource: &crate::gpu_lanczos::FullscreenPaintPayload,
        transform: &DisplayedImageTransform,
        painter: &egui::Painter,
        underlay: &FsBgStyle,
        live: bool,
    ) -> Vec<(&'static str, serde_json::Value)> {
        let mut fields = vec![("origin", "prepare_reuse".into())];
        let mut different = Vec::<&str>::new();
        if let PresentationState::Turning { incoming, .. }
        | PresentationState::Completing { incoming, .. } = &self.page_turn_presentation.state
        {
            macro_rules! diff {
                ($name:literal, $same:expr) => {
                    if !$same {
                        different.push($name);
                    }
                };
            }
            diff!(
                "destination",
                incoming.destination == self.page_turn_destination()
            );
            diff!("items", incoming.items == self.items_generation);
            diff!("mode", incoming.mode == self.spread_mode);
            diff!("slice", incoming.slice == self.fullscreen_page_slice);
            let current_layout = self.page_turn_resolved_layout(incoming.binding.layout.anchor);
            diff!(
                "unit_binding",
                current_layout.as_ref() == Some(&incoming.binding.layout)
            );
            diff!("reading_flow", incoming.reading_flow == self.reading_flow);
            diff!(
                "reading_rtl",
                incoming.reading_rtl == self.page_turn_reading_rtl()
            );
            diff!(
                "prepare_policy",
                incoming.binding.matches_prepare_policy(self)
            );
            diff!("live", !self.settings.page_turn_animation_enabled || live);
            if let Some(old) = incoming
                .pages
                .iter()
                .find(|page| Some(page.occurrence.idx) == resource.page_idx())
            {
                let idx = old.occurrence.idx;
                diff!(
                    "view_eligible",
                    !self.settings.page_turn_animation_enabled || self.page_turn_view_eligible(idx)
                );
                let content = incoming
                    .pages
                    .iter()
                    .position(|p| p.occurrence == old.occurrence)
                    .and_then(|i| incoming.content.get(i));
                diff!("content", content == Some(&self.page_turn_content(idx)));
                let a = &old.transform;
                let b = transform;
                diff!("viewport_rect", a.viewport_rect == b.viewport_rect);
                diff!("placement", a.placement == b.placement);
                diff!("pixels_per_point", a.pixels_per_point == b.pixels_per_point);
                diff!("rotation", a.rotation == b.rotation);
                diff!(
                    "free_rotation_rad",
                    a.free_rotation_rad == b.free_rotation_rad
                );
                diff!("content_bbox", a.content_bbox == b.content_bbox);
                diff!("fit_mode", a.fit_mode == b.fit_mode);
                diff!("fit_scale_limits", a.fit_scale_limits == b.fit_scale_limits);
                diff!("clip_rect", old.painter.clip_rect() == painter.clip_rect());
                diff!("underlay", same_underlay(&old.underlay, underlay));
                diff!("fit_policy", same_fit_policy(a.fit_input, b.fit_input));
                // These are the two final reuse predicates; matches_prepare compares
                // the held Lanczos key against the current filter/settings request.
                if self
                    .page_turn_matching_held_page(idx, transform, painter, underlay, live)
                    .is_some()
                    && let crate::gpu_lanczos::FullscreenPaintPayload::PresentationMaterial(
                        material,
                    ) = &old.resource
                {
                    diff!(
                        "material_coverage",
                        self.page_turn_material_covers_transform(resource, transform)
                    );
                    let post_filter = if self.post_filter_bypassed {
                        crate::adjustment::PostFilter::None
                    } else {
                        self.effective_params(idx).post_filter
                    };
                    diff!(
                        "prepare_key",
                        material.matches_prepare(
                            a.total_scale,
                            a.pixels_per_point,
                            material.visible_region_request(a, painter.clip_rect()),
                            post_filter,
                            self.settings.downscale_smoothing_percent,
                            self.settings.anime_upscale_source_limit,
                        )
                    );
                }
            } else {
                different.push("page_idx");
            }
            if let Some(layout) = current_layout.filter(|layout| *layout != incoming.binding.layout)
            {
                let mut pages = incoming.pages.clone();
                if let Some(page) = pages
                    .iter_mut()
                    .find(|page| Some(page.occurrence.idx) == resource.page_idx())
                {
                    page.transform = *transform;
                }
                fields.push(("layout", incoming.layout_comparison(&layout, &pages)));
            }
        }
        fields.push(("different_fields", different.into()));
        fields
    }
}
