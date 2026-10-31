//! Composition permission, shared-builder formation and slideshow ownership across the merge.
#![cfg(all(test, windows))]

use super::*;
use crate::book_composition::{
    BookComposition, BookCompositionService, CanonicalRawPage, CompositionProjectionOutcome,
    CompositionToken, PageDomain, PageOverride, PageOverrideEntry, PageSide,
    ReadingDirection as CompositionDirection, fixtures, project_composition,
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
fn page_turn_merge_master_coverless_tail_builder_preserves_vacant_leaf_motion() {
    use crate::page_turn_animation::{PageMaterial, project_prepared};

    let _serial = crate::key_input::lock_test_input();
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        for explicit_alone in [false, true] {
            let mut book =
                BookComposition::new(direction, (0..11).map(fixtures::document_page).collect());
            book.cover_enabled = false;
            if explicit_alone {
                book.page_overrides.push(PageOverrideEntry {
                    page: fixtures::document_page(10),
                    directive: PageOverride::Alone,
                });
            }
            let ctx = egui::Context::default();
            install(&ctx);
            let mut app = composed_app(&ctx, &book, 11);
            app.settings.final_cover_spread_enabled = false;
            app.settings.last_page_alone_enabled = false;
            app.settings.page_after_cover_alone_enabled = false;
            for idx in 0..11 {
                let image = Arc::new(egui::ColorImage::filled([80, 120], egui::Color32::GREEN));
                let tex = ctx.load_texture(
                    format!("coverless-tail-{idx}"),
                    image.clone(),
                    egui::TextureOptions::LINEAR,
                );
                app.fs_cache.insert(
                    idx,
                    FsCacheEntry::Static {
                        pdf_rerender_failures: Vec::new(),
                        pdf_metadata: None,
                        tex,
                        pixels: image,
                        source_dims: Some([80, 120]),
                        load_seq: 1,
                        animation: crate::fs_animation::StaticAnimationState::Still,
                    },
                );
                app.record_page_dims_for_spread(idx, (80, 120));
                app.record_page_layout_dims_for_spread(idx, (80, 120));
                app.rotation_cache
                    .insert(idx, crate::rotation_db::Rotation::None);
            }
            let nav = app.get_nav_indices();
            let units = app.build_spread_display_units_for_nav(&nav);
            assert_eq!(units.len(), 6);
            assert_eq!(units[4].pages, vec![8, 9]);
            assert_eq!(units[4].formation, SpreadDisplayUnitFormation::Paired);
            assert_eq!(units[5].pages, vec![10]);
            assert_eq!(
                units[5].formation,
                SpreadDisplayUnitFormation::Singleton(if explicit_alone {
                    SpreadDisplaySingletonCause::ExplicitAlone
                } else {
                    SpreadDisplaySingletonCause::UnpairedSlot
                })
            );
            // A natural last singleton uses the last endpoint; Alone uses the
            // cover endpoint. Only the latter leaves the arriving leaf vacant.
            let right = (direction == CompositionDirection::Rtl) != explicit_alone;
            assert_eq!(
                app.spread_display_composition_for_anchor(10)
                    .singleton_placement(),
                if right {
                    SingletonSpreadPlacement::Right
                } else {
                    SingletonSpreadPlacement::Left
                }
            );
            let pair = capture_composed_flat(&ctx, &mut app, 8);
            let tail = capture_composed_flat(&ctx, &mut app, 10);
            assert_eq!(pair.pages.len(), 2);
            assert_eq!(tail.pages.len(), 1);
            assert!(tail.white.is_none());
            assert_eq!(
                tail.pages[0].occurrence,
                SpreadPageOccurrence::navigation(10, 10)
            );
            let canonical = canonical_unit(&tail).unwrap();
            let vacant = &canonical.pages[usize::from(!right)];
            assert!(matches!(vacant.material, PageMaterial::Vacant));
            assert!(vacant.occurrence.is_none());
            assert!(vacant.underlay.is_none());
            assert_eq!(canonical.pages[usize::from(right)].occurrence, Some(0));

            for (outgoing, incoming, forward) in [(&pair, &tail, true), (&tail, &pair, false)] {
                let leaf = prepare_leaf(outgoing, incoming, forward).unwrap();
                let incoming_ids = leaf
                    .incoming
                    .pages
                    .iter()
                    .filter_map(|page| page.occurrence)
                    .collect::<Vec<_>>();
                assert_eq!(incoming_ids.len(), incoming.pages.len());
                let mut previous_angle = 0.0;
                for progress in [0.0, 0.25, 0.6, 0.95, 1.0] {
                    let projection = project_prepared(
                        &leaf.outgoing,
                        &leaf.incoming,
                        leaf.direction,
                        leaf.recipe,
                        progress,
                        false,
                    )
                    .unwrap();
                    assert_eq!(projection.finished, progress == 1.0);
                    assert!(projection.meshes.iter().all(|mesh| {
                        mesh.mesh.texture_id == egui::TextureId::default()
                            || outgoing.pages.iter().chain(&incoming.pages).any(|page| {
                                page.resource.paint_texture_id() == mesh.mesh.texture_id
                            })
                    }));
                    assert_eq!(projection.visibility.incoming.len(), incoming_ids.len());
                    assert!(
                        projection
                            .visibility
                            .incoming
                            .iter()
                            .all(|page| incoming_ids.contains(&page.occurrence))
                    );
                    assert!(projection.angle_degrees >= previous_angle);
                    previous_angle = projection.angle_degrees;
                    if progress >= 0.6 {
                        assert!(projection.angle_degrees > 90.0);
                    }
                    if explicit_alone && progress < 1.0 {
                        if !forward && progress == 0.0 {
                            assert_eq!(projection.angle_degrees, 0.0);
                        }
                        let black_face =
                            (forward && progress >= 0.6) || (!forward && progress <= 0.25);
                        if black_face {
                            assert_eq!(projection.showing_back, forward);
                            assert!(
                                projection.meshes.iter().any(|mesh| {
                                    mesh.mesh.texture_id == egui::TextureId::default()
                                        && !mesh.mesh.indices.is_empty()
                                        && mesh.mesh.vertices.iter().all(|vertex| {
                                            vertex.color == egui::Color32::BLACK
                                                && vertex.uv == egui::epaint::WHITE_UV
                                        })
                                }),
                                "{direction:?}/forward={forward}/progress={progress}: black vacant leaf"
                            );
                        }
                        if forward && progress >= 0.6 {
                            let static_page =
                                pair.pages.iter().find(|p| p.occurrence.idx == 8).unwrap();
                            assert!(
                                projection.meshes.iter().any(|mesh| {
                                    mesh.mesh.texture_id == static_page.resource.paint_texture_id()
                                        && !mesh.mesh.indices.is_empty()
                                        && mesh.mesh.vertices.iter().all(|v| v.color.a() == 255)
                                }),
                                "page 9 remains painted beneath the arriving black leaf"
                            );
                        }
                    }
                    if progress == 1.0 {
                        assert_eq!(projection.angle_degrees, 180.0);
                        // Completion is the actual incoming canonical paint:
                        // retain its ordinary underlay, but no extra vacant
                        // black support, old texture or fake receipt.
                        let canonical_material_count = leaf
                            .incoming
                            .pages
                            .iter()
                            .filter(|page| !matches!(page.material, PageMaterial::Vacant))
                            .map(|page| 1 + usize::from(page.underlay.is_some()))
                            .sum::<usize>();
                        assert_eq!(projection.meshes.len(), canonical_material_count);
                        for mesh in &projection.meshes {
                            let bounds = egui::Rect::from_points(
                                &mesh.mesh.vertices.iter().map(|v| v.pos).collect::<Vec<_>>(),
                            );
                            assert!(leaf.incoming.pages.iter().any(|page| {
                                !matches!(page.material, PageMaterial::Vacant)
                                    && bounds == page.rect.intersect(page.clip_rect)
                            }));
                            if mesh.mesh.texture_id != egui::TextureId::default() {
                                let page = incoming
                                    .pages
                                    .iter()
                                    .find(|page| {
                                        page.resource.paint_texture_id() == mesh.mesh.texture_id
                                    })
                                    .expect("terminal texture belongs to the destination unit");
                                assert_eq!(
                                    bounds,
                                    page.transform
                                        .paint_rect
                                        .intersect(page.transform.viewport_rect)
                                );
                            }
                        }
                        assert!(
                            projection
                                .visibility
                                .incoming
                                .iter()
                                .all(|page| { (page.fraction() - 1.0).abs() < 0.0001 })
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn page_turn_merge_master_explicit_singletons_project_without_automatic_cover_proof() {
    use crate::page_turn_animation::project_prepared;

    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        for cover in [false, true] {
            for reordered_hidden in [false, true] {
                let order = if reordered_hidden {
                    vec![4, 0, 2, 1, 3, 5]
                } else {
                    (0..6).collect()
                };
                let first_page = order[0];
                let mut book = BookComposition::new(
                    direction,
                    order.into_iter().map(fixtures::document_page).collect(),
                );
                book.cover_enabled = cover;
                if reordered_hidden {
                    book.hidden_pages.push(fixtures::document_page(0));
                }
                book.page_overrides.push(PageOverrideEntry {
                    page: fixtures::document_page(first_page),
                    directive: PageOverride::Alone,
                });
                let ctx = egui::Context::default();
                install(&ctx);
                let mut app = composed_app(&ctx, &book, 6);
                assert_eq!(
                    app.composed_book_layout().unwrap().visible_pages[0].page,
                    fixtures::document_page(first_page),
                    "cover formation follows the first visible reordered page"
                );
                app.spread_mode = match (direction, cover) {
                    (CompositionDirection::Ltr, false) => SpreadMode::Ltr,
                    (CompositionDirection::Ltr, true) => SpreadMode::LtrCover,
                    (CompositionDirection::Rtl, false) => SpreadMode::Rtl,
                    (CompositionDirection::Rtl, true) => SpreadMode::RtlCover,
                };
                let first = capture_composed_flat(&ctx, &mut app, 0);
                let pair = capture_composed_flat(&ctx, &mut app, 1);
                assert_eq!(
                    first.layout[0].formation,
                    SpreadDisplayUnitFormation::Singleton(
                        SpreadDisplaySingletonCause::ExplicitAlone,
                    )
                );
                assert_eq!(
                    app.spread_display_composition_for_anchor(0)
                        .singleton_placement(),
                    if direction == CompositionDirection::Ltr {
                        SingletonSpreadPlacement::Right
                    } else {
                        SingletonSpreadPlacement::Left
                    },
                    "explicit Alone uses the effective first endpoint policy"
                );
                assert_eq!(pair.layout[1].pages, vec![1, 2]);
                let canonical = canonical_unit(&first).unwrap_or_else(|reason| {
                    panic!("{direction:?}/cover={cover}/hidden={reordered_hidden}: {reason}")
                });
                let actual = canonical
                    .pages
                    .iter()
                    .find(|page| page.occurrence.is_some())
                    .unwrap();
                assert_eq!(
                    actual.rect, first.pages[0].transform.paint_rect,
                    "the common resolver's placement is authoritative"
                );
                for (outgoing, incoming, forward) in [(&first, &pair, true), (&pair, &first, false)]
                {
                    let leaf = prepare_leaf(outgoing, incoming, forward).unwrap_or_else(|reason| {
                        panic!("{direction:?}/cover={cover}/hidden={reordered_hidden}/forward={forward}: {reason}")
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
fn page_turn_merge_master_alone_first_middle_last_follow_one_endpoint_policy() {
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        for cover in [false, true] {
            for animation in [false, true] {
                let mut book =
                    BookComposition::new(direction, (0..4).map(fixtures::document_page).collect());
                book.cover_enabled = cover;
                book.page_overrides = [0, 2, 3]
                    .into_iter()
                    .map(|idx| PageOverrideEntry {
                        page: fixtures::document_page(idx),
                        directive: PageOverride::Alone,
                    })
                    .collect();
                let ctx = egui::Context::default();
                install(&ctx);
                let mut app = composed_app(&ctx, &book, 4);
                app.spread_mode = match (direction, cover) {
                    (CompositionDirection::Ltr, false) => SpreadMode::Ltr,
                    (CompositionDirection::Ltr, true) => SpreadMode::LtrCover,
                    (CompositionDirection::Rtl, false) => SpreadMode::Rtl,
                    (CompositionDirection::Rtl, true) => SpreadMode::RtlCover,
                };
                app.settings.page_turn_animation_enabled = animation;
                app.settings.singleton_spread_first_enabled = false;
                app.settings.singleton_spread_last_enabled = true;
                app.settings.final_cover_spread_enabled = false;
                app.singleton_spread_endpoint_preferences = Default::default();
                let side = if !animation {
                    SingletonSpreadPlacement::Center
                } else if direction == CompositionDirection::Ltr {
                    SingletonSpreadPlacement::Right
                } else {
                    SingletonSpreadPlacement::Left
                };
                for anchor in [0, 2, 3] {
                    let composition = app.spread_display_composition_for_anchor(anchor);
                    assert_eq!(
                        composition.singleton_placement(),
                        side,
                        "{direction:?}/cover={cover}/animation={animation}/anchor={anchor}"
                    );
                    if animation {
                        let unit = capture_composed_flat(&ctx, &mut app, anchor);
                        let canonical = canonical_unit(&unit).unwrap();
                        assert_eq!(
                            canonical.layout,
                            crate::page_turn_animation::UnitLayout::Spread
                        );
                        let actual = canonical
                            .pages
                            .iter()
                            .find(|p| p.occurrence.is_some())
                            .unwrap();
                        assert_eq!(actual.rect, unit.pages[0].transform.paint_rect);
                        assert_eq!(
                            canonical
                                .pages
                                .iter()
                                .filter(|p| p.occurrence.is_some())
                                .count(),
                            1
                        );
                    } else {
                        app.fullscreen_idx = Some(anchor);
                        frame(&ctx, |ui, ctx| {
                            let (_, sources) = draw_current(&mut app, ui, ctx);
                            assert_eq!(sources.pages.len(), 1);
                            assert!(
                                matches!(&sources.submission, FsDisplayUnitSubmission::Paged(receipt) if !receipt.curl)
                            );
                        });
                    }
                }
            }
        }
    }
}

#[test]
fn page_turn_merge_master_shared_builder_controls_presentation_and_prefetch() {
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        for directive in [
            PageOverride::Alone,
            PageOverride::BlankOpposite {
                page_side: PageSide::Leading,
            },
            PageOverride::BlankOpposite {
                page_side: PageSide::Trailing,
            },
        ] {
            for final_cover in [false, true] {
                let mut book =
                    BookComposition::new(direction, (0..12).map(fixtures::document_page).collect());
                book.cover_enabled = true;
                book.page_overrides = vec![
                    PageOverrideEntry {
                        page: fixtures::document_page(0),
                        directive: PageOverride::Alone,
                    },
                    PageOverrideEntry {
                        page: fixtures::document_page(11),
                        directive,
                    },
                ];
                let ctx = egui::Context::default();
                let mut app = composed_app(&ctx, &book, 12);
                app.spread_mode = if direction == CompositionDirection::Ltr {
                    SpreadMode::LtrCover
                } else {
                    SpreadMode::RtlCover
                };
                app.settings.final_cover_spread_enabled = final_cover;
                app.settings.last_page_alone_enabled = true;
                app.settings.page_after_cover_alone_enabled = false;
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
                for animation in [true, false] {
                    app.settings.page_turn_animation_enabled = animation;
                    assert_eq!(app.spread_display_composition_for_anchor(9), pair);
                    assert_eq!(
                        app.spread_display_composition_for_anchor(0)
                            .singleton_placement(),
                        if !animation {
                            SingletonSpreadPlacement::Center
                        } else if direction == CompositionDirection::Ltr {
                            SingletonSpreadPlacement::Right
                        } else {
                            SingletonSpreadPlacement::Left
                        },
                    );
                }
                app.settings.page_turn_animation_enabled = true;
                let blank = matches!(directive, PageOverride::BlankOpposite { .. });
                let supplemented = final_cover && !blank;
                let tail = app.spread_display_composition_for_anchor(11);
                assert_eq!(tail.navigation_pages(), vec![11]);
                assert_eq!(tail.singleton_placement().has_white_companion(), blank);
                if let PageOverride::BlankOpposite { page_side } = directive {
                    let right = (page_side == PageSide::Leading)
                        == (direction == CompositionDirection::Rtl);
                    assert_eq!(
                        tail.singleton_placement().side(),
                        if right {
                            SingletonSpreadPlacement::Right
                        } else {
                            SingletonSpreadPlacement::Left
                        }
                    );
                } else {
                    assert_eq!(
                        tail.singleton_placement(),
                        if supplemented {
                            SingletonSpreadPlacement::Center
                        } else if direction == CompositionDirection::Ltr {
                            SingletonSpreadPlacement::Right
                        } else {
                            SingletonSpreadPlacement::Left
                        }
                    );
                }
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
                assert_eq!(
                    tail.presentation_demand(app.spread_mode, crate::page_split::PageSlice::Full)
                        .len(),
                    if supplemented { 2 } else { 1 },
                    "explicit blank has no permission/source demand and wins over final cover"
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

                // Unlike old saved Pair units, the common builder reflects late
                // geometry, and a saved quarter-turn restores ordinary pairing.
                app.record_page_dims_for_spread(9, (1700, 1200));
                app.rotation_cache
                    .insert(9, crate::rotation_db::Rotation::None);
                app.reconcile_spread_landscapes_from_rotation_cache();
                assert_eq!(
                    app.spread_display_composition_for_anchor(9)
                        .navigation_pages(),
                    vec![9]
                );
                app.rotation_cache
                    .insert(9, crate::rotation_db::Rotation::Cw90);
                app.reconcile_spread_landscapes_from_rotation_cache();
                assert_eq!(app.spread_display_composition_for_anchor(9), pair);

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
                            .presentation_demand(
                                app.spread_mode,
                                crate::page_split::PageSlice::Full
                            )
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
}

#[test]
fn page_turn_merge_master_explicit_blank_turns_both_ways_with_only_real_receipts() {
    let _serial = crate::key_input::lock_test_input();
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        for cover in [false, true] {
            for page_side in [PageSide::Leading, PageSide::Trailing] {
                for blank_idx in [0, 3] {
                    let mut book = BookComposition::new(
                        direction,
                        (0..4).map(fixtures::document_page).collect(),
                    );
                    book.cover_enabled = cover;
                    book.page_overrides = [0, 3]
                        .into_iter()
                        .map(|idx| PageOverrideEntry {
                            page: fixtures::document_page(idx),
                            directive: if idx == blank_idx {
                                PageOverride::BlankOpposite { page_side }
                            } else {
                                PageOverride::Alone
                            },
                        })
                        .collect();
                    let ctx = egui::Context::default();
                    install(&ctx);
                    let mut app = composed_app(&ctx, &book, 4);
                    app.spread_mode = match (direction, cover) {
                        (CompositionDirection::Ltr, false) => SpreadMode::Ltr,
                        (CompositionDirection::Ltr, true) => SpreadMode::LtrCover,
                        (CompositionDirection::Rtl, false) => SpreadMode::Rtl,
                        (CompositionDirection::Rtl, true) => SpreadMode::RtlCover,
                    };
                    app.settings.final_cover_spread_enabled = true;
                    let blank_idx = blank_idx as usize;
                    let blank = capture_composed_flat(&ctx, &mut app, blank_idx);
                    let (white, _) = blank.white.expect("ordinary paint includes explicit white");
                    let real = blank.pages[0].transform.paint_rect;
                    let right = (page_side == PageSide::Leading)
                        == (direction == CompositionDirection::Rtl);
                    assert_eq!(real.center().x > white.center().x, right);
                    assert_eq!(
                        blank.pages.len(),
                        1,
                        "final cover cannot replace explicit white"
                    );
                    let canonical = canonical_unit(&blank).unwrap();
                    assert_eq!(canonical.pages.len(), 2);
                    assert_eq!(
                        canonical
                            .pages
                            .iter()
                            .filter(|page| page.occurrence.is_some())
                            .count(),
                        1,
                        "white is paint, never a navigation occurrence"
                    );
                    for (from, target) in [(1, blank_idx), (blank_idx, 1)] {
                        capture_composed_flat(&ctx, &mut app, from);
                        take_test_events();
                        let operation = ViewerOperationId::Pad {
                            batch: next_operation_batch(),
                            ordinal: 0,
                        };
                        frame(&ctx, |ui, ctx| {
                            app.begin_page_turn_viewer_frame(ctx, false, None);
                            app.page_turn_external_operation(
                                ctx,
                                operation,
                                OperationPhase::Press,
                                "merge",
                            );
                            app.handle_fs_navigation_with_operation(
                                ctx,
                                false,
                                false,
                                None,
                                None,
                                None,
                                FsPageNav::Target(target),
                                None,
                                from,
                                Some(operation),
                            );
                            assert!(matches!(
                                app.page_turn_presentation.state,
                                PresentationState::Offered { .. }
                            ));
                            assert_eq!(app.fullscreen_idx, Some(target));
                            let (composition, sources) = draw_current(&mut app, ui, ctx);
                            let FsDisplayUnitSubmission::Paged(receipt) = &sources.submission
                            else {
                                panic!("paged white companion submission");
                            };
                            assert!(receipt.curl);
                            assert_eq!(receipt.pages().len(), sources.pages.len());
                            assert!(receipt.pages().iter().all(|(occurrence, _, _, _)| {
                                occurrence.role == SpreadPageRole::Navigation
                            }));
                            if target == blank_idx {
                                assert_eq!(sources.pages.len(), 1);
                                assert_eq!(sources.pages[0].occurrence.idx, blank_idx);
                            }
                            app.emit_fs_page_turn_ready_for_display_unit(
                                ctx,
                                &composition,
                                FsPageTurnDecision::normal(),
                                &sources,
                            );
                        });
                        let PresentationState::Turning {
                            clock,
                            incoming,
                            offer,
                            projection,
                            ..
                        } = &mut app.page_turn_presentation.state
                        else {
                            panic!("explicit white must turn");
                        };
                        assert_eq!(offer.forward, target > from);
                        assert_eq!(offer.ready_sources.0.len(), incoming.pages.len());
                        let white_canonical = if target == blank_idx {
                            &projection.incoming
                        } else {
                            &projection.outgoing
                        };
                        let paper = white_canonical
                            .pages
                            .iter()
                            .find(|p| p.occurrence.is_none())
                            .unwrap();
                        assert!(
                            matches!(paper.material, crate::page_turn_animation::PageMaterial::Blank { color } if color == egui::Color32::WHITE)
                        );
                        let tracker = offer.submission.clone();
                        assert_eq!(tracker.status(), SubmissionStatus::Started);
                        *clock = MotionClock::Running {
                            start_at: Instant::now() - offer.duration - Duration::from_millis(1),
                        };
                        frame(&ctx, |ui, ctx| {
                            app.begin_page_turn_viewer_frame(ctx, false, None);
                            let (composition, sources) = draw_current(&mut app, ui, ctx);
                            let FsDisplayUnitSubmission::Paged(receipt) = &sources.submission
                            else {
                                panic!("canonical white companion completion");
                            };
                            assert!(!receipt.curl);
                            assert_eq!(receipt.pages().len(), sources.pages.len());
                            assert!(receipt.pages().iter().all(|(_, _, _, area)| *area > 0.0));
                            let decision = app.fs_page_turn_decision_for_frame(ctx, target);
                            app.emit_fs_page_turn_ready_for_display_unit(
                                ctx,
                                &composition,
                                decision,
                                &sources,
                            );
                        });
                        assert_eq!(tracker.status(), SubmissionStatus::Finished);
                        let events = take_test_events();
                        assert!(
                            events.iter().any(|(fact, kind, reason)| {
                                fact.operation_id == operation
                                    && *kind == "start"
                                    && *reason == "curl_submitted"
                            }),
                            "{direction:?}/cover={cover}/{page_side:?}/{from}->{target}: {events:?}"
                        );
                        assert!(
                            !events
                                .iter()
                                .any(|(fact, kind, _)| fact.operation_id == operation
                                    && matches!(*kind, "skip" | "cancel"))
                        );
                        let completed = capture_composed_flat(&ctx, &mut app, target);
                        assert_eq!(completed.white.is_some(), target == blank_idx);
                        if target == blank_idx {
                            assert_eq!(completed.white, blank.white);
                        }
                        assert_eq!(app.fullscreen_idx, Some(target));
                    }
                }
            }
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
        app.grid_scroll.set_reveal_requested(false);
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
            assert!(!app.grid_scroll.reveal_requested());
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
        assert!(app.grid_scroll.reveal_requested());
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

// Saved schema-2 input from the user's 2026-10-14 coverless PDF report.
// Page numbers in this fixture are raw, zero-based, not projected indices.
fn saved_coverless_book() -> BookComposition {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/book_composition_coverless.json"
    ))
    .unwrap()
}

fn saved_pdf_viewer(ctx: &egui::Context, book: &BookComposition) -> crate::app::AppTestEnvForTest {
    let mut app = composed_app(ctx, book, 40);
    app.spread_mode = if book.cover_enabled {
        SpreadMode::LtrCover
    } else {
        SpreadMode::Ltr
    }
    .with_reading_direction(composition_view_direction(book.direction));
    app.native_video_in_window_active = true;
    app.viewer_presentation = crate::app::ViewerPresentation::MainWindow;
    app.settings.video_in_window_mode = true;
    app.settings.singleton_spread_first_enabled = false;
    app.settings.singleton_spread_last_enabled = false;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
    app.settings.fullscreen_navigator_visible = false;
    app.settings.still_seek_strip_visible = false;
    for idx in 0..app.items.len() {
        let image = Arc::new(egui::ColorImage::filled([80, 113], egui::Color32::GREEN));
        let tex = ctx.load_texture(
            format!("saved-coverless-{idx}"),
            image.clone(),
            egui::TextureOptions::LINEAR,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                pdf_rerender_failures: Vec::new(),
                pdf_metadata: None,
                tex,
                pixels: image,
                source_dims: Some([576, 816]),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.record_page_dims_for_spread(idx, (576, 816));
        app.record_page_layout_dims_for_spread(idx, (576, 816));
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
    }
    app
}

fn saved_pdf_index(app: &App, raw_page: u32) -> usize {
    app.composed_book_layout()
        .unwrap()
        .visible_pages
        .iter()
        .position(|page| page.page == fixtures::document_page(raw_page))
        .unwrap()
}

fn paint_saved_pdf_viewport(
    app: &mut App,
    ctx: &egui::Context,
) -> Vec<crate::app::paint_record_test_support::PaintRecord> {
    let output = crate::app::paint_record_test_support::with_capture(|| {
        crate::test_ui::headless_paint_output(ctx.run_pass(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1093.0, 614.0),
                )),
                ..Default::default()
            },
            |ctx| ctx.root_ui(|root_ui| app.render_fullscreen_viewport(root_ui)),
        ))
    });
    crate::app::paint_record_test_support::paint_records(egui::ViewportId::ROOT, &output)
}

fn assert_saved_pdf_singleton_paint(
    app: &mut App,
    ctx: &egui::Context,
    raw_page: u32,
    side: SingletonSpreadPlacement,
) {
    let idx = saved_pdf_index(app, raw_page);
    // The production frame trims the fixture's overfull 38-page cache to
    // its ordinary resident window. Model the already decoded target before
    // each observation, rather than dispatching I/O to a nonexistent PDF.
    if !matches!(app.fs_cache.get(&idx), Some(FsCacheEntry::Static { .. })) {
        let pixels = Arc::new(egui::ColorImage::filled([80, 113], egui::Color32::GREEN));
        let tex = ctx.load_texture(
            format!("saved-ready-{idx}"),
            pixels.clone(),
            egui::TextureOptions::LINEAR,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                pdf_rerender_failures: Vec::new(),
                pdf_metadata: None,
                tex,
                pixels,
                source_dims: Some([576, 816]),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
    }
    app.fullscreen_idx = Some(idx);
    let composition = app.spread_display_composition_for_anchor(idx);
    assert_eq!(composition.navigation_pages(), vec![idx]);
    assert_eq!(
        composition.singleton_placement(),
        side,
        "builder raw={raw_page}"
    );
    let demand = app.book_target_presentation_demand(idx);
    app.install_allowed_presentation_for_test(&demand);
    paint_saved_pdf_viewport(app, ctx);
    let records = paint_saved_pdf_viewport(app, ctx);
    let transform = app
        .fullscreen_page_layout
        .single_page()
        .unwrap_or_else(|| {
            panic!(
                "actual viewport singleton raw={raw_page}, mode={:?}, animation={}, first={:?}/{}",
                app.spread_mode,
                app.settings.page_turn_animation_enabled,
                app.singleton_spread_endpoint_preferences.first,
                app.settings.singleton_spread_first_enabled
            )
        })
        .transform;
    assert_eq!(transform.page_idx, idx);
    let actual_side = match transform.placement {
        ResolvedDisplayPlacement::SingletonSpread { side, .. } => side,
        ResolvedDisplayPlacement::Normal { .. } => SingletonSpreadPlacement::Center,
        other => panic!("ordinary PDF viewport: {other:?}"),
    };
    assert_eq!(actual_side, side, "paint raw={raw_page}");
    let proof = app.test_paint_provenance_for_idx(idx).unwrap();
    let real = records
        .iter()
        .find(|record| record.provenance == proof)
        .expect("source-owned image primitive");
    let painted = egui::Rect::from_points(&real.vertices.iter().map(|v| v.pos).collect::<Vec<_>>());
    assert_eq!(
        painted, transform.paint_rect,
        "actual primitive, not only layout metadata"
    );
    let delta = painted.center().x - transform.viewport_rect.center().x;
    match side {
        SingletonSpreadPlacement::Right => assert!(delta > 10.0, "raw={raw_page}, {transform:?}"),
        SingletonSpreadPlacement::Left => assert!(delta < -10.0, "raw={raw_page}, {transform:?}"),
        SingletonSpreadPlacement::Center => {
            assert!(delta.abs() <= 1.0, "raw={raw_page}, {transform:?}")
        }
        _ => panic!("Alone has no explicit white companion"),
    }
}

#[test]
fn page_turn_saved_coverless_alone_actual_viewport_placement() {
    let _serial = crate::key_input::lock_test_input();
    let book = saved_coverless_book();
    assert!(!book.cover_enabled);
    assert_eq!(book.page_order.len(), 40);
    let ctx = egui::Context::default();
    install(&ctx);
    install_fs_navigator_input_tracking(&ctx);
    let _pdf_rerender = crate::app::PdfRerenderRequestProbe::start();
    let mut app = saved_pdf_viewer(&ctx, &book);
    assert_eq!(app.items.len(), 38);
    assert_eq!(saved_pdf_index(&app, 10), 8);
    assert_eq!(saved_pdf_index(&app, 11), 9);
    assert_eq!(saved_pdf_index(&app, 17), 15);
    // Warm the real builder, then use the actual prepared presentation and
    // ordinary viewport draw for consecutive Alone and later Alone pages.
    app.spread_display_composition_for_anchor(0);
    for raw_page in [10, 11, 17] {
        assert_saved_pdf_singleton_paint(&mut app, &ctx, raw_page, SingletonSpreadPlacement::Right);
    }
    // Page 15 (raw 14) is the reported middle singleton: the leading white
    // override on Page 16 prevents it from acquiring a partner.
    assert_saved_pdf_singleton_paint(&mut app, &ctx, 14, SingletonSpreadPlacement::Left);
    let next = saved_pdf_index(&app, 12);
    assert_eq!(
        app.spread_display_composition_for_anchor(next)
            .navigation_pages(),
        vec![next, saved_pdf_index(&app, 13)]
    );
    app.fullscreen_idx = Some(next);
    let demand = app.book_target_presentation_demand(next);
    app.install_allowed_presentation_for_test(&demand);
    paint_saved_pdf_viewport(&mut app, &ctx);
    assert_eq!(app.fullscreen_page_layout.displayed_pages().count(), 2);
}

#[test]
fn page_turn_saved_coverless_alone_viewport_endpoint_matrix() {
    use crate::settings::SingletonSpreadPlacementPreference as Pref;
    let _serial = crate::key_input::lock_test_input();
    let _pdf_rerender = crate::app::PdfRerenderRequestProbe::start();
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        for cover in [false, true] {
            let mut book = saved_coverless_book();
            book.direction = direction;
            book.cover_enabled = cover;
            book.page_overrides.push(PageOverrideEntry {
                page: fixtures::document_page(39),
                directive: PageOverride::Alone,
            });
            let ctx = egui::Context::default();
            install(&ctx);
            install_fs_navigator_input_tracking(&ctx);
            let mut app = saved_pdf_viewer(&ctx, &book);
            // Keep the terminal Alone a real single leaf; supplement behavior
            // has separate regressions and is unavailable without a cover.
            app.settings.final_cover_spread_enabled = false;
            app.settings.singleton_spread_last_enabled = true;
            for animation in [true, false] {
                app.settings.page_turn_animation_enabled = animation;
                for global in [false, true] {
                    app.settings.singleton_spread_first_enabled = global;
                    for preference in [Pref::FollowGlobal, Pref::Center, Pref::Place] {
                        app.singleton_spread_endpoint_preferences.first = preference;
                        let side = if animation || preference.effective(global) {
                            if direction == CompositionDirection::Ltr {
                                SingletonSpreadPlacement::Right
                            } else {
                                SingletonSpreadPlacement::Left
                            }
                        } else {
                            SingletonSpreadPlacement::Center
                        };
                        for raw_page in [10, 11, 17, 39] {
                            assert_saved_pdf_singleton_paint(&mut app, &ctx, raw_page, side);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn page_turn_saved_coverless_boundary_remainder_actual_viewport_matrix() {
    use crate::settings::SingletonSpreadPlacementPreference as Pref;
    let _serial = crate::key_input::lock_test_input();
    let _pdf_rerender = crate::app::PdfRerenderRequestProbe::start();
    for direction in [CompositionDirection::Ltr, CompositionDirection::Rtl] {
        for cover in [false, true] {
            let mut book = saved_coverless_book();
            book.direction = direction;
            book.cover_enabled = cover;
            let ctx = egui::Context::default();
            install(&ctx);
            install_fs_navigator_input_tracking(&ctx);
            let mut app = saved_pdf_viewer(&ctx, &book);
            app.settings.final_cover_spread_enabled = false;
            app.singleton_spread_endpoint_preferences.first = Pref::Center;
            for animation in [true, false] {
                app.settings.page_turn_animation_enabled = animation;
                for global in [false, true] {
                    app.settings.singleton_spread_last_enabled = global;
                    for preference in [Pref::FollowGlobal, Pref::Center, Pref::Place] {
                        app.singleton_spread_endpoint_preferences.last = preference;
                        let side = if animation || preference.effective(global) {
                            if direction == CompositionDirection::Ltr {
                                SingletonSpreadPlacement::Left
                            } else {
                                SingletonSpreadPlacement::Right
                            }
                        } else {
                            SingletonSpreadPlacement::Center
                        };
                        assert_saved_pdf_singleton_paint(&mut app, &ctx, 14, side);
                        let nav = app.get_nav_indices();
                        let units = app.build_spread_display_units_for_nav(&nav);
                        let idx = saved_pdf_index(&app, 14);
                        let unit = units.iter().find(|unit| unit.contains_idx(idx)).unwrap();
                        assert_eq!(
                            unit.formation,
                            SpreadDisplayUnitFormation::Singleton(
                                SpreadDisplaySingletonCause::OverrideBoundaryRemainder
                            )
                        );
                    }
                }
            }
            // A genuinely landscape page stays centered before the directive.
            let idx = saved_pdf_index(&app, 14);
            let pixels = Arc::new(egui::ColorImage::filled([113, 80], egui::Color32::GREEN));
            let tex = ctx.load_texture(
                "wide-boundary",
                pixels.clone(),
                egui::TextureOptions::LINEAR,
            );
            app.fs_cache.insert(
                idx,
                FsCacheEntry::Static {
                    pdf_rerender_failures: Vec::new(),
                    pdf_metadata: None,
                    tex,
                    pixels,
                    source_dims: Some([816, 576]),
                    load_seq: 1,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
            app.record_page_dims_for_spread(idx, (816, 576));
            app.record_page_layout_dims_for_spread(idx, (816, 576));
            app.settings.page_turn_animation_enabled = true;
            app.settings.singleton_spread_last_enabled = true;
            app.singleton_spread_endpoint_preferences.last = Pref::Place;
            assert_saved_pdf_singleton_paint(&mut app, &ctx, 14, SingletonSpreadPlacement::Center);
        }
    }
}
