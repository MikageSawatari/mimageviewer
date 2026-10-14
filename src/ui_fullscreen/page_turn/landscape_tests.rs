//! Normal composition / offer / prepared submission regressions for the reported book.
use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, frame, ready_app};

fn book(
    ctx: &egui::Context,
    mode: SpreadMode,
    side_endpoint: bool,
    rotate: Option<(usize, crate::rotation_db::Rotation)>,
) -> crate::app::AppTestEnvForTest {
    let mut app = ready_app(ctx, mode);
    app.items = (0..10)
        .map(|i| GridItem::Image(PathBuf::from(format!("c:/page-turn-landscape/{i:02}.png"))))
        .collect();
    app.visible_indices = (0..10).collect();
    app.items_generation += 1;
    app.fs_cache.clear();
    app.thumbnails = (0..10).map(|_| ThumbnailState::Pending).collect();
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.settings.singleton_spread_last_enabled = side_endpoint;
    for idx in 0..10 {
        let dims = if idx == 4 { [160, 90] } else { [90, 130] };
        let image =
            egui::ColorImage::filled(dims, egui::Color32::from_rgb(40 + idx as u8 * 15, 100, 160));
        let texture = ctx.load_texture(
            format!("landscape-book-{idx}"),
            image.clone(),
            egui::TextureOptions::LINEAR,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex: texture,
                pixels: Arc::new(image),
                source_dims: Some(dims),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.rotation_cache.insert(
            idx,
            rotate
                .filter(|(i, _)| *i == idx)
                .map_or(crate::rotation_db::Rotation::None, |(_, r)| r),
        );
        app.record_page_dims_for_spread(idx, (dims[0] as u32, dims[1] as u32));
    }
    app.reconcile_spread_landscapes_from_rotation_cache();
    install(ctx);
    app
}

fn draw_observed(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
) -> FsNavigatorTextureSources {
    let (composition, sources) = draw_current(app, ui, ctx);
    let decision = app.fs_page_turn_decision_for_frame(ctx, app.fullscreen_idx.unwrap());
    // The real fullscreen handler connects its submitted unit to the ordinary
    // navigation latch. Drive that owner too, rather than leaving a fake stop pending.
    app.emit_fs_page_turn_ready_for_display_unit(ctx, &composition, decision, &sources);
    sources
}

fn started_and_finished(ctx: &egui::Context, app: &mut App, from: usize, to: usize, forward: bool) {
    // Both units pass through ordinary rendering first: no hand-built geometry/state.
    for anchor in [to, from] {
        app.fullscreen_idx = Some(anchor);
        frame(ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            draw_observed(app, ui, ctx);
        });
    }
    take_test_events();
    let operation = ViewerOperationId::Slideshow(next_operation_batch());
    let actual_count = app
        .spread_display_composition_for_anchor(to)
        .pages_in_reading_order()
        .len();
    frame(ctx, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        app.page_turn_external_operation(
            ctx,
            operation,
            OperationPhase::Press,
            "landscape-regression",
        );
        app.handle_fs_navigation_with_operation(
            ctx,
            false,
            false,
            None,
            None,
            None,
            FsPageNav::Target(to),
            None,
            from,
            Some(operation),
        );
        assert_eq!(
            app.fullscreen_idx,
            Some(to),
            "accepted navigation {from}->{to}"
        );
        let sources = draw_observed(app, ui, ctx);
        let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
            panic!("paged submission")
        };
        assert!(
            receipt.curl,
            "ordinary transition {from}->{to}, forward={forward}, events={:?}",
            take_test_events()
        );
        assert_eq!(
            receipt.pages().len(),
            actual_count,
            "halves must not create source occurrences"
        );
    });
    let tracker = match &mut app.page_turn_presentation.state {
        PresentationState::Turning {
            offer,
            projection,
            incoming,
            ..
        } => {
            assert_eq!(offer.forward, forward);
            assert_eq!(offer.submission.status(), SubmissionStatus::Started);
            if actual_count == 1 {
                assert_eq!(
                    projection.incoming.pages.len(),
                    if incoming.mode.is_spread() { 2 } else { 1 }
                );
                let ids: Vec<_> = projection
                    .incoming
                    .pages
                    .iter()
                    .filter_map(|p| match &p.material {
                        crate::page_turn_animation::PageMaterial::Texture {
                            texture_id, ..
                        } => Some(*texture_id),
                        _ => None,
                    })
                    .collect();
                if ids.len() == 2 {
                    assert_eq!(ids[0], ids[1], "one texture shared by both halves");
                }
            }
            offer.accepted_at -= offer.duration + Duration::from_millis(1);
            offer.submission.clone()
        }
        _ => panic!("started turn"),
    };
    frame(ctx, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        let sources = draw_observed(app, ui, ctx);
        let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
            panic!("canonical flat")
        };
        assert!(!receipt.curl);
        assert_eq!(receipt.pages().len(), actual_count);
        assert!(receipt.pages().iter().all(|(_, _, _, area)| *area > 0.0));
    });
    assert_eq!(tracker.status(), SubmissionStatus::Finished);
    let events = take_test_events();
    let events: Vec<_> = events
        .iter()
        .filter(|(fact, _, _)| fact.operation_id == operation)
        .collect();
    for kind in ["opportunity", "offered", "start", "finish", "fully_visible"] {
        assert_eq!(
            events.iter().filter(|(_, k, _)| *k == kind).count(),
            1,
            "{from}->{to} {kind}: {events:?}"
        );
    }
    assert!(
        !events
            .iter()
            .any(|(_, k, _)| matches!(*k, "skip" | "cancel")),
        "{events:?}"
    );
    // Complete the ordinary target latch before preparing another test boundary.
    frame(ctx, |_, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
    });
}

#[test]
fn page_turn_reported_landscape_mid_book_all_five_boundaries_complete_in_both_directions() {
    let mut completed = 0;
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        for side in [false, true] {
            let ctx = egui::Context::default();
            let mut app = book(&ctx, mode, side, None);
            let units = app.build_spread_display_units_for_nav(&(0..10).collect::<Vec<_>>());
            let anchors: Vec<_> = units.iter().map(|u| u.anchor_idx()).collect();
            assert_eq!(anchors, vec![0, 2, 4, 5, 7, 9]);
            for pair in anchors.windows(2) {
                started_and_finished(&ctx, &mut app, pair[0], pair[1], true);
                completed += 1;
            }
            for pair in anchors.windows(2).rev() {
                started_and_finished(&ctx, &mut app, pair[1], pair[0], false);
                completed += 1;
            }
        }
    }
    assert_eq!(completed, 40);
}

#[test]
fn page_turn_rotated_portrait_and_landscape_follow_normal_composition_and_complete() {
    let mut completed = 0;
    use crate::rotation_db::Rotation::*;
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        for (rotated_idx, rotation) in [4, 5, 6]
            .into_iter()
            .flat_map(|idx| [Cw90, Cw180, Cw270].into_iter().map(move |r| (idx, r)))
        {
            let ctx = egui::Context::default();
            // Rotated page07 matches the user's observed session. 180 preserves portrait;
            // 90/270 make a landscape singleton without changing source/history identity.
            let mut app = book(&ctx, mode, false, Some((rotated_idx, rotation)));
            let units = app.build_spread_display_units_for_nav(&(0..10).collect::<Vec<_>>());
            let anchors: Vec<_> = units.iter().map(|u| u.anchor_idx()).collect();
            for pair in anchors.windows(2) {
                started_and_finished(&ctx, &mut app, pair[0], pair[1], true);
                completed += 1;
            }
            for pair in anchors.windows(2).rev() {
                started_and_finished(&ctx, &mut app, pair[1], pair[0], false);
                completed += 1;
            }
        }
    }
    assert_eq!(completed, 180);
}

#[test]
fn page_turn_landscape_faces_aggregate_into_one_physical_visibility_receipt() {
    use crate::page_turn_animation::{PageVisibility, VisibilityReceipt};
    let receipt = VisibilityReceipt {
        incoming: vec![
            PageVisibility {
                occurrence: 0,
                visible_area: 30.0,
                canonical_visible_area: 100.0,
                short_edge: 7.0,
            },
            PageVisibility {
                occurrence: 1,
                visible_area: 60.0,
                canonical_visible_area: 100.0,
                short_edge: 12.0,
            },
        ],
        outgoing_front_area: 0.0,
        outgoing_front_canonical_area: 1.0,
    };
    let actual = actual_page_visibility(&receipt, 1);
    assert_eq!(actual.len(), 1);
    assert_eq!(actual[0].occurrence, 0);
    assert_eq!(actual[0].visible_area, 90.0);
    assert_eq!(actual[0].canonical_visible_area, 200.0);
    assert_eq!(actual[0].short_edge, 12.0);
    assert_eq!(actual_page_visibility(&receipt, 2).len(), 2);
}

#[test]
fn page_turn_single_portrait_landscape_eight_routes_start_and_complete() {
    let mut completed = 0;
    for reading_direction in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
        let ctx = egui::Context::default();
        let mut app = book(&ctx, SpreadMode::Single, false, None);
        app.reading_direction = reading_direction;
        for (from, to, forward) in [(3, 4, true), (4, 3, false), (4, 5, true), (5, 4, false)] {
            started_and_finished(&ctx, &mut app, from, to, forward);
            completed += 1;
        }
    }
    assert_eq!(completed, 8);
}

#[test]
fn page_turn_duration_preferences_change_cancels_through_existing_flat_completion() {
    use crate::ui_fullscreen::page_turn_submission_tests::seed_leaf;
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    seed_leaf(&ctx, &mut app);
    let tracker = match &app.page_turn_presentation.state {
        PresentationState::Turning { offer, .. } => offer.submission.clone(),
        _ => panic!("active leaf"),
    };
    let mut edited = app.settings.clone();
    edited.page_turn_animation_duration_ms += 100;
    app.install_preferences_settings(edited);
    assert_eq!(tracker.status(), SubmissionStatus::Cancelled);
    assert!(app.page_turn_completion_active());
    frame(&ctx, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        let sources = draw_observed(&mut app, ui, ctx);
        let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
            panic!("canonical flat");
        };
        assert!(!receipt.curl);
    });
    frame(&ctx, |_, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
    });
    assert!(!app.page_turn_completion_active());
    started_and_finished(&ctx, &mut app, 1, 2, true);
}

#[test]
fn page_turn_shape_gutter_difference_with_current_content_starts_and_completes() {
    let mut completed = 0;
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        for (from, to, forward) in [(2, 4, true), (4, 2, false), (4, 5, true), (5, 4, false)] {
            let ctx = egui::Context::default();
            let mut app = book(&ctx, mode, false, None);
            let mut gutters = Vec::new();
            for anchor in [from, to] {
                app.fullscreen_idx = Some(anchor);
                frame(&ctx, |ui, ctx| {
                    app.begin_page_turn_viewer_frame(ctx, false, None);
                    draw_observed(&mut app, ui, ctx);
                });
                let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
                    panic!("ordinary flat");
                };
                assert!(unit.content.iter().all(|c| c.gap == 12));
                gutters.push(canonical_unit(unit).unwrap().gutter_width);
            }
            assert_ne!(gutters[0], gutters[1], "shape changes the resolved gutter");
            assert_eq!(
                gutters[usize::from(from != 4)],
                0.0,
                "landscape has no gutter"
            );
            started_and_finished(&ctx, &mut app, from, to, forward);
            completed += 1;
        }
    }
    assert_eq!(completed, 8);
}

#[test]
fn page_turn_gap_setting_change_before_repaint_skips_stale_outgoing_at_acceptance() {
    let mut skipped = 0;
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        for (a, b) in [(0, 2), (2, 4), (4, 5)] {
            for (from, to) in [(a, b), (b, a)] {
                for new_gap in [0, 24] {
                    let ctx = egui::Context::default();
                    let mut app = book(&ctx, mode, false, None);
                    for anchor in [to, from] {
                        app.fullscreen_idx = Some(anchor);
                        frame(&ctx, |ui, ctx| {
                            app.begin_page_turn_viewer_frame(ctx, false, None);
                            draw_observed(&mut app, ui, ctx);
                        });
                    }
                    let PresentationState::Flat(outgoing) = &app.page_turn_presentation.state
                    else {
                        panic!("ordinary outgoing flat");
                    };
                    let outgoing = outgoing.clone();
                    assert!(outgoing.content.iter().all(|c| c.gap == 12));
                    assert_eq!(outgoing.reserved_bytes.load(Ordering::Relaxed), 0);
                    let leases_before = app.page_turn_leases.sources.len();
                    take_test_events();
                    let operation = ViewerOperationId::Slideshow(next_operation_batch());
                    frame(&ctx, |ui, ctx| {
                        app.begin_page_turn_viewer_frame(ctx, false, None);
                        // Settings can change before the saved outgoing is repainted.
                        app.settings.spread_page_gap_px = new_gap;
                        app.page_turn_external_operation(
                            ctx,
                            operation,
                            OperationPhase::Press,
                            "gap-regression",
                        );
                        app.handle_fs_navigation_with_operation(
                            ctx,
                            false,
                            false,
                            None,
                            None,
                            None,
                            FsPageNav::Target(to),
                            None,
                            from,
                            Some(operation),
                        );
                        assert_eq!(
                            app.fullscreen_idx,
                            Some(to),
                            "ordinary navigation still succeeds"
                        );
                        let PresentationState::Flat(retained) = &app.page_turn_presentation.state
                        else {
                            panic!("stale outgoing must be rejected before an offer");
                        };
                        assert!(Arc::ptr_eq(retained, &outgoing));
                        assert_eq!(outgoing.reserved_bytes.load(Ordering::Relaxed), 0);
                        assert_eq!(app.page_turn_leases.sources.len(), leases_before);
                        let sources = draw_observed(&mut app, ui, ctx);
                        let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                            panic!("normal target flat");
                        };
                        assert!(!receipt.curl);
                        let PresentationState::Flat(incoming) = &app.page_turn_presentation.state
                        else {
                            panic!("normal target replaces stale outgoing");
                        };
                        assert!(incoming.content.iter().all(|c| c.gap == new_gap));
                    });
                    let events = take_test_events();
                    let events: Vec<_> = events
                        .iter()
                        .filter(|(f, _, _)| f.operation_id == operation)
                        .collect();
                    assert_eq!(
                        events.iter().filter(|(_, kind, _)| *kind == "skip").count(),
                        1,
                        "{events:?}"
                    );
                    assert!(
                        events.iter().any(|(_, kind, reason)| *kind == "skip"
                            && *reason == "outgoing_content_changed"),
                        "{events:?}"
                    );
                    assert!(
                        !events
                            .iter()
                            .any(|(_, kind, _)| matches!(*kind, "offered" | "start" | "cancel")),
                        "{events:?}"
                    );
                    skipped += 1;
                }
            }
        }
    }
    assert_eq!(skipped, 24);
}
