use super::*;
use crate::book_composition::ReadingDirection as CompositionDirection;
use crate::book_composition::*;
use crate::settings::ReadingDirection;

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
    app.spread_mode = SpreadMode::Ltr.with_reading_direction(app.reading_direction);
    app.install_composition_for_test(projection);
    app
}

#[test]
fn composed_cache_never_observes_dormant_shift_or_late_landscape() {
    let mut app = composed_app(&fixtures::four_page_book(CompositionDirection::Ltr));
    let nav = app.get_nav_indices();
    let original = app.build_spread_display_units_for_nav(&nav);
    assert_eq!(
        original
            .iter()
            .map(|unit| unit.pages.clone())
            .collect::<Vec<_>>(),
        vec![vec![0, 1], vec![2, 3]]
    );
    app.rotation_cache
        .insert(1, crate::rotation_db::Rotation::Cw90);
    app.record_page_dims_for_spread(1, (1000, 2000));
    app.spread_shift = crate::spread_shift::contract_fixtures::four_page_book("c:/dormant")
        .live_owner(1, crate::spread_shift::ShiftPersistence::Saved(91));
    app.settings.page_after_cover_alone_enabled = true;
    app.settings.last_page_alone_enabled = true;
    app.settings.final_cover_spread_enabled = true;
    let still = app.build_spread_display_units_for_nav(&nav);
    assert!(Arc::ptr_eq(&original, &still));
    assert_eq!(
        app.resolve_spread_pair(0),
        SpreadPair::Double { left: 0, right: 1 }
    );
    let (continuous, _) = app.continuous_reading_units_and_pos(0).unwrap();
    assert_eq!(
        continuous[0]
            .presentation
            .iter()
            .map(|page| page.idx)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
}

#[test]
fn composed_blank_trim_uses_physical_slot_and_single_projection_uses_single_margin() {
    use crate::view_trim::{ViewTrimApplyMode, ViewTrimMargins, ViewTrimSpreadSide};
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        let fixture = fixtures::editor_unit_edits(direction);
        let mut app = composed_app(&fixture.blank);
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
fn legacy_binding_same_value_refuses_before_state_and_single_split_nudge_stays_navigation() {
    let ctx = egui::Context::default();
    let mut app = composed_app(&fixtures::four_page_book(CompositionDirection::Ltr));
    app.fullscreen_idx = Some(0);
    let settings = serde_json::to_value(&app.settings).unwrap();
    let units = app.build_spread_display_units_for_nav(&[0, 1, 2, 3]);
    app.apply_fullscreen_spread_mode(&ctx, 0, SpreadMode::Ltr);
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
fn explicit_units_share_paged_continuous_roles_slots_and_real_navigation() {
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        let fixture = fixtures::editor_unit_edits(direction);
        for book in [
            &fixture.original,
            &fixture.blank,
            &fixture.with_cover_helper,
            &fixture.hidden,
        ] {
            let layout = resolved(book, 4);
            let mut actual_navigation = Vec::new();
            for unit in &layout.units {
                let paged = SpreadDisplayComposition::from_composed_unit(unit, direction);
                let continuous =
                    ContinuousReadingUnitSpec::composition(paged.clone(), SpreadMode::LtrCover);
                assert_eq!(
                    paged.pages_in_screen_order(SpreadMode::LtrCover),
                    continuous.presentation
                );
                assert_eq!(paged.singleton_placement(), continuous.singleton_placement);
                actual_navigation.extend(paged.navigation_pages());
                assert_eq!(
                    paged.presentation_demand(SpreadMode::Rtl, crate::page_split::PageSlice::Full),
                    continuous.presentation_demand()
                );
            }
            assert_eq!(
                actual_navigation,
                layout
                    .visible_pages
                    .iter()
                    .map(|page| page.item_index)
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn explicit_pair_direction_is_frozen_and_missing_keeps_original_slot() {
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        let book = fixtures::four_page_book(direction);
        let layout = resolved(&book, 4);
        let pair = SpreadDisplayComposition::from_composed_unit(&layout.units[0], direction);
        for mode in [
            SpreadMode::Ltr,
            SpreadMode::Rtl,
            SpreadMode::LtrCover,
            SpreadMode::RtlCover,
        ] {
            let screen = pair.pages_in_screen_order(mode);
            assert_eq!(
                screen.iter().map(|page| page.idx).collect::<Vec<_>>(),
                if direction == CompositionDirection::Ltr {
                    vec![0, 1]
                } else {
                    vec![1, 0]
                }
            );
        }
        let mut manifest = fixtures::document_manifest(4);
        manifest[1].availability = ManifestAvailability::FilteredOut;
        let missing = resolve_composition(&book, &PageDomain::Document, &manifest).unwrap();
        let single = SpreadDisplayComposition::from_composed_unit(&missing.units[0], direction);
        assert_eq!(single.navigation_pages(), vec![0]);
        assert_eq!(
            single.singleton_placement(),
            if direction == CompositionDirection::Ltr {
                SingletonSpreadPlacement::Left
            } else {
                SingletonSpreadPlacement::Right
            }
        );
        assert_eq!(
            missing.units[1]
                .navigation_pages()
                .map(|page| page.item_index)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
    }
}

#[test]
fn hidden_permission_partner_keeps_physical_slot_and_helper_has_no_navigation() {
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        let fixture = fixtures::editor_unit_edits(direction);
        let layout = resolved(&fixture.original, 4);
        let pair = SpreadDisplayComposition::from_composed_unit(&layout.units[0], direction);
        for allowed in [vec![true, false], vec![false, true]] {
            let paged = pair
                .permitted_projection(SpreadMode::Ltr, &allowed)
                .unwrap();
            let unit = ContinuousReadingUnitSpec::composition(pair.clone(), SpreadMode::Ltr);
            let continuous = unit.permitted_projection(&allowed).unwrap();
            assert_eq!(paged.singleton_placement(), continuous.singleton_placement);
            assert_eq!(
                paged.pages_in_screen_order(SpreadMode::Ltr),
                continuous.presentation
            );
            assert_eq!(
                paged.singleton_placement(),
                if allowed[0] {
                    SingletonSpreadPlacement::Left
                } else {
                    SingletonSpreadPlacement::Right
                }
            );
        }
        assert!(
            pair.permitted_projection(SpreadMode::Ltr, &[false, false])
                .is_none()
        );
        let helper = resolved(&fixture.with_cover_helper, 4);
        let unit =
            SpreadDisplayComposition::from_composed_unit(helper.units.last().unwrap(), direction);
        assert_eq!(unit.navigation_pages(), vec![3]);
        let screen = unit.pages_in_screen_order(SpreadMode::Ltr);
        let allow_helper_only = screen
            .iter()
            .map(|page| page.role == SpreadPageRole::FrontCoverSupplement)
            .collect::<Vec<_>>();
        assert!(
            unit.permitted_projection(SpreadMode::Ltr, &allow_helper_only)
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
                                UnitEdit::SetHidden {
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
fn composed_facet_masks_original_slots_without_repairing_units_or_helpers() {
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        let mut app = composed_app(&fixtures::four_page_book(direction));
        app.visible_indices = vec![0, 2, 3];
        app.details_order = app.visible_indices.clone();
        app.viewer_navigation_caches.invalidate();
        app.fullscreen_idx = Some(0);
        let paged = app.spread_display_composition_for_anchor(0);
        assert_eq!(paged.navigation_pages(), vec![0]);
        assert_eq!(
            paged.singleton_placement(),
            if direction == CompositionDirection::Ltr {
                SingletonSpreadPlacement::Left
            } else {
                SingletonSpreadPlacement::Right
            }
        );
        app.reading_flow = ReadingFlow::Vertical;
        let (units, _) = app.continuous_reading_units_and_pos(0).unwrap();
        assert_eq!(
            units
                .iter()
                .map(|unit| unit
                    .presentation
                    .iter()
                    .map(|page| page.idx)
                    .collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            vec![
                vec![0],
                if direction == CompositionDirection::Ltr {
                    vec![2, 3]
                } else {
                    vec![3, 2]
                }
            ]
        );
        assert_eq!(units[0].singleton_placement, paged.singleton_placement());
        assert_eq!(
            units[1].anchor_idx, 2,
            "facet masking retains the frozen reading anchor in both directions"
        );
        app.fullscreen_idx = Some(1);
        let outside = app.spread_display_composition_for_anchor(1);
        assert_eq!(
            outside.navigation_pages(),
            vec![1],
            "an already shown facet-excluded anchor stays a qualified singleton"
        );
        assert_eq!(
            outside.singleton_placement(),
            if direction == CompositionDirection::Ltr {
                SingletonSpreadPlacement::Right
            } else {
                SingletonSpreadPlacement::Left
            }
        );
        let (outside_units, pos) = app.continuous_reading_units_and_pos(1).unwrap();
        assert_eq!(
            outside_units[pos].source_indices().collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(
            outside_units[pos].singleton_placement,
            outside.singleton_placement()
        );
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
        app.install_book_composition_runtime(service);
        let held =
            app.install_checking_presentation_for_test(&outside_units[pos].presentation_demand());
        assert!(matches!(
            app.book_presentation_allowed(&outside_units[pos].presentation_demand()),
            PresentationDecision::Checking
        ));
        drop(held);
        owner.shutdown_and_join();
        drop(app);
        let mut helper = composed_app(&fixtures::editor_unit_edits(direction).with_cover_helper);
        helper.visible_indices = vec![1, 2, 3];
        helper.details_order = helper.visible_indices.clone();
        helper.viewer_navigation_caches.invalidate();
        let last = helper.spread_display_composition_for_anchor(3);
        assert_eq!(
            last.pages_in_reading_order()
                .iter()
                .map(|page| page.idx)
                .collect::<Vec<_>>(),
            vec![3]
        );
        helper.reading_flow = ReadingFlow::Vertical;
        let (units, _) = helper.continuous_reading_units_and_pos(3).unwrap();
        assert!(
            units
                .iter()
                .flat_map(|unit| &unit.presentation)
                .all(|page| page.idx != 0)
        );
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
