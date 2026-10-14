//! Composition permission, explicit units and slideshow ownership across the master merge.
#![cfg(all(test, windows))]

use super::*;
use crate::book_composition::{
    BookComposition, BookCompositionService, CanonicalRawPage, CompositionProjectionOutcome,
    CompositionToken, PageDomain, ReadingDirection as CompositionDirection, SingleCompanion,
    SinglePlacement, fixtures, project_composition,
};
use crate::ui_fullscreen::page_turn_submission_tests::{
    adopt_interrupted_target, draw_current, frame, ready_app, seed_leaf,
};

fn composed_app(
    ctx: &egui::Context,
    book: &BookComposition,
    count: u32,
) -> crate::app::AppTestEnvForTest {
    let path = PathBuf::from("c:/page-turn-merge.pdf");
    let scope = crate::book_scope::BookScopeKey::document(&path).unwrap();
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
        panic!("fixture must install the actual resolved composition");
    };
    let mut app = ready_app(ctx, SpreadMode::Ltr);
    app.items = prepared
        .output_to_input
        .iter()
        .map(|&page| crate::app::book_materialization::document_page_item(&path, page as u32))
        .collect();
    app.current_folder = Some(path.clone());
    app.projection_recipe = crate::app::book_materialization::ProjectionRecipe::Book(
        crate::app::book_materialization::BookScope::Document(path),
    );
    app.items_generation = 42;
    app.visible_indices = (0..app.items.len()).collect();
    app.details_order = app.visible_indices.clone();
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    app.image_metas = vec![None; app.items.len()];
    app.viewer_navigation_caches.invalidate();
    app.reading_direction = composition_view_direction(book.direction);
    app.spread_mode = SpreadMode::Ltr.with_reading_direction(app.reading_direction);
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.install_composition_for_test(projection);
    app
}

fn capture_composed_flat(ctx: &egui::Context, app: &mut App, anchor: usize) -> Arc<PaintedUnit> {
    app.fullscreen_idx = Some(anchor);
    frame(ctx, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        let (composition, sources) = draw_current(app, ui, ctx);
        let decision = app.fs_page_turn_decision_for_frame(ctx, anchor);
        app.emit_fs_page_turn_ready_for_display_unit(ctx, &composition, decision, &sources);
    });
    let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
        panic!("explicit unit must reach the ordinary flat receipt at {anchor}");
    };
    assert!(unit.evidence.was_accepted());
    unit.clone()
}

#[test]
fn page_turn_merge_master_explicit_singletons_project_without_automatic_cover_proof() {
    use crate::page_turn_animation::project_prepared;

    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        for placement in [
            SinglePlacement::Center,
            SinglePlacement::Leading,
            SinglePlacement::Trailing,
        ] {
            for cover in [0, 3] {
                let mut book = BookComposition::new(
                    direction,
                    vec![
                        fixtures::single(0, placement, SingleCompanion::Vacant),
                        fixtures::pair(1, 2),
                        fixtures::single(3, SinglePlacement::Center, SingleCompanion::Vacant),
                    ],
                );
                book.cover = Some(fixtures::document_page(cover));
                let ctx = egui::Context::default();
                install(&ctx);
                let mut app = composed_app(&ctx, &book, 4);
                app.spread_mode = if direction == CompositionDirection::Ltr {
                    SpreadMode::LtrCover
                } else {
                    SpreadMode::RtlCover
                };
                let first = capture_composed_flat(&ctx, &mut app, 0);
                let pair = capture_composed_flat(&ctx, &mut app, 1);
                assert_eq!(
                    first.layout[0].formation,
                    SpreadDisplayUnitFormation::Singleton(
                        SpreadDisplaySingletonCause::ExplicitComposition,
                    )
                );
                let canonical = canonical_unit(&first).unwrap_or_else(|reason| {
                    panic!("{direction:?}/{placement:?}/cover={cover}: {reason}")
                });
                let actual = canonical
                    .pages
                    .iter()
                    .find(|page| page.occurrence.is_some())
                    .unwrap();
                assert_eq!(
                    actual.rect, first.pages[0].transform.paint_rect,
                    "saved placement is authoritative, including Center"
                );
                for (outgoing, incoming, forward) in [(&first, &pair, true), (&pair, &first, false)]
                {
                    let leaf = prepare_leaf(outgoing, incoming, forward).unwrap_or_else(|reason| {
                        panic!(
                            "{direction:?}/{placement:?}/cover={cover}/forward={forward}: {reason}"
                        )
                    });
                    for progress in [0.0, 0.25, 0.6, 1.0] {
                        let projection = project_prepared(
                            &leaf.outgoing,
                            &leaf.incoming,
                            leaf.direction,
                            leaf.recipe,
                            progress,
                            false,
                        )
                        .unwrap();
                        assert!(!projection.meshes.is_empty());
                        assert_eq!(projection.finished, progress == 1.0);
                    }
                }
            }
        }
    }
}

#[test]
fn page_turn_merge_master_cached_incoming_requires_every_presentation_permission() {
    let _serial = crate::key_input::lock_test_input();
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        for single in [false, true] {
            for status in ["hidden", "partner_hidden", "checking"] {
                if single && status == "partner_hidden" {
                    continue;
                }
                take_test_events();
                let ctx = egui::Context::default();
                install(&ctx);
                let mut app = composed_app(&ctx, &fixtures::four_page_book(direction), 4);
                if single {
                    app.spread_mode = SpreadMode::Single;
                }
                let data = tempfile::tempdir().unwrap();
                let (service, owner) =
                    BookCompositionService::spawn_at(data.path().to_path_buf(), None).unwrap();
                app.install_book_composition_runtime(service);
                let outgoing = app.book_target_presentation_demand(0);
                app.install_allowed_presentation_for_test(&outgoing);
                frame(&ctx, |ui, ctx| {
                    draw_current(&mut app, ui, ctx);
                });
                assert!(matches!(
                    app.page_turn_presentation.state,
                    PresentationState::Flat(_)
                ));

                let target = if single { 1 } else { 2 };
                let incoming = app.book_target_presentation_demand(target);
                assert_eq!(incoming.len(), if single { 1 } else { 2 });
                assert!(
                    incoming
                        .iter()
                        .all(|page| app.fs_cache.contains_key(&page.item_index))
                );
                app.install_allowed_presentation_for_test(&incoming);
                let held = if status == "checking" {
                    app.install_checking_presentation_for_test(&incoming)
                } else {
                    let hidden = if status == "partner_hidden" {
                        &incoming[..1]
                    } else {
                        &incoming[..]
                    };
                    app.install_hidden_presentation_for_test(hidden);
                    Vec::new()
                };
                let operation = ViewerOperationId::Pad {
                    batch: next_operation_batch(),
                    ordinal: 0,
                };
                frame(&ctx, |_, ctx| {
                    app.page_turn_external_operation(
                        ctx,
                        operation,
                        OperationPhase::Press,
                        "merge",
                    );
                    app.offer_page_turn_with_operation(ctx, target, true, Some(operation));
                });
                assert!(matches!(
                    app.page_turn_presentation.state,
                    PresentationState::Flat(_)
                ));
                let events = take_test_events();
                assert_eq!(
                    events
                        .iter()
                        .filter(|(fact, kind, reason)| {
                            fact.operation_id == operation
                                && *kind == "skip"
                                && *reason == "book_presentation_not_fully_allowed"
                        })
                        .count(),
                    1,
                    "{direction:?}/{single}/{status}: {events:?}"
                );
                assert!(!events.iter().any(|(_, kind, _)| *kind == "offered"));

                app.install_allowed_presentation_for_test(&incoming);
                drop(held);
                frame(&ctx, |ui, ctx| {
                    app.begin_page_turn_viewer_frame(ctx, false, None);
                    draw_current(&mut app, ui, ctx);
                });
                assert!(matches!(
                    app.page_turn_presentation.state,
                    PresentationState::Flat(_)
                ));
                assert!(
                    !take_test_events()
                        .iter()
                        .any(|(_, kind, _)| *kind == "offered")
                );
                let fresh = ViewerOperationId::Pad {
                    batch: next_operation_batch(),
                    ordinal: 0,
                };
                frame(&ctx, |_, ctx| {
                    app.page_turn_external_operation(ctx, fresh, OperationPhase::Press, "merge");
                    app.offer_page_turn_with_operation(ctx, target, true, Some(fresh));
                });
                assert!(matches!(
                    app.page_turn_presentation.state,
                    PresentationState::Offered { .. }
                ));
                assert!(
                    take_test_events()
                        .iter()
                        .any(|(fact, kind, _)| fact.operation_id == fresh && *kind == "offered")
                );
                owner.shutdown_and_join();
            }
        }
    }
}

#[test]
fn page_turn_merge_master_explicit_topology_controls_presentation_and_prefetch() {
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        for companion in [
            SingleCompanion::Vacant,
            SingleCompanion::Blank,
            SingleCompanion::FrontCoverSupplement {
                page: fixtures::document_page(0),
            },
        ] {
            let mut units = vec![fixtures::single(
                0,
                SinglePlacement::Center,
                SingleCompanion::Vacant,
            )];
            units.extend(
                (1..11)
                    .step_by(2)
                    .map(|page| fixtures::pair(page, page + 1)),
            );
            units.push(fixtures::single(
                11,
                SinglePlacement::Trailing,
                companion.clone(),
            ));
            let mut book = BookComposition::new(direction, units);
            book.cover = Some(fixtures::document_page(0));
            let ctx = egui::Context::default();
            let mut app = composed_app(&ctx, &book, 12);
            app.spread_mode = if direction == CompositionDirection::Ltr {
                SpreadMode::LtrCover
            } else {
                SpreadMode::RtlCover
            };
            // Automatic endpoint/cover preferences cannot rewrite explicit units.
            app.settings.final_cover_spread_enabled = true;
            app.settings.last_page_alone_enabled = true;
            app.settings.page_after_cover_alone_enabled = true;
            let nav = app.get_nav_indices();
            let cached = app.build_spread_display_units_for_nav(&nav);
            assert_eq!(cached.len(), 7);
            assert_eq!(cached[5].pages, vec![9, 10]);
            assert_eq!(cached[6].pages, vec![11]);
            let pair = app.spread_display_composition_for_anchor(9);
            let screen = pair.pages_in_screen_order(app.spread_mode);
            assert_eq!(
                screen.iter().map(|page| page.idx).collect::<Vec<_>>(),
                if direction == CompositionDirection::Ltr {
                    vec![9, 10]
                } else {
                    vec![10, 9]
                }
            );
            assert_eq!(
                pair.presentation_demand(app.spread_mode, crate::page_split::PageSlice::Full)
                    .len(),
                2
            );

            // An explicit unit owns its pair and placement even when geometry
            // arrives late and animation forces Automatic endpoint preferences.
            app.record_page_dims_for_spread(9, (1700, 1200));
            app.rotation_cache
                .insert(9, crate::rotation_db::Rotation::Cw90);
            app.reconcile_spread_landscapes_from_rotation_cache();
            for animation in [true, false] {
                app.settings.page_turn_animation_enabled = animation;
                assert_eq!(app.spread_display_composition_for_anchor(9), pair);
                assert_eq!(
                    app.spread_display_composition_for_anchor(0)
                        .singleton_placement(),
                    SingletonSpreadPlacement::Center
                );
            }
            app.settings.page_turn_animation_enabled = true;

            let tail = app.spread_display_composition_for_anchor(11);
            let supplemented = matches!(companion, SingleCompanion::FrontCoverSupplement { .. });
            assert_eq!(tail.navigation_pages(), vec![11]);
            assert_eq!(
                tail.singleton_placement().has_white_companion(),
                matches!(companion, SingleCompanion::Blank)
            );
            assert_eq!(
                tail.pages_in_reading_order().len(),
                if supplemented { 2 } else { 1 }
            );
            if supplemented {
                assert_eq!(
                    tail.pages_in_reading_order()[1],
                    SpreadPageOccurrence::final_cover_supplement(0, 11)
                );
            }
            let demand =
                tail.presentation_demand(app.spread_mode, crate::page_split::PageSlice::Full);
            assert_eq!(
                demand.len(),
                if supplemented { 2 } else { 1 },
                "blank has no permission/source demand"
            );
            let plan = app.page_turn_prefetch_targets(9, 1, 0).unwrap();
            assert_eq!(plan.current_position, 5);
            assert_eq!(
                plan.targets,
                if supplemented {
                    vec![(5, 10), (6, 11), (6, 0)]
                } else {
                    vec![(5, 10), (6, 11)]
                }
            );

            app.spread_mode = SpreadMode::Single;
            let singles = app.build_spread_display_units_for_nav(&nav);
            assert_eq!(singles.len(), 12);
            assert!(
                singles
                    .iter()
                    .enumerate()
                    .all(|(idx, unit)| unit.pages == vec![idx])
            );
            for idx in [0, 9, 10, 11] {
                let single = app.spread_display_composition_for_anchor(idx);
                assert_eq!(single.navigation_pages(), vec![idx]);
                assert_eq!(
                    single.pages_in_reading_order(),
                    &[SpreadPageOccurrence::navigation(idx, idx)]
                );
                assert_eq!(
                    single
                        .presentation_demand(app.spread_mode, crate::page_split::PageSlice::Full)
                        .len(),
                    1
                );
                assert!(!single.singleton_placement().has_white_companion());
            }
            assert_eq!(
                app.page_turn_prefetch_targets(9, 1, 0).unwrap().targets,
                vec![(10, 10)]
            );
        }
    }
}

#[test]
fn page_turn_merge_master_slideshow_barrier_commits_grid_and_deadline_once() {
    use crate::app::book_composition_permission::BookTargetPurpose;

    let _serial = crate::key_input::lock_test_input();
    crate::key_input::clear_test_frame();
    for purpose in [BookTargetPurpose::Next, BookTargetPurpose::FirstInBook] {
        let ctx = egui::Context::default();
        let mut app = ready_app(&ctx, SpreadMode::Single);
        seed_leaf(&ctx, &mut app);
        app.move_grid_cursor(Some(1), crate::app::GridCursorIntent::Automatic);
        app.scroll_to_selected = false;
        app.slideshow_playing = true;
        app.settings.slideshow_interval_secs = 3.0;
        app.slideshow_anchor_idx = None;
        app.slideshow_next_at = Instant::now() - Duration::from_secs(1);
        let expired = app.slideshow_next_at;
        take_test_events();
        frame(&ctx, |_, ctx| {
            app.accept_book_slideshow_target(ctx, 2, None, purpose);
            assert_eq!(app.fullscreen_idx, Some(1));
            assert!(app.page_turn_completion_active());
            assert_eq!(
                app.page_turn_presentation
                    .continuation_slideshow_purpose_for_test(),
                Some(purpose)
            );
            assert_eq!(app.selected, Some(1));
            assert!(!app.scroll_to_selected);
            assert_eq!(app.slideshow_next_at, expired);
        });
        adopt_interrupted_target(&ctx, &mut app);
        assert_eq!(app.slideshow_next_at, expired);
        let before = Instant::now();
        frame(&ctx, |_, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
        });
        let after = Instant::now();
        assert_eq!(app.fullscreen_idx, Some(2));
        assert_eq!(app.selected, Some(2));
        assert_eq!(
            app.grid_click_selection_anchor
                .and_then(|anchor| anchor.index_for_generation(app.items_generation)),
            Some(2)
        );
        assert!(app.scroll_to_selected);
        assert_eq!(app.slideshow_anchor_idx, Some(2));
        assert!(app.slideshow_next_at >= before + Duration::from_secs(3));
        assert!(app.slideshow_next_at <= after + Duration::from_secs(3));
        let deadline = app.slideshow_next_at;
        frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            let state = app.prepare_fullscreen_state(ctx, 2, FsPageTurnDecision::normal());
            draw_current(&mut app, ui, ctx);
            app.sync_slideshow_anchor_for_frame(ctx, 2, &state);
        });
        assert_eq!(app.slideshow_next_at, deadline);
        assert!(
            !take_test_events()
                .iter()
                .any(|(_, kind, _)| *kind == "offered")
        );
    }
}
