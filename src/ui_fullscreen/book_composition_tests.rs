#![cfg(test)]

use super::*;
use crate::book_composition::ReadingDirection as CompositionDirection;
use crate::book_composition::*;
use crate::settings::{PageAloneSettings, ReadingDirection, SingletonSpreadEndpointSettings};

fn resolved(book: &BookComposition, count: u32) -> ResolvedComposition {
    resolve_composition(
        book,
        &PageDomain::Document,
        &fixtures::document_manifest(count),
    )
    .unwrap()
}

fn composed_app(book: &BookComposition) -> crate::app::AppTestEnvForTest {
    let scope = crate::book_scope::BookScopeKey::document("c:/g3-display.pdf").unwrap();
    let count = 4;
    let raw = (0..count)
        .map(|page| CanonicalRawPage {
            page: fixtures::document_page(page),
            raw_index: page as usize,
        })
        .collect::<Vec<_>>();
    let prepared = project_composition(
        scope,
        &PageDomain::Document,
        CompositionToken {
            service_epoch: 3,
            publication_sequence: 7,
        },
        Some(book),
        &raw,
        &fixtures::document_manifest(count),
        count as usize,
    )
    .unwrap();
    let CompositionProjectionOutcome::Composed(projection) = prepared.outcome else {
        panic!("expected test composition");
    };
    let mut app = crate::app::setup_app_for_test();
    app.current_folder = Some(PathBuf::from("c:/g3-display.pdf"));
    app.projection_recipe = crate::app::book_materialization::ProjectionRecipe::Book(
        crate::app::book_materialization::BookScope::Document(PathBuf::from("c:/g3-display.pdf")),
    );
    app.items = prepared
        .output_to_input
        .iter()
        .map(|&page| {
            crate::app::book_materialization::document_page_item(
                Path::new("c:/g3-display.pdf"),
                page as u32,
            )
        })
        .collect();
    app.items_generation = 42;
    app.visible_indices = (0..app.items.len()).collect();
    app.details_order = app.visible_indices.clone();
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    app.image_metas = vec![None; app.items.len()];
    app.viewer_navigation_caches.invalidate();
    app.reading_direction = composition_view_direction(book.direction);
    app.spread_mode = (if book.cover_enabled {
        SpreadMode::LtrCover
    } else {
        SpreadMode::Ltr
    })
    .with_reading_direction(app.reading_direction);
    app.install_composition_for_test(projection);
    app
}

#[test]
fn composed_cache_ignores_dormant_shift_but_rebuilds_for_landscape_and_automatic_rules() {
    let mut app = composed_app(&fixtures::four_page_book(CompositionDirection::Ltr));
    let nav = app.get_nav_indices();
    let original = app.build_spread_display_units_for_nav(&nav);
    assert_eq!(
        original.iter().map(|u| u.pages.clone()).collect::<Vec<_>>(),
        vec![vec![0, 1], vec![2, 3]]
    );
    app.spread_shift = crate::spread_shift::contract_fixtures::four_page_book("c:/dormant")
        .live_owner(1, crate::spread_shift::ShiftPersistence::Saved(91));
    assert!(Arc::ptr_eq(
        &original,
        &app.build_spread_display_units_for_nav(&nav)
    ));
    app.rotation_cache
        .insert(1, crate::rotation_db::Rotation::None);
    app.record_page_dims_for_spread(1, (2000, 1000));
    let rebuilt = app.build_spread_display_units_for_nav(&nav);
    assert!(!Arc::ptr_eq(&original, &rebuilt));
    assert_eq!(
        rebuilt.iter().map(|u| u.pages.clone()).collect::<Vec<_>>(),
        vec![vec![0], vec![1], vec![2, 3]]
    );
}

fn specified_book(
    direction: CompositionDirection,
    directive: PageOverride,
    page: u32,
    cover: bool,
) -> BookComposition {
    let mut book = fixtures::four_page_book(direction);
    book.cover_enabled = cover;
    book.page_overrides.push(PageOverrideEntry {
        page: fixtures::document_page(page),
        directive,
    });
    book
}

#[test]
fn book_composition_rating_key_rebase_recomputes_paint_permission_and_masked_anchor() {
    for status in ["allowed", "hidden", "checking", "partial"] {
        let data = tempfile::tempdir().unwrap();
        let (service, owner) =
            BookCompositionService::spawn_at(data.path().to_path_buf(), None).unwrap();
        let mut app = composed_app(&fixtures::four_page_book(CompositionDirection::Ltr));
        app.install_book_composition_runtime(service);
        app.fullscreen_idx = Some(0);
        let old = app
            .spread_display_composition_for_anchor(0)
            .presentation_demand(app.spread_mode, app.fullscreen_page_slice);
        app.install_allowed_presentation_for_test(&old);
        assert!(app.resolve_book_fullscreen_presentation(0).allowed);

        // The rating key can replace the reader anchor inside the same paint pass.
        // A cached old permission cannot authorize the newly rebound unit.
        app.fullscreen_idx = Some(2);
        let next = app
            .spread_display_composition_for_anchor(2)
            .presentation_demand(app.spread_mode, app.fullscreen_page_slice);
        app.install_allowed_presentation_for_test(&next);
        let held = if status == "checking" {
            app.install_checking_presentation_for_test(&next)
        } else {
            Vec::new()
        };
        match status {
            "hidden" => app.install_hidden_presentation_for_test(&next),
            "partial" => app.install_hidden_presentation_for_test(&next[..1]),
            _ => {}
        }
        let rebound = app.resolve_book_fullscreen_presentation(2);
        assert_eq!(
            rebound.allowed,
            matches!(status, "allowed" | "partial"),
            "{status}"
        );
        if status == "partial" {
            assert_eq!(rebound.paint_idx, 3);
            assert_eq!(
                rebound
                    .composition
                    .pages_in_screen_order(app.spread_mode)
                    .iter()
                    .map(|page| page.idx)
                    .collect::<Vec<_>>(),
                vec![3]
            );
        }
        assert!(
            app.book_presentation_demand_snapshot()
                .iter()
                .all(|occurrence| next.contains(occurrence))
        );
        drop(held);
        owner.shutdown_and_join();
    }
}

#[test]
fn composed_blank_trim_uses_physical_slot_and_single_projection_uses_single_margin() {
    use crate::view_trim::{ViewTrimApplyMode, ViewTrimMargins, ViewTrimSpreadSide};
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        let mut app = composed_app(&specified_book(
            direction,
            PageOverride::BlankOpposite {
                page_side: PageSide::Leading,
            },
            1,
            false,
        ));
        app.view_trim_book_settings.enabled = true;
        app.view_trim_apply_mode = ViewTrimApplyMode::Book;
        app.view_trim_book_settings.spread_separate = true;
        app.view_trim_book_settings.single = ViewTrimMargins {
            left: 0.03,
            ..Default::default()
        };
        app.view_trim_book_settings.spread_left = ViewTrimMargins {
            left: 0.11,
            ..Default::default()
        };
        app.view_trim_book_settings.spread_right = ViewTrimMargins {
            right: 0.17,
            ..Default::default()
        };
        let side = if direction == CompositionDirection::Ltr {
            ViewTrimSpreadSide::Left
        } else {
            ViewTrimSpreadSide::Right
        };
        assert_eq!(app.composed_blank_spread_side(1), Some(side));
        assert_eq!(
            app.view_trim_single_content_bbox(1),
            app.view_trim_book_settings.spread_bbox(side)
        );
        app.spread_mode = SpreadMode::Single;
        assert_eq!(app.composed_blank_spread_side(1), None);
        assert_eq!(
            app.view_trim_single_content_bbox(1),
            app.view_trim_book_settings.single_bbox()
        );
    }
}

#[test]
fn legacy_matching_same_value_preserves_state_and_single_split_nudge_stays_navigation() {
    let ctx = egui::Context::default();
    let mut app = composed_app(&fixtures::four_page_book(CompositionDirection::Ltr));
    app.fullscreen_idx = Some(0);
    let settings = serde_json::to_value(&app.settings).unwrap();
    let units = app.build_spread_display_units_for_nav(&[0, 1, 2, 3]);
    assert!(app.apply_fullscreen_spread_mode(&ctx, 0, SpreadMode::Ltr));
    app.set_reading_direction_for_fullscreen(&ctx, 0, ReadingDirection::Ltr);
    let final_cover = app.final_cover_spread_preference;
    let page_alone = app.page_alone_preferences;
    let endpoint = app.singleton_spread_endpoint_preferences;
    app.set_final_cover_spread_preference_for_fullscreen(&ctx, final_cover);
    app.set_page_alone_preferences_for_fullscreen(&ctx, page_alone);
    app.set_singleton_spread_endpoint_preferences_for_fullscreen(&ctx, endpoint);
    assert_eq!(serde_json::to_value(&app.settings).unwrap(), settings);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert!(Arc::ptr_eq(
        &units,
        &app.build_spread_display_units_for_nav(&[0, 1, 2, 3])
    ));
    for mode in [
        SpreadMode::Single,
        SpreadMode::SplitLtr,
        SpreadMode::SplitRtl,
    ] {
        app.spread_mode = mode;
        for dir in [-1, 1] {
            let mut action = FsKeyAction::default();
            app.queue_spread_shift_action(0, dir, &mut action);
            assert_eq!(action.page_nav, FsPageNav::Delta(dir));
        }
    }
    app.spread_mode = SpreadMode::Ltr;
    let mut action = FsKeyAction::default();
    app.queue_spread_shift_action(0, 1, &mut action);
    assert!(action.page_nav.is_none());
}

#[test]
fn automatic_and_specified_groups_share_paged_continuous_roles_and_real_navigation() {
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        for directive in [
            None,
            Some(PageOverride::Alone),
            Some(PageOverride::BlankOpposite {
                page_side: PageSide::Leading,
            }),
        ] {
            let mut book = fixtures::four_page_book(direction);
            book.cover_enabled = true;
            if let Some(directive) = directive {
                book.page_overrides.push(PageOverrideEntry {
                    page: fixtures::document_page(3),
                    directive,
                });
            }
            let mut app = composed_app(&book);
            app.settings.final_cover_spread_enabled = true;
            let nav = app.get_nav_indices();
            let groups = app.build_spread_display_units_for_nav(&nav);
            let mut seen = Vec::new();
            for position in 0..groups.len() {
                let paged = resolve_spread_display_composition(
                    &groups,
                    position,
                    app.spread_mode,
                    true,
                    SingletonSpreadEndpointSettings::default(),
                    true,
                )
                .unwrap();
                let continuous =
                    ContinuousReadingUnitSpec::composition(paged.clone(), app.spread_mode);
                assert_eq!(
                    paged.pages_in_screen_order(app.spread_mode),
                    continuous.presentation
                );
                assert_eq!(
                    paged.presentation_demand(app.spread_mode, crate::page_split::PageSlice::Full),
                    continuous.presentation_demand()
                );
                seen.extend(paged.navigation_pages());
            }
            assert_eq!(&seen, nav.as_ref());
            let last = app.spread_display_composition_for_anchor(3);
            let has_cover = last
                .pages_in_reading_order()
                .iter()
                .any(|p| p.role == SpreadPageRole::FrontCoverSupplement);
            assert_eq!(
                has_cover,
                !matches!(directive, Some(PageOverride::BlankOpposite { .. }))
            );
        }
    }
}

#[test]
fn missing_or_hidden_page_is_removed_before_pairing() {
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        let mut book = fixtures::four_page_book(direction);
        book.hidden_pages.push(fixtures::document_page(1));
        let mut app = composed_app(&book);
        let nav = app.get_nav_indices();
        assert_eq!(nav.as_slice(), &[0, 1, 2]);
        assert_eq!(
            app.build_spread_display_units_for_nav(&nav)
                .iter()
                .map(|u| u.pages.clone())
                .collect::<Vec<_>>(),
            vec![vec![0, 1], vec![2]]
        );
        let mut manifest = fixtures::document_manifest(4);
        manifest[1].availability = ManifestAvailability::FilteredOut;
        book.hidden_pages.clear();
        let missing = resolve_composition(&book, &PageDomain::Document, &manifest).unwrap();
        let nav = missing
            .visible_pages
            .iter()
            .map(|p| p.item_index)
            .collect::<Vec<_>>();
        let groups = build_spread_display_units_with_overrides(
            &nav,
            app.spread_mode,
            None,
            PageAloneSettings::default(),
            &missing.page_overrides,
            |_, _| false,
            |_| true,
        );
        assert_eq!(
            groups.iter().map(|u| u.pages.clone()).collect::<Vec<_>>(),
            vec![vec![0, 2], vec![3]]
        );
    }
}

#[test]
fn permission_mask_keeps_physical_slot_and_automatic_cover_has_no_navigation() {
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        let mut app = composed_app(&fixtures::four_page_book(direction));
        let pair = app.spread_display_composition_for_anchor(0);
        for allowed in [vec![true, false], vec![false, true]] {
            let paged = pair
                .permitted_projection(app.spread_mode, &allowed)
                .unwrap();
            let unit = ContinuousReadingUnitSpec::composition(pair.clone(), app.spread_mode);
            let continuous = unit.permitted_projection(&allowed).unwrap();
            assert_eq!(paged.singleton_placement(), continuous.singleton_placement);
            assert_eq!(
                paged.pages_in_screen_order(app.spread_mode),
                continuous.presentation
            );
        }
        assert!(
            pair.permitted_projection(app.spread_mode, &[false, false])
                .is_none()
        );
        drop(app);
        let mut helper = composed_app(&specified_book(direction, PageOverride::Alone, 3, true));
        helper.settings.final_cover_spread_enabled = true;
        let unit = helper.spread_display_composition_for_anchor(3);
        assert_eq!(unit.navigation_pages(), vec![3]);
        let helper_only = unit
            .pages_in_screen_order(helper.spread_mode)
            .iter()
            .map(|p| p.role == SpreadPageRole::FrontCoverSupplement)
            .collect::<Vec<_>>();
        assert!(
            unit.permitted_projection(helper.spread_mode, &helper_only)
                .is_none()
        );
    }
}

#[test]
fn preparing_freezes_effective_spread_oracle_from_single_split_without_writes() {
    use crate::spread_shift::contract_fixtures::{four_page_book, preparing_snapshot};
    use crate::spread_shift::{ShiftPersistence, SpreadShiftPageRef};
    let mut app = crate::app::setup_app_for_test();
    for count in [1, 2, 3, 4, 5, 16] {
        let mut fixture = four_page_book("c:/g3-c4-book");
        fixture.pages = (0..count)
            .map(|page| SpreadShiftPageRef::RelativePath(format!("{page}.jpg")))
            .collect();
        app.current_folder = Some(PathBuf::from("c:/g3-c4-book"));
        app.top_level_grid_view
            .replace_surface(crate::app::top_level_grid_view::TopLevelGridSurface::Folder);
        app.visible_indices = (0..count).collect();
        app.details_order = (0..count).collect();
        app.thumbnails = vec![ThumbnailState::Pending; count];
        app.image_metas = vec![None; count];
        for shape in ["portrait", "late-landscape", "nonpairable"] {
            app.items = (0..count)
                .map(|page| GridItem::Image(PathBuf::from(format!("c:/g3-c4-book/{page}.jpg"))))
                .collect();
            if shape == "nonpairable" {
                app.items[count / 2] = GridItem::Video(PathBuf::from("c:/g3-c4-book/boundary.mp4"));
            }
            app.items_generation += 1;
            app.viewer_navigation_caches.invalidate();
            app.rotation_cache.clear();
            for page in 0..count {
                app.rotation_cache
                    .insert(page, crate::rotation_db::Rotation::None);
                app.record_page_dims_for_spread(page, (1000, 2000));
            }
            if shape == "late-landscape" {
                // Publish dimensions after the first topology cache exists,
                // exercising the real landscape epoch invalidation boundary.
                app.spread_mode = SpreadMode::Ltr;
                let nav = app.get_nav_indices();
                app.build_spread_display_units_for_nav(&nav);
                app.record_page_dims_for_spread(count / 2, (2000, 1000));
            }
            for mode in [
                SpreadMode::Ltr,
                SpreadMode::Rtl,
                SpreadMode::LtrCover,
                SpreadMode::RtlCover,
            ] {
                for shift in [0, count - 1] {
                    // Five independent released rule switches, all combinations.
                    for flags in 0u8..32 {
                        app.spread_mode = mode;
                        app.reading_direction = mode.reading_direction().unwrap();
                        app.spread_shift = fixture.live_owner(shift, ShiftPersistence::Saved(7));
                        app.settings.singleton_spread_first_enabled = flags & 1 != 0;
                        app.settings.singleton_spread_last_enabled = flags & 2 != 0;
                        app.settings.page_after_cover_alone_enabled = flags & 4 != 0;
                        app.settings.last_page_alone_enabled = flags & 8 != 0;
                        app.settings.final_cover_spread_enabled = flags & 16 != 0;
                        let oracle = preparing_snapshot(&mut app, 12, "accepted G token");
                        let nav = app.get_nav_indices();
                        let descriptor = |units: &[SpreadDisplayComposition]| {
                            units
                                .iter()
                                .map(|unit| {
                                    (
                                        unit.pages_in_screen_order(mode)
                                            .iter()
                                            .map(|page| (page.idx, page.role))
                                            .collect::<Vec<_>>(),
                                        unit.singleton_placement(),
                                    )
                                })
                                .collect::<Vec<_>>()
                        };
                        let expected = descriptor(&oracle.units);
                        let owner_before = format!("{:?}", app.spread_shift.state);
                        let settings_before = serde_json::to_value(&app.settings).unwrap();
                        for viewing in [
                            SpreadMode::Single,
                            SpreadMode::SplitLtr,
                            SpreadMode::SplitRtl,
                        ] {
                            app.spread_mode = viewing;
                            let captured = app.capture_effective_automatic_compositions(&nav, mode);
                            assert_eq!(
                                descriptor(&captured),
                                expected,
                                "count={count} shape={shape} mode={mode:?} shift={shift} flags={flags:05b} viewing={viewing:?}"
                            );
                            assert_eq!(app.spread_mode, viewing);
                            assert_eq!(format!("{:?}", app.spread_shift.state), owner_before);
                        }
                        assert_eq!(
                            serde_json::to_value(&app.settings).unwrap(),
                            settings_before
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn checking_placeholder_cannot_pick_geometry_source_or_fit_reference() {
    let mut app = crate::app::setup_app_for_test();
    app.settings.fullscreen_fit_mode = FullscreenFitMode::SpecifiedScale;
    let unit = ContinuousReadingUnitSpec {
        anchor_idx: 999,
        presentation: Vec::new(),
        singleton_placement: SingletonSpreadPlacement::Center,
        slice: crate::page_split::PageSlice::Full,
    };
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    let size = app.continuous_unit_size_for_flow(
        &unit,
        ReadingFlow::Vertical,
        rect,
        1.0,
        rect.size(),
        true,
        1.0,
    );
    assert!(size.pages.is_empty());
    app.prepare_continuous_specified_reference(&[unit], 0, rect.size(), true, 1.0);
    assert!(matches!(
        app.fs_continuous_fit_reference,
        ContinuousSpecifiedFitReference::Legacy
    ));
}

#[test]
fn retired_measurement_preserves_extents_without_image_authority_and_requires_exact_proof() {
    let data = tempfile::tempdir().unwrap();
    let (service, owner) =
        BookCompositionService::spawn_at(data.path().to_path_buf(), None).unwrap();
    let mut app = composed_app(&fixtures::four_page_book(CompositionDirection::Ltr));
    app.install_book_composition_runtime(service);
    app.reading_flow = ReadingFlow::Vertical;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
    app.settings.fullscreen_fit_no_upscale = false;
    app.settings.fullscreen_fit_no_downscale = false;
    let (canonical, _) = app.continuous_reading_units_and_pos(0).unwrap();
    let sources = vec![
        ContinuousSpecifiedFitReferenceSource {
            size: egui::vec2(200.0, 1200.0),
            auto_content_bbox: None
        };
        2
    ];
    let measurement = ContinuousUnitMeasurement {
        items_generation: app.items_generation,
        mode: app.spread_mode,
        composed: app.composed_layout_token(),
        identities: app
            .continuous_measurement_identities(&canonical[0])
            .unwrap(),
        sources,
    };
    app.spread_display_units_cache
        .continuous_measurements
        .push_back(measurement);
    app.retire_book_presentation_permission();
    let mut checking = canonical[0].clone();
    checking.presentation.clear();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    let (pages, _, _, range) = app
        .continuous_reading_layout_with_canonical(
            &egui::Context::default(),
            rect,
            &[checking],
            &canonical[..1],
            0,
            false,
        )
        .unwrap();
    assert!(pages.is_empty());
    assert!(
        range.1 - range.0 >= 599.0,
        "retirement preserves the accepted 1200px source extent"
    );
    assert!(matches!(
        app.fs_continuous_fit_reference,
        ContinuousSpecifiedFitReference::Legacy
    ));
    assert!(matches!(
        app.book_presentation_allowed(&canonical[0].presentation_demand()),
        PresentationDecision::Checking
    ));
    let mut different_slot = canonical[0].clone();
    different_slot.presentation.reverse();
    assert!(
        app.retained_continuous_measurement_sources(&different_slot)
            .is_none()
    );
    let mut different_slice = canonical[0].clone();
    different_slice.slice = crate::page_split::PageSlice::Left;
    assert!(
        app.retained_continuous_measurement_sources(&different_slice)
            .is_none()
    );
    let mut different_role = canonical[0].clone();
    different_role.presentation[0].role = SpreadPageRole::FrontCoverSupplement;
    assert!(
        app.retained_continuous_measurement_sources(&different_role)
            .is_none()
    );
    app.items.swap(0, 1);
    assert!(
        app.retained_continuous_measurement_sources(&canonical[0])
            .is_none()
    );
    app.items.swap(0, 1);
    app.items_generation += 1;
    assert!(
        app.retained_continuous_measurement_sources(&canonical[0])
            .is_none()
    );
    owner.shutdown_and_join();
}

#[test]
fn mode_only_handler_preserves_dormant_shift_and_exact_presentation_columns() {
    let data = tempfile::tempdir().unwrap();
    let (service, owner) =
        BookCompositionService::spawn_at(data.path().to_path_buf(), None).unwrap();
    let mut app = composed_app(&fixtures::four_page_book(CompositionDirection::Rtl));
    app.install_book_composition_runtime(service.clone());
    let scope = app.composed_layout_token().unwrap().0;
    let key = crate::spread_db::container_key(&scope.logical_root, &[]);
    let db = crate::spread_db::SpreadDb::open_at(&data.path().join("spread.db")).unwrap();
    db.set_presentation_state(
        &key,
        SpreadMode::RtlCover,
        ReadingFlow::Horizontal,
        ReadingDirection::Rtl,
        (
            SpreadMode::Single,
            ReadingFlow::Paged,
            ReadingDirection::Ltr,
        ),
    )
    .unwrap();
    app.spread_shift = crate::spread_shift::contract_fixtures::four_page_book("c:/dormant-mode")
        .live_owner(1, crate::spread_shift::ShiftPersistence::Saved(91));
    let shift = format!("{:?}", app.spread_shift.state);
    let settings = serde_json::to_value(&app.settings).unwrap();
    for (requested, expected) in [
        (FullscreenViewMode::Single, SpreadMode::Single),
        (FullscreenViewMode::LandscapeSplit, SpreadMode::SplitRtl),
        (FullscreenViewMode::Spread, SpreadMode::Rtl),
    ] {
        app.apply_fullscreen_view_mode(&egui::Context::default(), 0, requested);
        assert_eq!(app.spread_mode, expected);
        assert_eq!(format!("{:?}", app.spread_shift.state), shift);
        assert_eq!(serde_json::to_value(&app.settings).unwrap(), settings);
        service
            .prepare(scope.clone(), PageDomain::Document)
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap();
        assert_eq!(
            FullscreenViewMode::from_legacy(db.get(&key).unwrap()),
            requested
        );
        assert_eq!(db.get_flow(&key), Some(ReadingFlow::Horizontal));
        assert_eq!(db.get_direction(&key), Some(ReadingDirection::Rtl));
    }
    owner.shutdown_and_join();
}

#[test]
fn continuous_hidden_run_keeps_only_bounded_candidates_and_reentry_checks_again() {
    let mut app = crate::app::setup_app_for_test();
    app.items = (0..128)
        .map(|page| GridItem::Image(PathBuf::from(format!("c:/g3-hidden/{page}.png"))))
        .collect();
    app.thumbnails = vec![ThumbnailState::Pending; 128];
    app.image_metas = vec![None; 128];
    for idx in 0..128 {
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
    }
    app.items_generation += 1;
    app.spread_mode = SpreadMode::Single;
    app.reading_flow = ReadingFlow::Vertical;
    app.fullscreen_idx = Some(0);
    let units = (0..128)
        .map(|idx| ContinuousReadingUnitSpec::pages(idx, vec![idx]))
        .collect::<Vec<_>>();
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    let mut next = 0;
    while next < units.len() {
        let end = (next + 3).min(units.len());
        let hidden = units[next..end]
            .iter()
            .flat_map(ContinuousReadingUnitSpec::presentation_demand)
            .collect::<Vec<_>>();
        app.install_hidden_presentation_for_test(&hidden);
        app.permitted_continuous_reading_layout(&ctx, rect, &units, 0, false);
        assert!(app.presentation_demand_count_for_test() <= VERTICAL_READING_MAX_VISIBLE_PAGES);
        assert_eq!(
            app.fullscreen_idx,
            Some(0),
            "skip discovery never advances the reading anchor"
        );
        next = match &app.spread_display_units_cache.continuous_admission {
            ContinuousAdmissionEpisode::Searching { skipped, .. } => skipped.end,
            ContinuousAdmissionEpisode::Exhausted { .. } => units.len(),
            ContinuousAdmissionEpisode::Idle | ContinuousAdmissionEpisode::ModeTransition(_) => {
                panic!("active viewport must own its admission episode")
            }
        };
        assert!(next >= end);
    }
    assert!(matches!(
        app.spread_display_units_cache.continuous_admission,
        ContinuousAdmissionEpisode::Exhausted { .. }
    ));
    assert_eq!(app.presentation_demand_count_for_test(), 0);
    app.fs_vertical_scroll += 1.0;
    app.permitted_continuous_reading_layout(&ctx, rect, &units, 0, false);
    assert!(!matches!(
        app.spread_display_units_cache.continuous_admission,
        ContinuousAdmissionEpisode::Exhausted { .. }
    ));
    assert!(app.presentation_demand_count_for_test() <= VERTICAL_READING_MAX_VISIBLE_PAGES);
    app.retire_book_presentation_permission();
    app.permitted_continuous_reading_layout(&ctx, rect, &units, 0, false);
    assert!(!matches!(
        app.spread_display_units_cache.continuous_admission,
        ContinuousAdmissionEpisode::Exhausted { .. }
    ));
    assert!(app.presentation_demand_count_for_test() <= VERTICAL_READING_MAX_VISIBLE_PAGES);
}

#[test]
fn preparing_oracle_includes_landscape_boundaries_and_forced_singletons() {
    use crate::spread_shift::ShiftPersistence;
    use crate::spread_shift::contract_fixtures::{four_page_book, preparing_snapshot};
    let fixture = four_page_book("c:/g3-boundary");
    let mut app = crate::app::setup_app_for_test();
    app.current_folder = Some(PathBuf::from("c:/g3-boundary"));
    app.top_level_grid_view
        .replace_surface(crate::app::top_level_grid_view::TopLevelGridSurface::Folder);
    app.settings.final_cover_spread_enabled = true;
    app.settings.singleton_spread_first_enabled = true;
    app.settings.singleton_spread_last_enabled = true;
    for shape in [0, 1, 2] {
        app.items = (0..4)
            .map(|page| GridItem::Image(PathBuf::from(format!("c:/g3-boundary/{page}.jpg"))))
            .collect();
        if shape == 2 {
            app.items[2] = GridItem::Video(PathBuf::from("c:/g3-boundary/boundary.mp4"));
        }
        app.items_generation += 1;
        app.viewer_navigation_caches.invalidate();
        app.visible_indices = (0..4).collect();
        app.thumbnails = vec![ThumbnailState::Pending; 4];
        app.image_metas = vec![None; 4];
        app.rotation_cache.clear();
        for page in 0..4 {
            app.rotation_cache
                .insert(page, crate::rotation_db::Rotation::None);
        }
        app.record_page_dims_for_spread(
            2,
            if shape == 1 {
                (2000, 1000)
            } else {
                (1000, 2000)
            },
        );
        for after_cover in [false, true] {
            for last in [false, true] {
                app.settings.page_after_cover_alone_enabled = after_cover;
                app.settings.last_page_alone_enabled = last;
                for mode in [SpreadMode::LtrCover, SpreadMode::RtlCover] {
                    app.spread_mode = mode;
                    app.reading_direction = mode.reading_direction().unwrap();
                    app.spread_shift = fixture.live_owner(1, ShiftPersistence::SessionOnly);
                    let oracle = preparing_snapshot(&mut app, 9, "boundary token");
                    let nav = app.get_nav_indices();
                    app.spread_mode = SpreadMode::Single;
                    let frozen = app.capture_effective_automatic_compositions(&nav, mode);
                    let compare = |units: &[SpreadDisplayComposition]| {
                        units
                            .iter()
                            .map(|unit| {
                                (
                                    unit.navigation_pages(),
                                    unit.singleton_placement(),
                                    unit.pages_in_reading_order()
                                        .iter()
                                        .map(|page| (page.idx, page.role))
                                        .collect::<Vec<_>>(),
                                )
                            })
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(
                        compare(&frozen),
                        compare(&oracle.units),
                        "shape={shape} after={after_cover} last={last} mode={mode:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn continuous_hidden_cursor_exhausts_both_sides_of_a_middle_anchor() {
    let mut app = crate::app::setup_app_for_test();
    app.items = (0..12)
        .map(|page| GridItem::Image(PathBuf::from(format!("c:/g3-middle/{page}.png"))))
        .collect();
    app.thumbnails = vec![ThumbnailState::Pending; 12];
    app.image_metas = vec![None; 12];
    for idx in 0..12 {
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
    }
    app.items_generation += 1;
    app.spread_mode = SpreadMode::Single;
    app.reading_flow = ReadingFlow::Vertical;
    app.fullscreen_idx = Some(6);
    let units = (0..12)
        .map(|idx| ContinuousReadingUnitSpec::pages(idx, vec![idx]))
        .collect::<Vec<_>>();
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    for hidden_range in [6..9, 9..12, 4..6, 2..4, 0..2] {
        let hidden = units[hidden_range]
            .iter()
            .flat_map(ContinuousReadingUnitSpec::presentation_demand)
            .collect::<Vec<_>>();
        app.install_hidden_presentation_for_test(&hidden);
        app.permitted_continuous_reading_layout(&ctx, rect, &units, 6, false);
        assert_eq!(app.fullscreen_idx, Some(6));
        assert!(app.presentation_demand_count_for_test() <= VERTICAL_READING_MAX_VISIBLE_PAGES);
    }
    assert!(matches!(
        app.spread_display_units_cache.continuous_admission,
        ContinuousAdmissionEpisode::Exhausted { .. }
    ));
    assert_eq!(app.presentation_demand_count_for_test(), 0);
}

#[test]
fn separated_hidden_candidates_stay_excluded_during_the_same_viewport_episode() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    app.items = (0..12)
        .map(|page| GridItem::Image(PathBuf::from(format!("c:/g3-separated/{page}.png"))))
        .collect();
    app.thumbnails = vec![ThumbnailState::Pending; 12];
    app.image_metas = vec![None; 12];
    app.items_generation += 1;
    app.spread_mode = SpreadMode::Single;
    app.reading_flow = ReadingFlow::Vertical;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
    app.fullscreen_idx = Some(0);
    for idx in 0..12 {
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
        app.record_page_dims_for_spread(idx, (10, 10));
        let pixels = Arc::new(egui::ColorImage::filled([10, 10], egui::Color32::WHITE));
        let tex = ctx.load_texture(
            format!("g3-separated-{idx}"),
            pixels.clone(),
            egui::TextureOptions::LINEAR,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex,
                pixels,
                source_dims: Some([10, 10]),
                load_seq: 0,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
    }
    let units = (0..12)
        .map(|idx| ContinuousReadingUnitSpec::pages(idx, vec![idx]))
        .collect::<Vec<_>>();
    let hidden = units
        .iter()
        .enumerate()
        .filter(|(position, _)| position % 2 == 1)
        .flat_map(|(_, unit)| unit.presentation_demand())
        .collect::<Vec<_>>();
    app.install_hidden_presentation_for_test(&hidden);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    for _ in 0..3 {
        let (permitted, _, _, _, _) = app
            .permitted_continuous_reading_layout(&ctx, rect, &units, 0, false)
            .unwrap();
        assert!(
            permitted
                .iter()
                .flat_map(ContinuousReadingUnitSpec::source_indices)
                .all(|idx| idx % 2 == 0)
        );
        assert!(app.presentation_demand_count_for_test() <= VERTICAL_READING_MAX_VISIBLE_PAGES * 2);
    }
}

#[test]
fn alternating_hidden_window_over_forty_units_keeps_bounded_ranges_and_reentry_is_fresh() {
    let mut app = crate::app::setup_app_for_test();
    app.items = (0..128)
        .map(|page| GridItem::Image(PathBuf::from(format!("c:/g3-alternating/{page}.png"))))
        .collect();
    app.thumbnails = vec![ThumbnailState::Pending; 128];
    app.image_metas = vec![None; 128];
    app.items_generation += 1;
    app.spread_mode = SpreadMode::Single;
    app.reading_flow = ReadingFlow::Vertical;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
    app.fullscreen_idx = Some(0);
    for idx in 0..128 {
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
        app.record_page_dims_for_spread(idx, (10, 10));
    }
    let units = (0..128)
        .map(|idx| ContinuousReadingUnitSpec::pages(idx, vec![idx]))
        .collect::<Vec<_>>();
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    for _ in 0..6 {
        app.permitted_continuous_reading_layout(&ctx, rect, &units, 0, false)
            .unwrap();
        let hidden = app
            .book_presentation_demand_snapshot()
            .into_iter()
            .filter(|occurrence| occurrence.item_index % 2 == 1)
            .collect::<Vec<_>>();
        app.install_hidden_presentation_for_test(&hidden);
        app.permitted_continuous_reading_layout(&ctx, rect, &units, 0, false)
            .unwrap();
        let ContinuousAdmissionEpisode::Searching { hidden_ranges, .. } =
            &app.spread_display_units_cache.continuous_admission
        else {
            panic!("allowed pages keep the viewport active");
        };
        assert!(hidden_ranges.len() <= VERTICAL_READING_MAX_VISIBLE_PAGES);
        assert!(app.presentation_demand_count_for_test() <= VERTICAL_READING_MAX_VISIBLE_PAGES * 2);
    }
    app.fs_vertical_scroll += 100.0;
    app.permitted_continuous_reading_layout(&ctx, rect, &units, 0, false)
        .unwrap();
    let ContinuousAdmissionEpisode::Searching {
        skipped,
        hidden_ranges,
        ..
    } = &app.spread_display_units_cache.continuous_admission
    else {
        panic!("new viewport owns a fresh episode");
    };
    assert!(skipped.is_empty());
    assert!(
        hidden_ranges.is_empty(),
        "old Hidden endpoints are never a grant/exclusion on reentry"
    );
    assert!(app.presentation_demand_count_for_test() <= VERTICAL_READING_MAX_VISIBLE_PAGES * 2);
}

fn painted_texture_ids(ctx: &egui::Context, output: egui::FullOutput) -> Vec<egui::TextureId> {
    ctx.tessellate(output.shapes, output.pixels_per_point)
        .into_iter()
        .filter_map(|primitive| match primitive.primitive {
            egui::epaint::Primitive::Mesh(mesh) => Some(mesh.texture_id),
            egui::epaint::Primitive::Callback(_) => None,
        })
        .collect()
}

fn paint_composition_surface(
    app: &mut App,
    ctx: &egui::Context,
    rect: egui::Rect,
) -> (
    Vec<egui::TextureId>,
    Vec<crate::app::paint_record_test_support::PaintRecord>,
) {
    let output = crate::app::paint_record_test_support::with_capture(|| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                if app.reading_flow.is_paged() {
                    app.render_fullscreen_viewport(ctx);
                } else {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ctx, |ui| {
                            let idx = app.fullscreen_idx.unwrap();
                            app.draw_fs_continuous_reading(
                                ui,
                                ctx,
                                rect,
                                idx,
                                true,
                                FsPageTurnDecision::normal(),
                            );
                        });
                }
            },
        )
    });
    let records =
        crate::app::paint_record_test_support::paint_records(egui::ViewportId::ROOT, &output);
    (painted_texture_ids(ctx, output), records)
}

fn paint_record_shows_source(
    record: &crate::app::paint_record_test_support::PaintRecord,
    proof: &crate::gpu_lanczos::TestPaintProvenance,
) -> bool {
    record.provenance == *proof
        && egui::Rect::from_points(
            &record
                .vertices
                .iter()
                .map(|vertex| vertex.pos)
                .collect::<Vec<_>>(),
        )
        .intersect(record.clip)
        .is_positive()
}

#[test]
fn single_to_spread_actual_paint_retains_exact_old_surface_until_atomic_reply() {
    for extracted in [false, true] {
        for flow in [ReadingFlow::Paged, ReadingFlow::Vertical] {
            for hidden_companion in [false, true] {
                let case = format!(
                    "extracted={extracted}, flow={flow:?}, hidden_companion={hidden_companion}"
                );
                let data = tempfile::tempdir().unwrap();
                let (service, owner) =
                    BookCompositionService::spawn_at(data.path().to_path_buf(), None).unwrap();
                let scope = crate::book_scope::BookScopeKey::document("c:/g3-display.pdf").unwrap();
                service
                    .prepare(scope.clone(), PageDomain::Document)
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .unwrap()
                    .status
                    .unwrap();
                let ctx = egui::Context::default();
                install_fs_navigator_input_tracking(&ctx);
                // This fixture supplies cached PDF rasters, not a physical PDF.
                // Keep real resolution-demand calculation while intercepting
                // worker dispatch: a fake-file failure would replace the cache
                // with Failed and test file I/O rather than presentation admission.
                let pdf_rerender = crate::app::PdfRerenderRequestProbe::start();
                let mut app = composed_app(&fixtures::four_page_book(CompositionDirection::Ltr));
                if extracted {
                    app.settings.default_reading_direction = ReadingDirection::Ltr;
                    app.projection_recipe =
                        crate::app::book_materialization::ProjectionRecipe::ExtractedResult(
                            crate::app::book_materialization::ExtractedSurface::Rating,
                        );
                    app.begin_extracted_result_view();
                    assert!(app.composed_book_layout().is_none());
                }
                app.install_book_composition_runtime(service.clone());
                app.spread_mode = SpreadMode::Single;
                app.fullscreen_idx = Some(0);
                app.reading_flow = flow;
                app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
                app.settings.fullscreen_navigator_visible = false;
                app.settings.still_seek_strip_visible = false;
                app.settings.fullscreen_top_bar_locked = false;
                app.settings
                    .set_still_bottom_lock(crate::settings::BottomBarLock::None);
                #[cfg(windows)]
                {
                    app.native_video_in_window_active = true;
                    app.viewer_presentation = crate::app::ViewerPresentation::MainWindow;
                }
                let mut texture_ids = Vec::new();
                let mut cached_rasters = Vec::new();
                for idx in 0..4 {
                    let pixels = Arc::new(egui::ColorImage::filled(
                        [40, 60],
                        if idx == 0 {
                            egui::Color32::GREEN
                        } else {
                            egui::Color32::RED
                        },
                    ));
                    let tex = ctx.load_texture(
                        format!("g3-mode-{idx}"),
                        pixels.clone(),
                        egui::TextureOptions::LINEAR,
                    );
                    texture_ids.push(tex.id());
                    cached_rasters.push((tex.clone(), pixels.clone()));
                    app.fs_cache.insert(
                        idx,
                        FsCacheEntry::Static {
                            tex,
                            pixels,
                            source_dims: Some([40, 60]),
                            load_seq: 1,
                            animation: crate::fs_animation::StaticAnimationState::Still,
                        },
                    );
                    app.record_page_layout_dims_for_spread(idx, (40_000, 60_000));
                    app.rotation_cache
                        .insert(idx, crate::rotation_db::Rotation::None);
                }
                let old = app.book_target_presentation_demand(0);
                assert_eq!(old[0].slot, PresentationSlot::Center);
                app.install_allowed_presentation_for_test(&old);
                // Real paged paint uses final-pipeline resources; continuous
                // original preview uses raw resources. Track both cached forms.
                let mut processed_ids = (0..4)
                    .map(|idx| {
                        app.resolve_fs_processed_texture(&ctx, idx, false)
                            .unwrap()
                            .id()
                    })
                    .collect::<Vec<_>>();
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
                let a_proof = app.test_paint_provenance_for_idx(0).unwrap();
                let b_proof = app.test_paint_provenance_for_idx(1).unwrap();
                let (initial, initial_records) = paint_composition_surface(&mut app, &ctx, rect);
                assert!(
                    initial.contains(&texture_ids[0]) || initial.contains(&processed_ids[0]),
                    "extracted={extracted}, flow={flow:?}: an A-owned rendition must actually paint"
                );
                assert!(
                    initial_records
                        .iter()
                        .any(|record| paint_record_shows_source(record, &a_proof)),
                    "{case}: initial submitted visible A mesh records={initial_records:?}"
                );
                let old_rect = app
                    .fullscreen_page_layout
                    .displayed_pages()
                    .find(|page| page.page_idx() == 0)
                    .unwrap()
                    .transform
                    .layout_full_image_rect;
                if extracted {
                    let before = (
                        app.projection_recipe.clone(),
                        app.items_generation,
                        app.visible_indices.clone(),
                        app.details_order.clone(),
                        app.items
                            .iter()
                            .map(|item| format!("{item:?}"))
                            .collect::<Vec<_>>(),
                        app.thumbnails
                            .iter()
                            .map(|thumb| matches!(thumb, ThumbnailState::Pending))
                            .collect::<Vec<_>>(),
                    );
                    let nav = app.get_nav_indices();
                    let cached_units = Arc::clone(&app.spread_display_units_cache.units);
                    let mut external_record = fixtures::four_page_book(CompositionDirection::Ltr);
                    if hidden_companion {
                        external_record = external_record
                            .edited(
                                &PageDomain::Document,
                                CompositionEdit::SetHidden {
                                    page: fixtures::document_page(1),
                                    hidden: true,
                                },
                            )
                            .unwrap();
                    }
                    service
                        .save(scope.clone(), PageDomain::Document, external_record)
                        .recv_timeout(std::time::Duration::from_secs(30))
                        .unwrap()
                        .status
                        .unwrap();
                    app.poll_book_composition_service_for_test(&ctx);
                    assert_eq!(app.poll_and_remap_book_target(&ctx, 0), Some(0));
                    assert!(
                        service.drain_changed_scope_keys().is_empty(),
                        "actual publication drain consumed the exact changed scope"
                    );
                    let after = (
                        app.projection_recipe.clone(),
                        app.items_generation,
                        app.visible_indices.clone(),
                        app.details_order.clone(),
                        app.items
                            .iter()
                            .map(|item| format!("{item:?}"))
                            .collect::<Vec<_>>(),
                        app.thumbnails
                            .iter()
                            .map(|thumb| matches!(thumb, ThumbnailState::Pending))
                            .collect::<Vec<_>>(),
                    );
                    assert_eq!(
                        after, before,
                        "external book Save never reloads or invalidates the A2 result snapshot"
                    );
                    assert!(Arc::ptr_eq(&nav, &app.get_nav_indices()));
                    assert!(Arc::ptr_eq(
                        &cached_units,
                        &app.spread_display_units_cache.units
                    ));
                    assert!(
                        matches!(app.book_presentation_allowed(&old), PresentationDecision::Allowed(ref mask) if mask == &[true]),
                        "ongoing old Single episode remains Allowed after Save"
                    );
                    let (painted, records) = paint_composition_surface(&mut app, &ctx, rect);
                    assert!(
                        painted.contains(&texture_ids[0]) || painted.contains(&processed_ids[0])
                    );
                    assert!(
                        records
                            .iter()
                            .any(|record| paint_record_shows_source(record, &a_proof)),
                        "{case}: continuing visible Single after real Save records={records:?}"
                    );
                }
                let cache_before_mode = (0..4)
                    .map(|idx| {
                        (
                            idx,
                            match app.fs_cache.get(&idx) {
                                Some(FsCacheEntry::Static { .. }) => "Static",
                                Some(FsCacheEntry::Failed) => "Failed",
                                Some(_) => "Other",
                                None => "Absent",
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                assert!(
                    cache_before_mode
                        .iter()
                        .all(|(_, state)| *state != "Failed"),
                    "{case}: fixture dispatch must not create fake-file failures: {cache_before_mode:?}"
                );
                // Continuous prefetch legitimately retires unadmitted neighbors
                // while showing Single. Reestablish the cached-companion case
                // immediately before the mode switch, without granting B a permit.
                if !matches!(app.fs_cache.get(&1), Some(FsCacheEntry::Static { .. })) {
                    let (tex, pixels) = cached_rasters[1].clone();
                    app.fs_cache.insert(
                        1,
                        FsCacheEntry::Static {
                            tex,
                            pixels,
                            source_dims: Some([40, 60]),
                            load_seq: 1,
                            animation: crate::fs_animation::StaticAnimationState::Still,
                        },
                    );
                }
                processed_ids[1] = app
                    .resolve_fs_processed_texture(&ctx, 1, false)
                    .unwrap()
                    .id();
                pdf_rerender.take_requests();
                app.apply_fullscreen_view_mode(&ctx, 0, FullscreenViewMode::Spread);
                assert!(
                    matches!(
                        app.spread_display_units_cache.continuous_admission,
                        ContinuousAdmissionEpisode::ModeTransition(_)
                    ),
                    "extracted={extracted}, flow={flow:?}"
                );
                let next = app.book_target_presentation_demand(0);
                assert_eq!(next.len(), 2);
                assert_eq!(next[0].slot, PresentationSlot::Left);
                let checking = app.install_checking_presentation_for_test(&next);
                for _ in 0..2 {
                    let checking_snapshot = (
                        app.book_presentation_allowed(&old),
                        app.book_presentation_allowed(&next),
                        app.spread_display_units_cache.continuous_admission.clone(),
                    );
                    let (painted, records) = paint_composition_surface(&mut app, &ctx, rect);
                    assert!(
                        painted.contains(&texture_ids[0]) || painted.contains(&processed_ids[0]),
                        "{case}: old Allowed Single continues during Checking; before={checking_snapshot:?}, records={records:?}"
                    );
                    assert!(
                        !painted.contains(&texture_ids[1]) && !painted.contains(&processed_ids[1]),
                        "{case}: cached companion never grants paint"
                    );
                    assert!(
                        records
                            .iter()
                            .any(|record| paint_record_shows_source(record, &a_proof)),
                        "{case}: Checking must submit captured visible A mesh; before={checking_snapshot:?}, records={records:?}"
                    );
                    assert!(
                        records.iter().all(|record| record.provenance != b_proof),
                        "{case}: Checking has no B-source mesh across any cached rendition"
                    );
                    let displayed = app
                        .fullscreen_page_layout
                        .displayed_pages()
                        .collect::<Vec<_>>();
                    assert_eq!(displayed.len(), 1);
                    assert_eq!(
                        displayed[0].transform.layout_full_image_rect, old_rect,
                        "Checking retains captured Single geometry"
                    );
                    assert!(app.book_presentation_demand_snapshot().contains(&old[0]));
                }
                assert!(
                    pdf_rerender.take_requests().is_empty(),
                    "{case}: Checking must replay the old surface without new PDF sizing demands"
                );
                if extracted {
                    let correlations = app.checking_presentation_correlations_for_test(&next);
                    for ((occurrence, correlation), sender) in
                        next.iter().zip(correlations).zip(&checking)
                    {
                        let candidate = app.presentation_candidate(occurrence.item_index).unwrap();
                        let reply = service
                            .check_candidate(correlation.clone(), candidate)
                            .recv_timeout(std::time::Duration::from_secs(30))
                            .unwrap();
                        assert_eq!(reply.correlation, correlation);
                        assert!(matches!(
                            &reply.status,
                            Ok(CandidatePermission::Allowed(_))
                                | Ok(CandidatePermission::Hidden(_))
                        ));
                        assert_eq!(
                            matches!(&reply.status, Ok(CandidatePermission::Hidden(_))),
                            hidden_companion && occurrence.item_index == 1
                        );
                        sender.send(reply).unwrap();
                    }
                    app.poll_book_presentation_permission(&ctx);
                } else {
                    app.install_allowed_presentation_for_test(&next);
                    if hidden_companion {
                        app.install_hidden_presentation_for_test(&next[1..]);
                    }
                }
                assert!(
                    (0..2).all(|idx| matches!(
                        app.fs_cache.get(&idx),
                        Some(FsCacheEntry::Static { .. })
                    )),
                    "{case}: cached A/B must survive Checking; neighbors before mode={cache_before_mode:?}"
                );
                let terminal_snapshot = (
                    app.book_presentation_allowed(&next),
                    app.spread_display_units_cache.continuous_admission.clone(),
                    app.fullscreen_idx,
                    app.spread_mode,
                    app.composed_layout_token(),
                );
                let (_, records) = paint_composition_surface(&mut app, &ctx, rect);
                let terminal_pdf_requests = pdf_rerender.take_requests();
                assert!(
                    terminal_pdf_requests
                        .iter()
                        .all(|(idx, _)| { *idx == 0 || (!hidden_companion && *idx == 1) }),
                    "{case}: only the admitted terminal PDF projection may request rasters: {terminal_pdf_requests:?}"
                );
                if flow.is_paged() {
                    assert!(terminal_pdf_requests.iter().any(|(idx, _)| *idx == 0));
                    assert_eq!(
                        terminal_pdf_requests.iter().any(|(idx, _)| *idx == 1),
                        !hidden_companion,
                        "{case}: the real PDF demand consumer follows the terminal companion mask"
                    );
                }
                let terminal_demand = app.book_target_presentation_demand(0);
                assert!(
                    records
                        .iter()
                        .any(|record| paint_record_shows_source(record, &a_proof)),
                    "{case}: terminal projection must submit current visible A mesh; before={terminal_snapshot:?}, demand={terminal_demand:?}, after={:?}, records={records:?}, displayed={:?}",
                    app.book_presentation_allowed(&terminal_demand),
                    app.fullscreen_page_layout
                        .displayed_pages()
                        .map(|page| (page.page_idx(), page.transform.layout_full_image_rect))
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    records
                        .iter()
                        .any(|record| paint_record_shows_source(record, &b_proof)),
                    !hidden_companion,
                    "{case}: terminal B mesh follows exact decision; before={terminal_snapshot:?}, records={records:?}"
                );
                assert!(!matches!(
                    app.spread_display_units_cache.continuous_admission,
                    ContinuousAdmissionEpisode::ModeTransition(_)
                ));
                assert!(!app.book_presentation_demand_snapshot().contains(&old[0]));
                let displayed = app
                    .fullscreen_page_layout
                    .displayed_pages()
                    .collect::<Vec<_>>();
                assert_eq!(displayed.len(), if hidden_companion { 1 } else { 2 });
                assert!(displayed.iter().all(|page| page.page_idx() < 2));
                app.install_hidden_presentation_for_test(&next);
                let (painted, records) = paint_composition_surface(&mut app, &ctx, rect);
                assert!(
                    !painted.contains(&texture_ids[0])
                        && !painted.contains(&processed_ids[0])
                        && !painted.contains(&texture_ids[1])
                        && !painted.contains(&processed_ids[1]),
                    "both Hidden retires the old Single surface too"
                );
                assert!(
                    records
                        .iter()
                        .all(|record| record.provenance != a_proof && record.provenance != b_proof),
                    "{case}: both Hidden has no A/B-source submission"
                );
                drop(checking);
                owner.shutdown_and_join();
            }
        }
    }
}

#[test]
fn composed_facet_rebuilds_visible_pairs_and_disables_incomplete_endpoint_rules() {
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        let mut app = composed_app(&specified_book(direction, PageOverride::Alone, 3, true));
        app.visible_indices = vec![0, 2, 3];
        app.details_order = app.visible_indices.clone();
        app.viewer_navigation_caches.invalidate();
        app.settings.final_cover_spread_enabled = true;
        let paged = app.spread_display_composition_for_anchor(3);
        assert_eq!(paged.navigation_pages(), vec![3]);
        assert!(
            paged
                .pages_in_reading_order()
                .iter()
                .all(|p| p.role != SpreadPageRole::FrontCoverSupplement)
        );
        let (units, _) = app.continuous_reading_units_and_pos(3).unwrap();
        assert!(
            units
                .iter()
                .flat_map(|u| &u.presentation)
                .all(|p| p.idx != 1)
        );
        app.fullscreen_idx = Some(1);
        let outside = app.spread_display_composition_for_anchor(1);
        assert_eq!(outside.navigation_pages(), vec![1]);
    }
}

#[test]
fn same_generation_reader_order_change_retires_hidden_position_ranges() {
    let mut app = composed_app(&fixtures::four_page_book(CompositionDirection::Ltr));
    app.projection_recipe = crate::app::book_materialization::ProjectionRecipe::ExtractedResult(
        crate::app::book_materialization::ExtractedSurface::Rating,
    );
    app.begin_extracted_result_view();
    app.fullscreen_idx = Some(0);
    app.reading_flow = ReadingFlow::Vertical;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
    let ctx = egui::Context::default();
    for idx in 0..4 {
        let pixels = Arc::new(egui::ColorImage::filled([10, 10], egui::Color32::WHITE));
        let tex = ctx.load_texture(
            format!("g3-order-{idx}"),
            pixels.clone(),
            egui::TextureOptions::LINEAR,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex,
                pixels,
                source_dims: Some([10, 10]),
                load_seq: 0,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.record_page_layout_dims_for_spread(idx, (10, 10));
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
    }
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    let generation = app.items_generation;
    let (old_units, _) = app.continuous_reading_units_and_pos(0).unwrap();
    app.install_hidden_presentation_for_test(&old_units[1].presentation_demand());
    app.permitted_continuous_reading_layout(&ctx, rect, &old_units, 0, false);
    assert!(
        matches!(&app.spread_display_units_cache.continuous_admission, ContinuousAdmissionEpisode::Searching { skipped, .. } if skipped.contains(&1)),
        "the first Hidden run belongs to the primary cursor; no redundant disjoint range is needed"
    );
    app.visible_indices = vec![0, 2, 1, 3];
    app.details_order = app.visible_indices.clone();
    app.viewer_navigation_caches.invalidate();
    let (new_units, _) = app.continuous_reading_units_and_pos(0).unwrap();
    assert_eq!(new_units[1].anchor_idx, 2);
    let (permitted, ..) = app
        .permitted_continuous_reading_layout(&ctx, rect, &new_units, 0, false)
        .unwrap();
    assert!(
        permitted.iter().any(|unit| unit.anchor_idx == 2),
        "a position formerly Hidden cannot suppress its new identity"
    );
    assert!(
        permitted.iter().any(|unit| unit.anchor_idx == 1),
        "old Hidden proof retires on a reader-order episode change"
    );
    assert!(
        matches!(&app.spread_display_units_cache.continuous_admission, ContinuousAdmissionEpisode::Searching { skipped, hidden_ranges, .. } if skipped.is_empty() && hidden_ranges.is_empty()),
        "reader-order change retires primary and disjoint exclusions together"
    );
    assert_eq!(app.items_generation, generation);
    app.visible_indices = vec![0, 2, 3];
    app.details_order = app.visible_indices.clone();
    app.viewer_navigation_caches.invalidate();
    let (facet_units, _) = app.continuous_reading_units_and_pos(0).unwrap();
    let (permitted, ..) = app
        .permitted_continuous_reading_layout(&ctx, rect, &facet_units, 0, false)
        .unwrap();
    assert!(permitted.iter().all(|unit| unit.anchor_idx != 1));
    assert_eq!(app.items_generation, generation);
}

#[test]
fn continuous_actual_paint_never_uses_checking_or_hidden_cached_resources() {
    for cache in ["raw", "processed", "thumbnail", "holdover"] {
        let data = tempfile::tempdir().unwrap();
        let (service, owner) =
            BookCompositionService::spawn_at(data.path().to_path_buf(), None).unwrap();
        let ctx = egui::Context::default();
        install_fs_navigator_input_tracking(&ctx);
        let mut app = composed_app(&fixtures::four_page_book(CompositionDirection::Ltr));
        app.install_book_composition_runtime(service);
        app.fullscreen_idx = Some(0);
        app.reading_flow = ReadingFlow::Vertical;
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
        let mut source_ids = Vec::new();
        for idx in 0..4 {
            let pixels = Arc::new(egui::ColorImage::filled(
                [40, 60],
                if idx < 2 {
                    egui::Color32::GREEN
                } else {
                    egui::Color32::RED
                },
            ));
            let tex = ctx.load_texture(
                format!("g3-{cache}-{idx}"),
                pixels.clone(),
                egui::TextureOptions::LINEAR,
            );
            source_ids.push(tex.id());
            app.record_page_layout_dims_for_spread(idx, (40_000, 60_000));
            app.rotation_cache
                .insert(idx, crate::rotation_db::Rotation::None);
            if cache == "thumbnail" {
                app.thumbnails[idx] = ThumbnailState::Loaded {
                    tex,
                    origin: crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                        evaluated_display_px: 60,
                    },
                    from_edit_preview: false,
                    rendered_at_px: 60,
                    source_dims: Some((40, 60)),
                    layout_dims: None,
                };
                app.thumb_pixels.insert(idx, pixels);
                app.fs_cache.insert(
                    idx,
                    FsCacheEntry::RawPreview {
                        preview: None,
                        developed_dims: [40, 60],
                        load_seq: 1,
                    },
                );
            } else {
                app.fs_cache.insert(
                    idx,
                    FsCacheEntry::Static {
                        tex,
                        pixels,
                        source_dims: Some([40, 60]),
                        load_seq: 1,
                        animation: crate::fs_animation::StaticAnimationState::Still,
                    },
                );
            }
        }
        let (units, _) = app.continuous_reading_units_and_pos(0).unwrap();
        let a = units[0].presentation_demand();
        let b = units[1].presentation_demand();
        app.install_allowed_presentation_for_test(&a);
        app.install_allowed_presentation_for_test(&b);
        if cache == "processed" {
            for idx in 0..4 {
                let tex = app.resolve_fs_processed_texture(&ctx, idx, false).unwrap();
                source_ids.push(tex.id());
            }
        }
        if cache == "holdover" {
            let previous = app.capture_fs_navigation_display_unit(&ctx, 2).unwrap();
            app.fs_holdover_tex = Some(FsHoldover::PresentationSwitch(previous));
        }
        let checking_requests = app.install_checking_presentation_for_test(&b);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        for hidden in [false, true] {
            if hidden {
                app.install_hidden_presentation_for_test(&b);
            }
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ctx, |ui| {
                            app.draw_fs_continuous_reading(
                                ui,
                                ctx,
                                rect,
                                0,
                                cache != "processed",
                                FsPageTurnDecision::normal(),
                            );
                        });
                },
            );
            let painted = painted_texture_ids(&ctx, output);
            let allowed_ids = if cache == "processed" {
                &source_ids[4..6]
            } else {
                &source_ids[..2]
            };
            assert!(
                painted.iter().any(|id| allowed_ids.contains(id)),
                "{cache}: allowed A continues painting"
            );
            assert!(
                !painted.contains(&source_ids[2]) && !painted.contains(&source_ids[3]),
                "{cache}: B cache cannot grant presentation, hidden={hidden}"
            );
            if cache == "processed" {
                assert!(!painted.contains(&source_ids[6]) && !painted.contains(&source_ids[7]));
            }
            let displayed = app
                .fullscreen_page_layout
                .displayed_pages()
                .map(|page| page.page_idx())
                .collect::<Vec<_>>();
            assert!(
                displayed.iter().all(|idx| *idx < 2),
                "{cache}: no B transform/hit/trace occurrence"
            );
            assert!(matches!(
                app.fs_continuous_fit_reference,
                ContinuousSpecifiedFitReference::Legacy
            ));
        }
        drop(checking_requests);
        app.reading_flow = ReadingFlow::Paged;
        app.fullscreen_idx = Some(2);
        app.spread_display_units_cache.retire_continuous_admission();
        app.settings.fullscreen_navigator_visible = false;
        app.settings.still_seek_strip_visible = false;
        app.settings.fullscreen_top_bar_locked = false;
        app.settings
            .set_still_bottom_lock(crate::settings::BottomBarLock::None);
        #[cfg(windows)]
        {
            app.native_video_in_window_active = true;
            app.viewer_presentation = crate::app::ViewerPresentation::MainWindow;
        }
        let paged_checking = app.install_checking_presentation_for_test(&b);
        for hidden in [false, true] {
            if hidden {
                app.install_hidden_presentation_for_test(&b);
            }
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ctx| {
                    app.render_fullscreen_viewport(ctx);
                },
            );
            let painted = painted_texture_ids(&ctx, output);
            assert!(
                !painted.contains(&source_ids[2]) && !painted.contains(&source_ids[3]),
                "{cache}: paged B has no image grant, hidden={hidden}"
            );
            if cache == "processed" {
                assert!(!painted.contains(&source_ids[6]) && !painted.contains(&source_ids[7]));
            }
            assert!(
                app.fullscreen_page_layout
                    .displayed_pages()
                    .all(|page| page.page_idx() < 2),
                "{cache}: rejected paged sources have no transform/hit/trace occurrence"
            );
        }
        drop(paged_checking);
        owner.shutdown_and_join();
    }
}

#[test]
fn g3_continuous_visible_occurrences_keep_permission_through_scroll_reanchor_and_resize() {
    let data = tempfile::tempdir().unwrap();
    let (service, owner) =
        BookCompositionService::spawn_at(data.path().to_path_buf(), None).unwrap();
    service
        .prepare(
            crate::book_scope::BookScopeKey::document("c:/g3-display.pdf").unwrap(),
            PageDomain::Document,
        )
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    let (worker_entered, paused_worker) = service.hold_worker_for_test();
    worker_entered
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap();
    let mut app = composed_app(&fixtures::four_page_book(CompositionDirection::Ltr));
    app.install_book_composition_runtime(service);
    app.spread_mode = SpreadMode::Single;
    app.fullscreen_idx = Some(0);
    app.reading_flow = ReadingFlow::Vertical;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
    app.settings.continuous_reading_gap_px = 0;
    let ctx = egui::Context::default();
    let mut ids = Vec::new();
    for idx in 0..4 {
        let pixels = Arc::new(egui::ColorImage::filled([400, 600], egui::Color32::GREEN));
        let tex = ctx.load_texture(
            format!("g3-retained-{idx}"),
            pixels.clone(),
            egui::TextureOptions::LINEAR,
        );
        ids.push(tex.id());
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex,
                pixels,
                source_dims: Some([400, 600]),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.record_page_layout_dims_for_spread(idx, (400, 600));
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
    }
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    let (units, _) = app.continuous_reading_units_and_pos(0).unwrap();
    let a = units[0].presentation_demand();
    let b = units[1].presentation_demand();
    app.install_allowed_presentation_for_test(&a);
    let held_b = app.install_checking_presentation_for_test(&b);
    let b_correlation = app.checking_presentation_correlations_for_test(&b);
    let draw = |app: &mut crate::app::App, rect: egui::Rect| {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        app.draw_fs_continuous_reading(
                            ui,
                            ctx,
                            rect,
                            app.fullscreen_idx.unwrap(),
                            true,
                            FsPageTurnDecision::normal(),
                        );
                    });
            },
        );
        painted_texture_ids(&ctx, output)
    };
    app.fs_vertical_scroll = 1.0;
    assert!(draw(&mut app, rect).contains(&ids[0]));
    for step in [0.05, 0.25, 0.5] {
        app.fs_vertical_scroll += step;
        let painted = draw(&mut app, rect);
        assert!(
            painted.contains(&ids[0]),
            "micro scroll retains cached A while B waits"
        );
        assert!(
            !painted.contains(&ids[1]),
            "cached newly presented B has no permission"
        );
        assert!(matches!(
            app.book_presentation_allowed(&a),
            PresentationDecision::Allowed(_)
        ));
        assert_eq!(
            app.checking_presentation_correlations_for_test(&b),
            b_correlation
        );
    }
    let (_, _, _, offsets, _) = app
        .permitted_continuous_reading_layout(&ctx, rect, &units, 0, false)
        .unwrap();
    app.reanchor_continuous_reading_viewer(
        &ctx,
        &offsets,
        1,
        1,
        crate::page_split::PageSlice::Full,
        crate::app::HistoryTrigger::UserChosen,
    );
    for size in [egui::vec2(810.0, 620.0), egui::vec2(820.0, 610.0)] {
        let resized = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let painted = draw(&mut app, resized);
        assert!(
            painted.contains(&ids[0]),
            "reanchor/resize preserves still visible A"
        );
        assert!(!painted.contains(&ids[1]));
        assert!(matches!(
            app.book_presentation_allowed(&a),
            PresentationDecision::Allowed(_)
        ));
        assert_eq!(
            app.checking_presentation_correlations_for_test(&b),
            b_correlation
        );
    }
    // Actual viewport exit retires the grant. Cached A must wait on a fresh
    // request when it enters again, unlike the overlapping cases above.
    app.fs_vertical_scroll = 650.0;
    draw(&mut app, rect);
    assert!(
        !app.book_presentation_demand_snapshot()
            .iter()
            .any(|occ| a.contains(occ))
    );
    app.fs_vertical_scroll = -599.0;
    let painted = draw(&mut app, rect);
    assert!(!painted.contains(&ids[0]));
    assert!(matches!(
        app.book_presentation_allowed(&a),
        PresentationDecision::Checking
    ));
    assert!(
        !app.checking_presentation_correlations_for_test(&a)
            .is_empty()
    );
    drop(held_b);
    paused_worker.send(()).unwrap();
    owner.shutdown_and_join();
}

#[test]
fn composed_continuous_geometry_and_pdf_demand_match_the_real_e_consumer() {
    use crate::app::PdfRerenderRequestProbe;
    for flow in [ReadingFlow::Vertical, ReadingFlow::Horizontal] {
        for fit in [
            FullscreenFitMode::Original,
            FullscreenFitMode::SpecifiedScale,
        ] {
            for limited in [false, true] {
                let mut oracle = None;
                for composed in [false, true] {
                    let ctx = egui::Context::default();
                    let mut app = super::specified_scale_consumer_tests::pdf_app(&ctx, 4);
                    let probe = PdfRerenderRequestProbe::start();
                    app.reading_flow = flow;
                    app.spread_mode = SpreadMode::Ltr;
                    app.settings.spread_page_gap_px = 13;
                    app.settings.fullscreen_fit_mode = fit;
                    app.settings.fullscreen_fit_no_upscale = limited;
                    app.settings.fullscreen_fit_no_downscale = limited;
                    app.record_page_layout_dims_for_spread(1, (400_000, 1_000_000));
                    for idx in 0..4 {
                        app.rotation_cache
                            .insert(idx, crate::rotation_db::Rotation::None);
                    }
                    if composed {
                        let GridItem::PdfPage { pdf_path, .. } = app.items[0].clone() else {
                            unreachable!()
                        };
                        let scope = crate::book_scope::BookScopeKey::document(&pdf_path).unwrap();
                        app.current_folder = Some(pdf_path.clone());
                        app.projection_recipe =
                            crate::app::book_materialization::ProjectionRecipe::Book(
                                crate::app::book_materialization::BookScope::Document(pdf_path),
                            );
                        let raw = (0..4)
                            .map(|page| CanonicalRawPage {
                                page: fixtures::document_page(page),
                                raw_index: page as usize,
                            })
                            .collect::<Vec<_>>();
                        let projected = project_composition(
                            scope,
                            &PageDomain::Document,
                            CompositionToken {
                                service_epoch: 1,
                                publication_sequence: 1,
                            },
                            Some(&fixtures::four_page_book(CompositionDirection::Ltr)),
                            &raw,
                            &fixtures::document_manifest(4),
                            4,
                        )
                        .unwrap();
                        let CompositionProjectionOutcome::Composed(projection) = projected.outcome
                        else {
                            unreachable!()
                        };
                        app.install_composition_for_test(projection);
                        app.viewer_navigation_caches.invalidate();
                    }
                    let mut records = Vec::new();
                    let mut demands = Vec::new();
                    for (size, zoom, scroll) in [
                        (egui::vec2(1200.0, 800.0), 1.0, 0.0),
                        (egui::vec2(1600.0, 1000.0), 1.5, 100.0),
                    ] {
                        app.fs_zoom = zoom;
                        app.fs_vertical_scroll = scroll;
                        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                        let _ = ctx.run(
                            egui::RawInput {
                                screen_rect: Some(rect),
                                ..Default::default()
                            },
                            |ctx| {
                                egui::CentralPanel::default().frame(egui::Frame::NONE).show(
                                    ctx,
                                    |ui| {
                                        app.draw_fs_continuous_reading(
                                            ui,
                                            ctx,
                                            rect,
                                            0,
                                            true,
                                            FsPageTurnDecision::normal(),
                                        );
                                        app.reconcile_specified_pdf_demand(ctx);
                                    },
                                );
                            },
                        );
                        records.push(
                            app.fullscreen_page_layout
                                .displayed_pages()
                                .map(|page| {
                                    (page.page_idx(), page.transform.layout_full_image_rect)
                                })
                                .collect::<Vec<_>>(),
                        );
                        let mut request = probe.take_requests();
                        request.sort_unstable();
                        demands.push(request);
                        assert_eq!(
                            app.fs_zoom, zoom,
                            "the presentation adapter never invents manual zoom"
                        );
                    }
                    assert!(!records[0].is_empty());
                    if let Some((expected_records, expected_demands)) = &oracle {
                        assert_eq!(
                            &records, expected_records,
                            "flow={flow:?} fit={fit:?} limited={limited}"
                        );
                        assert_eq!(
                            &demands, expected_demands,
                            "canonical PDF demand stays on the E consumer"
                        );
                    } else {
                        oracle = Some((records, demands));
                    }
                }
            }
        }
    }
}

#[test]
fn g3_similar_preview_continuous_routing_requires_admitted_anchor_occurrence() {
    for expected in ["checking", "hidden", "allowed"] {
        let data = tempfile::tempdir().unwrap();
        let (service, owner) =
            BookCompositionService::spawn_at(data.path().to_path_buf(), None).unwrap();
        let ctx = egui::Context::default();
        let mut app = composed_app(&fixtures::four_page_book(CompositionDirection::Ltr));
        app.install_book_composition_runtime(service);
        app.fullscreen_idx = Some(0);
        app.reading_flow = ReadingFlow::Vertical;
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
        for idx in 0..4 {
            app.record_page_layout_dims_for_spread(idx, (40_000, 60_000));
            app.rotation_cache
                .insert(idx, crate::rotation_db::Rotation::None);
        }
        let (units, _) = app.continuous_reading_units_and_pos(0).unwrap();
        for unit in &units {
            app.install_allowed_presentation_for_test(&unit.presentation_demand());
        }
        let demand = units[0].presentation_demand();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                focused: true,
                events: vec![egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |_| {},
        );
        let hit = crate::similar_index::QueryHit {
            item_id: 901,
            item_key: "c:/preview/candidate.png".into(),
            kind: crate::similar_db::ItemKind::Image,
            container_key: None,
            page_index: None,
            distance: 1,
            band: crate::similar_index::MatchBand::NearlyIdentical,
            mtime: 7,
            file_size: 20,
            width: 32,
            height: 24,
            format: crate::similar_image::SimilarImageFormat::Png,
            target: Some(crate::similar_index::SimilarItemTarget::File(
                "c:/preview/candidate.png".into(),
            )),
        };
        let passwords = app.pdf_passwords.clone();
        let session = app.similar_preview_session(&ctx).unwrap();
        assert_eq!(
            session.layout_mode,
            crate::similar_preview::SimilarPreviewLayoutMode::Continuous
        );
        assert!(app.similar_panel.preview.begin_test_press(
            &ctx,
            &hit,
            &passwords,
            crate::pdf_loader::PdfDisplayTarget {
                width_px: 800,
                height_px: 600,
                fit_mode: crate::pdf_loader::PdfDisplayFitMode::Page
            },
            session
        ));
        app.similar_panel
            .preview
            .take_test_completion()
            .send_prepared(
                Arc::new(egui::ColorImage::filled(
                    [24, 16],
                    egui::Color32::LIGHT_BLUE,
                )),
                [2400, 1600],
                None,
                7,
            )
            .unwrap();
        // A prepared/cache-resident candidate cannot authorize the surface it would replace.
        app.similar_panel.preview.poll_background(&ctx, &passwords);
        let candidate_texture = app.similar_panel.preview.cached_texture().unwrap().id();
        let checking = if expected == "checking" {
            app.install_checking_presentation_for_test(&demand)
        } else {
            Vec::new()
        };
        if expected == "hidden" {
            app.install_hidden_presentation_for_test(&demand);
        }
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                focused: true,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        let asset = app
                            .similar_preview_asset_for_permitted_surface(ctx, rect, 0, false, true);
                        assert_eq!(
                            asset.is_some(),
                            expected == "allowed",
                            "{expected} anchor admission"
                        );
                        if let Some(asset) = asset {
                            let style = app.fs_bg_style(ctx);
                            app.draw_similar_preview_asset(ui, rect, &asset, &style);
                        }
                    });
            },
        );
        assert_eq!(
            painted_texture_ids(&ctx, output).contains(&candidate_texture),
            expected == "allowed",
            "{expected} candidate painting"
        );
        drop(checking);
        owner.shutdown_and_join();
    }
}

#[test]
fn explicit_directives_preserve_boundaries_and_blank_priority_for_all_automatic_rules() {
    for mode in [
        SpreadMode::Ltr,
        SpreadMode::Rtl,
        SpreadMode::LtrCover,
        SpreadMode::RtlCover,
    ] {
        for count in 1..=9 {
            let nav = (0..count).collect::<Vec<_>>();
            for at in 0..count {
                for directive in [
                    PageOverride::Alone,
                    PageOverride::BlankOpposite {
                        page_side: PageSide::Leading,
                    },
                    PageOverride::BlankOpposite {
                        page_side: PageSide::Trailing,
                    },
                ] {
                    let overrides = HashMap::from([(at, directive)]);
                    for flags in 0u8..32 {
                        let page_alone = PageAloneSettings {
                            after_cover: flags & 1 != 0,
                            last: flags & 2 != 0,
                        };
                        let endpoints = SingletonSpreadEndpointSettings {
                            first: flags & 4 != 0,
                            last: flags & 8 != 0,
                        };
                        let units = build_spread_display_units_with_overrides(
                            &nav,
                            mode,
                            None,
                            page_alone,
                            &overrides,
                            |_, i| i == at,
                            |_| true,
                        );
                        assert_eq!(
                            units
                                .iter()
                                .flat_map(|u| u.pages.iter().copied())
                                .collect::<Vec<_>>(),
                            nav
                        );
                        let (pos, unit) = find_spread_display_unit(&units, at).unwrap();
                        assert_eq!(unit.pages, vec![at]);
                        let result = resolve_spread_display_composition(
                            &units,
                            pos,
                            mode,
                            flags & 16 != 0,
                            endpoints,
                            true,
                        )
                        .unwrap();
                        match directive {
                            PageOverride::BlankOpposite { page_side } => {
                                let right = (page_side == PageSide::Leading) == mode.is_rtl();
                                assert_eq!(
                                    result.singleton_placement(),
                                    if right {
                                        SingletonSpreadPlacement::RightWhite
                                    } else {
                                        SingletonSpreadPlacement::LeftWhite
                                    }
                                );
                                assert_eq!(result.pages_in_reading_order().len(), 1);
                            }
                            PageOverride::Alone => assert_eq!(
                                result.singleton_placement(),
                                SingletonSpreadPlacement::Center
                            ),
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn hidden_first_and_last_redefine_automatic_cover_endpoints() {
    let mut book = fixtures::four_page_book(CompositionDirection::Ltr);
    book.cover_enabled = true;
    book.hidden_pages = vec![fixtures::document_page(0), fixtures::document_page(3)];
    let mut app = composed_app(&book);
    app.settings.final_cover_spread_enabled = true;
    let last = app.spread_display_composition_for_anchor(1);
    assert_eq!(last.navigation_pages(), vec![1]);
    assert_eq!(
        last.pages_in_reading_order()
            .iter()
            .map(|p| (p.idx, p.role))
            .collect::<Vec<_>>(),
        vec![
            (1, SpreadPageRole::Navigation),
            (0, SpreadPageRole::FrontCoverSupplement)
        ]
    );
    assert_eq!(app.items.len(), 2);
}

#[test]
fn current_outside_facet_keeps_explicit_white_side_in_paged_and_continuous() {
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        for page_side in [PageSide::Leading, PageSide::Trailing] {
            let mut app = composed_app(&specified_book(
                direction,
                PageOverride::BlankOpposite { page_side },
                1,
                true,
            ));
            app.visible_indices = vec![0, 2];
            app.details_order = app.visible_indices.clone();
            app.viewer_navigation_caches.invalidate();
            let right =
                (page_side == PageSide::Leading) == (direction == CompositionDirection::Rtl);
            let expected = if right {
                SingletonSpreadPlacement::RightWhite
            } else {
                SingletonSpreadPlacement::LeftWhite
            };
            for flow in [ReadingFlow::Vertical, ReadingFlow::Horizontal] {
                app.reading_flow = flow;
                let paged = app.spread_display_composition_for_anchor(1);
                assert_eq!(paged.singleton_placement(), expected);
                assert_eq!(paged.navigation_pages(), vec![1]);
                let (units, pos) = app
                    .continuous_reading_units_and_pos(1)
                    .expect("shown facet-excluded anchor survives");
                assert_eq!(units[pos].source_indices().collect::<Vec<_>>(), vec![1]);
                assert_eq!(units[pos].singleton_placement, expected);
                assert!(
                    units
                        .iter()
                        .flat_map(|u| &u.presentation)
                        .all(|p| p.idx != 3)
                );
                assert!(
                    units[pos]
                        .presentation
                        .iter()
                        .all(|p| p.role == SpreadPageRole::Navigation)
                );
            }
        }
    }
}

#[test]
fn composed_automatic_preference_writes_change_shared_rules_without_record_writes() {
    let mut book = fixtures::four_page_book(CompositionDirection::Ltr);
    book.cover_enabled = true;
    let mut app = composed_app(&book);
    app.fullscreen_idx = Some(1);
    let before = app.composed_book_layout().unwrap().clone();
    let token = app.composed_layout_token();
    let ctx = egui::Context::default();
    app.set_final_cover_spread_preference_for_fullscreen(
        &ctx,
        crate::settings::FinalCoverSpreadPreference::Off,
    );
    assert_eq!(
        app.final_cover_spread_preference,
        crate::settings::FinalCoverSpreadPreference::Off
    );
    let preferences = crate::settings::PageAlonePreferences {
        after_cover: crate::settings::PageAlonePreference::On,
        last: crate::settings::PageAlonePreference::On,
    };
    app.set_page_alone_preferences_for_fullscreen(&ctx, preferences);
    assert_eq!(app.page_alone_preferences, preferences);
    assert_eq!(
        app.build_spread_display_units_for_nav(&[0, 1, 2, 3])
            .iter()
            .map(|u| u.pages.clone())
            .collect::<Vec<_>>(),
        vec![vec![0], vec![1], vec![2], vec![3]]
    );
    let endpoints = crate::settings::SingletonSpreadEndpointPreferences {
        first: crate::settings::SingletonSpreadPlacementPreference::Place,
        last: crate::settings::SingletonSpreadPlacementPreference::Place,
    };
    app.set_singleton_spread_endpoint_preferences_for_fullscreen(&ctx, endpoints);
    assert_eq!(app.singleton_spread_endpoint_preferences, endpoints);
    assert_eq!(app.composed_book_layout().unwrap(), &before);
    assert_eq!(app.composed_layout_token(), token);
    assert_eq!(app.fullscreen_idx, Some(1));
    let key = app.spread_container_key_with_fallback().unwrap();
    let db = app.spread_db.as_ref().unwrap();
    assert_eq!(
        db.get_final_cover_spread_preference(&key.exact),
        Some(crate::settings::FinalCoverSpreadPreference::Off)
    );
    assert_eq!(
        db.get_page_alone_preferences_with_fallback(&key.exact, None)
            .unwrap(),
        preferences
    );
}

#[test]
fn temporary_viewing_ignore_does_not_bypass_selected_output_visibility() {
    let mut book = fixtures::four_page_book(CompositionDirection::Ltr);
    let mut app = composed_app(&book);
    let data = tempfile::tempdir().unwrap();
    let (service, owner) =
        BookCompositionService::spawn_at(data.path().to_path_buf(), None).unwrap();
    let scope = crate::book_scope::BookScopeKey::document("c:/g3-display.pdf").unwrap();
    book.hidden_pages.push(fixtures::document_page(1));
    service
        .save(scope.clone(), PageDomain::Document, book)
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    app.install_book_composition_runtime(service);
    app.book_composition = crate::app::book_composition_runtime::ViewerBookComposition::Ignored {
        scope,
        source: crate::app::book_composition_runtime::BookCompositionSource::Unavailable,
        generation: app.items_generation,
    };
    assert!(app.presentation_candidate(1).is_none());
    let target = crate::ui_dialogs::export_batch::ExportBatchTarget::Item {
        idx: 1,
        page_key: app.page_path_key(1).unwrap(),
    };
    let item = app.batch_export_item_for_target(&target).unwrap().unwrap();
    assert!(!item.composition_check.unwrap().permits().unwrap());
    owner.shutdown_and_join();
}

#[test]
fn book_composition_alone_uses_first_preference_everywhere_and_keeps_landscape_centered() {
    use crate::settings::{
        SingletonSpreadEndpointPreferences, SingletonSpreadPlacementPreference as Pref,
    };
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        let side = if direction == CompositionDirection::Ltr {
            SingletonSpreadPlacement::Right
        } else {
            SingletonSpreadPlacement::Left
        };
        for cover in [false, true] {
            for at in [0, 1, 3] {
                let mut app =
                    composed_app(&specified_book(direction, PageOverride::Alone, at, cover));
                app.settings.final_cover_spread_enabled = false;
                for landscape in [false, true] {
                    app.rotation_cache
                        .insert(at as usize, crate::rotation_db::Rotation::None);
                    app.record_page_dims_for_spread(
                        at as usize,
                        if landscape { (1440, 960) } else { (960, 1440) },
                    );
                    for flow in [ReadingFlow::Paged, ReadingFlow::Vertical] {
                        app.reading_flow = flow;
                        for animation in [false, true] {
                            app.settings.page_turn_animation_enabled = animation;
                            for global in [false, true] {
                                app.settings.singleton_spread_first_enabled = global;
                                // Contradict the first setting to prove tail Alone
                                // does not accidentally inherit the last endpoint.
                                app.settings.singleton_spread_last_enabled = !global;
                                for preference in [Pref::FollowGlobal, Pref::Place, Pref::Center] {
                                    let saved = SingletonSpreadEndpointPreferences {
                                        first: preference,
                                        last: Pref::Place,
                                    };
                                    app.singleton_spread_endpoint_preferences = saved;
                                    let composition =
                                        app.spread_display_composition_for_anchor(at as usize);
                                    let expected = if !landscape
                                        && ((animation && flow.is_paged())
                                            || preference.effective(global))
                                    {
                                        side
                                    } else {
                                        SingletonSpreadPlacement::Center
                                    };
                                    assert_eq!(
                                        composition.singleton_placement(),
                                        expected,
                                        "{direction:?}/{cover}/{at}/{landscape}/{flow:?}/{animation}/{global}/{preference:?}"
                                    );
                                    assert_eq!(composition.navigation_pages(), vec![at as usize]);
                                    assert_eq!(composition.pages_in_reading_order().len(), 1);
                                    assert_eq!(app.singleton_spread_endpoint_preferences, saved);
                                    assert_eq!(app.settings.singleton_spread_first_enabled, global);
                                    assert_eq!(app.settings.singleton_spread_last_enabled, !global);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn book_composition_facet_excluded_alone_keeps_first_policy_and_prepared_rotation() {
    use crate::rotation_db::Rotation;
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        let mut app = composed_app(&specified_book(direction, PageOverride::Alone, 1, true));
        // This pure fixture bypasses the projection worker. Supply the same
        // complete identity-keyed rotation input that worker adoption installs.
        let rotations = (0..app.items.len())
            .map(|idx| (app.page_path_key(idx).unwrap(), Rotation::None))
            .collect();
        let crate::app::book_composition_runtime::ViewerBookComposition::Accepted {
            layout: CompositionProjectionOutcome::Composed(projection),
            ..
        } = &mut app.book_composition
        else {
            panic!("composed fixture");
        };
        Arc::make_mut(projection).prepared_rotations = Arc::new(rotations);
        app.settings.singleton_spread_first_enabled = false;
        app.settings.singleton_spread_last_enabled = true;
        app.settings.final_cover_spread_enabled = true;
        app.settings.page_turn_animation_enabled = true;
        app.record_page_dims_for_spread(1, (960, 1440));
        app.visible_indices = vec![0, 2, 3];
        app.details_order = app.visible_indices.clone();
        app.viewer_navigation_caches.invalidate();
        let side = if direction == CompositionDirection::Ltr {
            SingletonSpreadPlacement::Right
        } else {
            SingletonSpreadPlacement::Left
        };
        let outside = app.spread_display_composition_for_anchor(1);
        assert_eq!(outside.singleton_placement(), side);
        assert_eq!(outside.navigation_pages(), vec![1]);
        assert_eq!(
            outside.pages_in_reading_order().len(),
            1,
            "no excluded cover or neighbor"
        );
        app.settings.page_turn_animation_enabled = false;
        assert_eq!(
            app.spread_display_composition_for_anchor(1)
                .singleton_placement(),
            SingletonSpreadPlacement::Center
        );
        app.settings.page_turn_animation_enabled = true;
        // Cold index memo must still use the worker-prepared rotation of the
        // shown page. There is no UI rotation DB or neighbor demand here.
        app.update_prepared_composed_rotation(1, Rotation::Cw90);
        assert_eq!(app.prepared_composed_rotation(1), Some(Rotation::Cw90));
        app.rotation_cache.clear();
        assert_eq!(
            app.spread_display_composition_for_anchor(1)
                .singleton_placement(),
            SingletonSpreadPlacement::Center
        );
        app.reading_flow = ReadingFlow::Vertical;
        app.settings.singleton_spread_first_enabled = true;
        app.fullscreen_idx = Some(1);
        let (units, at) = app.continuous_reading_units_and_pos(1).unwrap();
        assert_eq!(units[at].anchor_idx, 1);
        assert_eq!(
            units[at].singleton_placement,
            SingletonSpreadPlacement::Center
        );
        assert_eq!(units[at].presentation.len(), 1);
    }
}

#[test]
fn book_composition_legacy_spread_keys_match_binding_and_preserve_record() {
    let _serial = crate::key_input::lock_test_input();
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        for cover in [false, true] {
            let mut book = fixtures::four_page_book(direction);
            book.cover_enabled = cover;
            let expected = composition_spread_mode(direction, cover);
            for (key, requested) in [
                (egui::Key::Num2, SpreadMode::Ltr),
                (egui::Key::Num3, SpreadMode::LtrCover),
                (egui::Key::Num4, SpreadMode::Rtl),
                (egui::Key::Num5, SpreadMode::RtlCover),
            ] {
                let ctx = egui::Context::default();
                let mut app = composed_app(&book);
                app.fullscreen_idx = Some(0);
                let data = tempfile::tempdir().unwrap();
                let (service, owner) =
                    BookCompositionService::spawn_at(data.path().to_path_buf(), None).unwrap();
                let scope = crate::book_scope::BookScopeKey::document("c:/g3-display.pdf").unwrap();
                service
                    .save(scope.clone(), PageDomain::Document, book.clone())
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .unwrap()
                    .status
                    .unwrap();
                app.install_book_composition_runtime(service.clone());
                let token = app.composed_layout_token();
                let record = app.composed_book_layout().unwrap().clone();
                app.apply_fullscreen_view_mode(&ctx, 0, FullscreenViewMode::Single);
                app.spread_shift.state = crate::app::spread_shift_owner::ViewerShift::Ordinary(2);
                let revision_before = app.book_append_undo_revision;
                let _ = ctx.run(
                    egui::RawInput {
                        focused: true,
                        events: vec![egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }],
                        ..Default::default()
                    },
                    |ctx| {
                        app.handle_fs_key_input(ctx, 0, false);
                    },
                );
                assert_eq!(
                    app.spread_mode,
                    if requested == expected {
                        expected
                    } else {
                        SpreadMode::Single
                    },
                    "{direction:?}/{cover}/{key:?}"
                );
                assert_eq!(app.reading_direction, composition_view_direction(direction));
                assert_eq!(app.composed_layout_token(), token);
                assert_eq!(app.composed_book_layout().unwrap(), &record);
                assert!(matches!(
                    app.spread_shift.state,
                    crate::app::spread_shift_owner::ViewerShift::Ordinary(2)
                ));
                if requested != expected {
                    assert_eq!(app.book_append_undo_revision, revision_before);
                    let toast = &app.fs_feedback_toast.as_ref().unwrap().0;
                    let label = spread_mode_key_action(expected)
                        .and_then(|a| app.keymap.first_chord_label(a))
                        .unwrap();
                    assert!(toast.contains(&format!("[{label}]")), "{toast}");
                    assert!(
                        toast.contains("\u{672c}\u{306e}\u{69cb}\u{6210}"),
                        "editor guidance: {toast}"
                    );
                } else {
                    let after = app.book_append_undo_revision;
                    assert!(app.apply_fullscreen_spread_mode(&ctx, 0, expected));
                    assert_eq!(
                        app.book_append_undo_revision, after,
                        "matching no-op must not admit a followup mutation"
                    );
                    assert!(matches!(
                        app.spread_shift.state,
                        crate::app::spread_shift_owner::ViewerShift::Ordinary(2)
                    ));
                }
                owner.shutdown_and_join();
                let db = crate::spread_db::SpreadDb::open_existing_read_only_at(
                    &data.path().join("spread.db"),
                )
                .unwrap()
                .unwrap();
                let saved = db.get_state_with_fallback(
                    &crate::spread_db::container_key(&scope.logical_root, &[]),
                    None,
                );
                assert_eq!(
                    saved.view_mode,
                    Some(if requested == expected {
                        crate::spread_db::StoredViewMode::Spread
                    } else {
                        crate::spread_db::StoredViewMode::Single
                    })
                );
                assert_eq!(saved.mode, None, "mode-only has no new binding");
                assert_eq!(saved.direction, None);
                assert_eq!(saved.flow, None);
            }
        }
    }
}

#[test]
fn book_composition_spread_guidance_and_hud_respect_keymap_and_split_action() {
    let ctx = egui::Context::default();
    let mut book = fixtures::four_page_book(CompositionDirection::Rtl);
    book.cover_enabled = true;
    let mut app = composed_app(&book);
    app.fullscreen_idx = Some(0);
    for (ini, label) in [
        ("FsSpreadRtlCover=Ctrl+K", Some("Ctrl+K")),
        ("FsSpreadRtlCover=none\nFsViewSpread=S", Some("S")),
        ("FsSpreadRtlCover=none", None),
    ] {
        app.keymap = Keymap::from_ini_str(ini);
        assert!(!app.apply_fullscreen_spread_mode(&ctx, 0, SpreadMode::Ltr));
        let toast = &app.fs_feedback_toast.as_ref().unwrap().0;
        if let Some(label) = label {
            assert!(toast.contains(&format!("[{label}]")), "{toast}");
        } else {
            assert!(
                !toast.contains("[5]"),
                "unassigned shortcut must not be advertised: {toast}"
            );
        }
        assert_eq!(
            spread_mode_row_shortcut(
                &app.keymap,
                SpreadMode::RtlCover,
                true,
                Some(SpreadMode::RtlCover)
            )
            .as_deref(),
            label
        );
        assert_eq!(
            spread_mode_row_shortcut(
                &app.keymap,
                SpreadMode::SplitRtl,
                true,
                Some(SpreadMode::RtlCover)
            ),
            None
        );
    }
    app.keymap = Keymap::from_ini_str("FsViewLandscapeSplit=Ctrl+L");
    assert_eq!(
        spread_mode_row_shortcut(
            &app.keymap,
            SpreadMode::SplitRtl,
            true,
            Some(SpreadMode::RtlCover)
        )
        .as_deref(),
        Some("Ctrl+L")
    );
}

#[test]
fn book_composition_boundary_remainder_at_forced_last_keeps_last_preference() {
    for mode in [SpreadMode::LtrCover, SpreadMode::RtlCover] {
        for directive in [
            PageOverride::Alone,
            PageOverride::BlankOpposite {
                page_side: PageSide::Leading,
            },
        ] {
            let overrides = HashMap::from([(4, directive)]);
            let units = build_spread_display_units_with_overrides(
                &[0, 1, 2, 3, 4],
                mode,
                None,
                PageAloneSettings {
                    after_cover: false,
                    last: true,
                },
                &overrides,
                |_, _| false,
                |_| true,
            );
            let at = units.iter().position(|u| u.pages == [3]).unwrap();
            assert_eq!(
                units[at].formation,
                SpreadDisplayUnitFormation::Singleton(
                    SpreadDisplaySingletonCause::OverrideBoundaryRemainder
                )
            );
            let p = resolve_spread_display_composition(
                &units,
                at,
                mode,
                true,
                SingletonSpreadEndpointSettings {
                    first: false,
                    last: true,
                },
                true,
            )
            .unwrap();
            assert_eq!(
                p.singleton_placement(),
                if mode.is_rtl() {
                    SingletonSpreadPlacement::Right
                } else {
                    SingletonSpreadPlacement::Left
                }
            );
            assert_eq!(p.pages_in_reading_order().len(), 1);
        }
    }
}

#[test]
fn book_composition_empty_override_boundaries_keep_automatic_placement() {
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        for kind in 0..3 {
            let units = build_spread_display_units_with_overrides(
                &[0, 1, 2, 3, 4],
                mode,
                (kind == 2).then_some(3),
                PageAloneSettings::default(),
                &HashMap::new(),
                |_, idx| kind == 0 && idx == 3,
                |idx| kind != 1 || idx != 3,
            );
            let at = units.iter().position(|u| u.pages == [2]).unwrap();
            assert_eq!(
                units[at].formation,
                SpreadDisplayUnitFormation::Singleton(match kind {
                    0 => SpreadDisplaySingletonCause::LandscapeBoundary,
                    1 => SpreadDisplaySingletonCause::NonPairableBoundary,
                    _ => SpreadDisplaySingletonCause::UnpairedSlot,
                })
            );
            let p = resolve_spread_display_composition(
                &units,
                at,
                mode,
                false,
                SingletonSpreadEndpointSettings {
                    first: true,
                    last: true,
                },
                true,
            )
            .unwrap();
            assert_eq!(p.singleton_placement(), SingletonSpreadPlacement::Center);
        }
    }
}
