use super::*;

impl App {
    pub(crate) fn prepare_canonical_chrome_for_test(&mut self, idx: usize) {
        self.fullscreen_seek_info_cached(idx);
    }
}

pub(super) fn ready_app(ctx: &egui::Context, mode: SpreadMode) -> crate::app::AppTestEnvForTest {
    let mut app = crate::ui_fullscreen::page_turn_submission_tests::ready_app(ctx, mode);
    app.settings.page_turn_animation_enabled = true;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
    app.settings.margin_fit_enabled = false;
    app.settings.still_seek_strip_visible = false;
    app.view_trim_book_preference = crate::view_trim::ViewTrimBookPreference::None;
    for idx in 0..4 {
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
        if let Some(FsCacheEntry::Static { tex, pixels, .. }) = app.fs_cache.get_mut(&idx) {
            // Sampler metadata belongs to allocation. TextureHandle::set does
            // not replace TextureMeta::options, so create the production flavor.
            *tex = ctx.load_texture(
                format!("canonical-source-{idx}"),
                pixels.clone(),
                crate::app::DISPLAY_IMAGE_TEXTURE_OPTIONS,
            );
            assert_eq!(
                ctx.tex_manager().read().meta(tex.id()).unwrap().options,
                crate::app::DISPLAY_IMAGE_TEXTURE_OPTIONS
            );
        }
    }
    app.spread_display_composition_for_anchor(0);
    app.fullscreen_seek_info_cached(0);
    super::super::page_turn::install(ctx);
    super::super::page_turn_submission_tests::frame(ctx, |ui, ctx| {
        super::super::page_turn_submission_tests::draw_current_without_sources(&mut app, ui, ctx);
    });
    assert!(app.page_turn_current_unit_presented());
    app
}

pub(super) fn view(ui: &egui::Ui) -> CanonicalViewInputs {
    CanonicalViewInputs {
        full_rect: ui.max_rect(),
        base_clip: ui.painter().clip_rect(),
        pixels_per_point: ui.ctx().pixels_per_point(),
    }
}

pub(super) fn actual_transforms(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    demand: &CanonicalUnitDemand,
) -> Vec<DisplayedImageTransform> {
    let next = demand.composition.navigation_anchor_idx;
    app.fullscreen_idx = Some(next);
    let composition = app.spread_display_composition_for_anchor(next);
    assert_eq!(composition, demand.composition);
    let media = app.fullscreen_media_rect(ui.max_rect(), next, false);
    assert_eq!(media, demand.media_rect);
    let screen = composition.pages_in_screen_order(app.spread_mode);
    if let [left, right] = screen.as_slice() {
        let seek = app.still_seek_geometry_for_idx(ui.max_rect(), next, false);
        app.draw_fs_spread_with_submission(
            ui,
            ctx,
            media,
            left.idx,
            right.idx,
            false,
            FsPageTurnDecision::normal(),
            None,
            seek,
            false,
            Some(&composition),
        );
        app.fullscreen_page_layout
            .displayed_pages()
            .map(|page| page.transform)
            .collect()
    } else {
        let resource = app.resident_canonical_resource(ctx, next).unwrap();
        let trim = app.view_trim_single_content_bbox(next);
        let source_size = app.fs_page_coordinate_source_size(next);
        let limits = app.fullscreen_fit_scale_limits(ctx.pixels_per_point());
        let rotation = app.get_rotation(next);
        let fit = app.effective_fullscreen_fit_mode();
        vec![
            app.draw_fs_image(
                ui,
                media,
                next,
                source_size,
                FsPageLayoutSource::CurrentItem,
                None,
                None,
                composition.singleton_placement(),
                Some(&resource),
                None,
                false,
                false,
                false,
                rotation,
                None,
                0.0,
                &FsBgStyle::Solid(egui::Color32::BLACK),
                "",
                false,
                fit,
                limits,
                trim,
            )
            .unwrap(),
        ]
    }
}

fn assert_actual_geometry(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
) -> CanonicalUnitDemand {
    let demand = app.collect_next_canonical_demand(ctx, view(ui)).unwrap();
    let actual = actual_transforms(app, ui, ctx, &demand);
    assert_eq!(
        actual,
        demand
            .pages
            .iter()
            .map(|page| page.transform)
            .collect::<Vec<_>>()
    );
    demand
}

pub(super) fn accept_current(app: &mut App, ctx: &egui::Context) {
    let current = app.fullscreen_idx.unwrap();
    app.spread_display_composition_for_anchor(current);
    app.fullscreen_seek_info_cached(current);
    super::super::page_turn::install(ctx);
    super::super::page_turn_submission_tests::frame(ctx, |ui, ctx| {
        super::super::page_turn_submission_tests::draw_current_without_sources(app, ui, ctx);
    });
    assert!(app.page_turn_current_unit_presented());
}

fn composed_app(
    ctx: &egui::Context,
    book: &crate::book_composition::BookComposition,
    anchor: usize,
) -> crate::app::AppTestEnvForTest {
    let mut app = super::super::book_composition_tests::composed_app(book);
    app.settings.page_turn_animation_enabled = true;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
    app.settings.margin_fit_enabled = false;
    app.view_trim_book_preference = crate::view_trim::ViewTrimBookPreference::None;
    app.fullscreen_idx = Some(anchor);
    for idx in 0..4 {
        let pixels = Arc::new(egui::ColorImage::filled(
            [80 + idx * 3, 120 + idx * 2],
            egui::Color32::LIGHT_BLUE,
        ));
        let tex = ctx.load_texture(
            format!("canonical-composed-{idx}"),
            pixels.clone(),
            crate::app::DISPLAY_IMAGE_TEXTURE_OPTIONS,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                pdf_rerender_failures: Vec::new(),
                pdf_metadata: None,
                tex,
                pixels: pixels.clone(),
                source_dims: Some(pixels.size),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
        app.record_page_dims_for_spread(idx, (pixels.size[0] as u32, pixels.size[1] as u32));
    }
    let composition = app.spread_display_composition_for_anchor(anchor);
    let permission =
        composition.presentation_demand(app.spread_mode, crate::page_split::PageSlice::Full);
    app.install_allowed_presentation_for_test(&permission);
    accept_current(&mut app, ctx);
    app
}

#[test]
fn canonical_native_demand_preserves_blank_vacant_and_supplement_compositions() {
    use crate::book_composition::{
        PageOverride, PageOverrideEntry, PageSide, ReadingDirection, fixtures,
    };
    for direction in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
        for directive in [
            PageOverride::Alone,
            PageOverride::BlankOpposite {
                page_side: PageSide::Leading,
            },
            PageOverride::BlankOpposite {
                page_side: PageSide::Trailing,
            },
        ] {
            let ctx = egui::Context::default();
            let mut book = fixtures::four_page_book(direction);
            book.page_overrides.push(PageOverrideEntry {
                page: fixtures::document_page(2),
                directive,
            });
            let mut app = composed_app(&ctx, &book, 0);
            super::super::page_turn_submission_tests::frame(&ctx, |ui, ctx| {
                let demand = assert_actual_geometry(&mut app, ui, ctx);
                assert_eq!(
                    demand.pages.len(),
                    1,
                    "white/vacant slots do not demand textures"
                );
                assert_eq!(demand.pages[0].occurrence.idx, 2);
            });
        }
        let ctx = egui::Context::default();
        let mut book = fixtures::four_page_book(direction);
        book.cover_enabled = true;
        let mut app = composed_app(&ctx, &book, 1);
        app.settings.final_cover_spread_enabled = true;
        // The changed endpoint policy rebinds the installed layout before accepting.
        accept_current(&mut app, &ctx);
        super::super::page_turn_submission_tests::frame(&ctx, |ui, ctx| {
            let demand = assert_actual_geometry(&mut app, ui, ctx);
            assert_eq!(demand.pages.len(), 2);
            assert!(
                demand
                    .pages
                    .iter()
                    .any(|page| page.occurrence.role == SpreadPageRole::FrontCoverSupplement)
            );
        });
    }
}

#[test]
fn canonical_native_demand_matches_unequal_rotated_trimmed_and_landscape_pages() {
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        for rotation in [
            crate::rotation_db::Rotation::None,
            crate::rotation_db::Rotation::Cw180,
        ] {
            let ctx = egui::Context::default();
            let mut app = ready_app(&ctx, mode);
            app.view_trim_book_preference = crate::view_trim::ViewTrimBookPreference::Manual;
            app.view_trim_book_settings.enabled = true;
            app.view_trim_book_settings.spread_linked.top = 0.1;
            app.view_trim_book_settings.spread_linked.bottom = 0.15;
            for (idx, size) in [(2, [95, 143]), (3, [81, 132])] {
                let pixels = Arc::new(egui::ColorImage::filled(size, egui::Color32::LIGHT_BLUE));
                let tex = ctx.load_texture(
                    format!("canonical-unequal-{idx}"),
                    pixels.clone(),
                    crate::app::DISPLAY_IMAGE_TEXTURE_OPTIONS,
                );
                app.fs_cache.insert(
                    idx,
                    FsCacheEntry::Static {
                        pdf_rerender_failures: Vec::new(),
                        pdf_metadata: None,
                        tex,
                        pixels,
                        source_dims: Some(size),
                        load_seq: 2,
                        animation: crate::fs_animation::StaticAnimationState::Still,
                    },
                );
                app.rotation_cache.insert(idx, rotation);
                app.record_page_dims_for_spread(idx, (size[0] as u32, size[1] as u32));
            }
            accept_current(&mut app, &ctx);
            super::super::page_turn_submission_tests::frame(&ctx, |ui, ctx| {
                assert_actual_geometry(&mut app, ui, ctx);
            });
        }
        let ctx = egui::Context::default();
        let mut app = ready_app(&ctx, mode);
        // Live fs_cache dimensions outrank the retained dimension cache. Model
        // an actual landscape publication, including the sampled pixels.
        let pixels = Arc::new(egui::ColorImage::filled(
            [120, 80],
            egui::Color32::LIGHT_BLUE,
        ));
        let tex = ctx.load_texture(
            "canonical-landscape-2",
            pixels.clone(),
            crate::app::DISPLAY_IMAGE_TEXTURE_OPTIONS,
        );
        app.fs_cache.insert(
            2,
            FsCacheEntry::Static {
                pdf_rerender_failures: Vec::new(),
                pdf_metadata: None,
                tex,
                pixels,
                source_dims: Some([120, 80]),
                load_seq: 2,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.record_fs_cache_page_dims_for_spread(2);
        assert_eq!(app.known_page_dims_for_idx(2), Some((120, 80)));
        app.spread_display_composition_for_anchor(0);
        accept_current(&mut app, &ctx);
        super::super::page_turn_submission_tests::frame(&ctx, |ui, ctx| {
            let demand = assert_actual_geometry(&mut app, ui, ctx);
            assert_eq!(demand.pages.len(), 1);
            assert_eq!(
                demand.composition.singleton_placement(),
                SingletonSpreadPlacement::Center
            );
        });
    }
}

#[test]
fn canonical_native_demand_matches_actual_single_and_pair_draw_geometry() {
    for mode in [
        SpreadMode::Single,
        SpreadMode::Ltr,
        SpreadMode::Rtl,
        SpreadMode::LtrCover,
        SpreadMode::RtlCover,
    ] {
        for fit_mode in [
            FullscreenFitMode::Page,
            FullscreenFitMode::Width,
            FullscreenFitMode::Height,
            FullscreenFitMode::Original,
            FullscreenFitMode::SpecifiedScale,
        ] {
            for ppp in [1.0, 1.25, 2.0] {
                let ctx = egui::Context::default();
                ctx.set_pixels_per_point(ppp);
                let mut app = ready_app(&ctx, mode);
                app.settings.fullscreen_fit_mode = fit_mode;
                crate::ui_fullscreen::page_turn_submission_tests::frame(&ctx, |ui, ctx| {
                    let demand = app.collect_next_canonical_demand(ctx, view(ui)).unwrap();
                    app.observe_canonical_native_demand(demand.clone());
                    assert!(app.canonical_unit_demand_is_current(ctx, &demand));
                    let next = demand.composition.navigation_anchor_idx;
                    app.fullscreen_idx = Some(next);
                    let composition = app.spread_display_composition_for_anchor(next);
                    assert_eq!(composition, demand.composition);
                    let media = app.fullscreen_media_rect(ui.max_rect(), next, false);
                    assert_eq!(media, demand.media_rect);
                    let screen = composition.pages_in_screen_order(mode);
                    let actual = if let [left, right] = screen.as_slice() {
                        let seek = app.still_seek_geometry_for_idx(ui.max_rect(), next, false);
                        app.draw_fs_spread_with_submission(
                            ui,
                            ctx,
                            media,
                            left.idx,
                            right.idx,
                            false,
                            FsPageTurnDecision::normal(),
                            None,
                            seek,
                            false,
                            Some(&composition),
                        );
                        app.fullscreen_page_layout
                            .displayed_pages()
                            .map(|page| page.transform)
                            .collect::<Vec<_>>()
                    } else {
                        let resource = app.resident_canonical_resource(ctx, next).unwrap();
                        let trim = app.view_trim_single_content_bbox(next);
                        let source_size = app.fs_page_coordinate_source_size(next);
                        let limits = app.fullscreen_fit_scale_limits(ctx.pixels_per_point());
                        let rotation = app.get_rotation(next);
                        let transform = app
                            .draw_fs_image(
                                ui,
                                media,
                                next,
                                source_size,
                                FsPageLayoutSource::CurrentItem,
                                None,
                                None,
                                composition.singleton_placement(),
                                Some(&resource),
                                None,
                                false,
                                false,
                                false,
                                rotation,
                                None,
                                0.0,
                                &FsBgStyle::Solid(egui::Color32::BLACK),
                                "",
                                false,
                                fit_mode,
                                limits,
                                trim,
                            )
                            .unwrap();
                        vec![transform]
                    };
                    assert_eq!(
                        actual,
                        demand
                            .pages
                            .iter()
                            .map(|page| page.transform)
                            .collect::<Vec<_>>(),
                        "{mode:?} {fit_mode:?} ppp={ppp}"
                    );
                });
            }
        }
    }
}

#[test]
fn canonical_native_demand_revalidates_the_whole_pair_and_view_policy() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Ltr);
    crate::ui_fullscreen::page_turn_submission_tests::frame(&ctx, |ui, ctx| {
        let demand = app.collect_next_canonical_demand(ctx, view(ui)).unwrap();
        app.observe_canonical_native_demand(demand.clone());
        assert_eq!(demand.pages.len(), 2);
        app.settings.spread_page_gap_px += 3;
        assert!(!app.canonical_unit_demand_is_current(ctx, &demand));
        app.settings.spread_page_gap_px -= 3;
        assert!(app.canonical_unit_demand_is_current(ctx, &demand));
        app.settings.downscale_smoothing_percent += 1;
        assert!(!app.canonical_unit_demand_is_current(ctx, &demand));
        app.settings.downscale_smoothing_percent -= 1;
        let mut resized = demand.clone();
        resized.view.full_rect.max.x -= 75.0;
        assert!(!app.canonical_unit_demand_is_current(ctx, &resized));
        let mut dpi_changed = demand.clone();
        dpi_changed.view.pixels_per_point = 1.5;
        assert!(!app.canonical_unit_demand_is_current(ctx, &dpi_changed));
        let mut clip_changed = demand.clone();
        clip_changed.view.base_clip.max.y -= 60.0;
        assert!(!app.canonical_unit_demand_is_current(ctx, &clip_changed));
        let sibling = demand.pages[1].occurrence.idx;
        app.rotation_cache
            .insert(sibling, crate::rotation_db::Rotation::Cw90);
        assert!(!app.canonical_unit_demand_is_current(ctx, &demand));
        app.rotation_cache
            .insert(sibling, crate::rotation_db::Rotation::None);
        assert!(app.canonical_unit_demand_is_current(ctx, &demand));
        *app.input_generation.entry(sibling).or_default() += 1;
        assert!(!app.canonical_unit_demand_is_current(ctx, &demand));
    });
}

#[test]
fn canonical_native_demand_requires_accepted_current_and_selected_resident_material() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    super::super::page_turn_submission_tests::frame(&ctx, |ui, ctx| {
        let demand = app.collect_next_canonical_demand(ctx, view(ui)).unwrap();
        app.observe_canonical_native_demand(demand.clone());
        let mut params = app.effective_params(1).clone();
        params.post_filter = PostFilter::Nearest;
        app.adjustment_page_params.insert(1, params);
        assert!(!app.canonical_unit_demand_is_current(ctx, &demand));
        app.adjustment_page_params.remove(&1);
        assert!(app.canonical_unit_demand_is_current(ctx, &demand));
        app.fs_cache.remove(&1);
        assert!(
            app.canonical_demand_resource_if_current(ctx, &demand, &demand.pages[0])
                .is_none()
        );
        assert!(!app.canonical_unit_demand_is_current(ctx, &demand));
    });
    drop(app);
    let mut initial =
        crate::ui_fullscreen::page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
    initial.settings.page_turn_animation_enabled = true;
    super::super::page_turn_submission_tests::frame(&ctx, |ui, ctx| {
        assert_eq!(
            initial.collect_next_canonical_demand(ctx, view(ui)),
            Err(CanonicalDemandUnavailable::Ineligible)
        );
        assert!(initial.final_effect_pending.is_empty());
    });
}

#[test]
fn canonical_native_demand_hidden_strip_does_not_require_cold_seek_metadata() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.viewer_navigation_caches.fs_seek_info = None;
    super::super::page_turn_submission_tests::frame(&ctx, |ui, ctx| {
        assert!(!app.settings.still_seek_strip_visible);
        let demand = app.collect_next_canonical_demand(ctx, view(ui)).unwrap();
        assert!(app.viewer_navigation_caches.fs_seek_info.is_none());
        app.settings.still_seek_strip_visible = true;
        assert_eq!(
            app.collect_next_canonical_demand(ctx, view(ui)),
            Err(CanonicalDemandUnavailable::Chrome)
        );
        assert!(app.viewer_navigation_caches.fs_seek_info.is_none());
        app.settings.still_seek_strip_visible = false;
        let actual = actual_transforms(&mut app, ui, ctx, &demand);
        assert_eq!(
            actual,
            demand
                .pages
                .iter()
                .map(|page| page.transform)
                .collect::<Vec<_>>()
        );
    });
}

#[test]
fn canonical_native_demand_does_not_read_cold_rotation_or_detect_margins() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    crate::ui_fullscreen::page_turn_submission_tests::frame(&ctx, |ui, ctx| {
        let inputs = view(ui);
        app.rotation_cache.remove(&1);
        assert_eq!(
            app.collect_next_canonical_demand(ctx, inputs),
            Err(CanonicalDemandUnavailable::Rotation)
        );
        assert!(!app.rotation_cache.contains_key(&1));
        app.rotation_cache
            .insert(1, crate::rotation_db::Rotation::None);
        app.view_trim_book_preference = crate::view_trim::ViewTrimBookPreference::Auto;
        assert!(app.fs_margin_bbox_cache.is_empty());
        assert_eq!(
            app.collect_next_canonical_demand(ctx, inputs),
            Err(CanonicalDemandUnavailable::AutoTrim)
        );
        assert!(app.fs_margin_bbox_cache.is_empty());
        let FsCacheEntry::Static {
            pixels, load_seq, ..
        } = app.fs_cache.get(&1).unwrap()
        else {
            panic!()
        };
        let cached = (*load_seq, Arc::as_ptr(pixels) as usize, None);
        app.fs_margin_bbox_cache.insert(1, cached);
        assert!(
            app.collect_next_canonical_demand(ctx, inputs).is_ok(),
            "a computed no-trim answer is usable"
        );
        app.fs_margin_bbox_cache.get_mut(&1).unwrap().0 += 1;
        assert_eq!(
            app.collect_next_canonical_demand(ctx, inputs),
            Err(CanonicalDemandUnavailable::AutoTrim)
        );
    });
}
