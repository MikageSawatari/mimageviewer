// Kept with the scroll regression files; included under collection_navigation's
// tests so the existing actor and continuation fixtures remain private.
use super::*;
use crate::ui_fullscreen::{FsInitialPlacementKind, FsPageLanding};

fn image_collection(
    temp: &tempfile::TempDir,
) -> (
    AppTestEnvForTest,
    crate::collection_store::CollectionStoreClient,
    Arc<CollectionPreparedSnapshot>,
) {
    let paths = ["a.jpg", "b.jpg", "c.jpg", "d.jpg"].map(|name| temp.path().join(name));
    for path in &paths {
        image::RgbImage::new(90, 160).save(path).unwrap();
    }
    let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
    let created = recv(client.create_collection("scroll arrivals".into()).unwrap());
    let registrations = paths
        .iter()
        .map(|path| {
            crate::collection_store::CollectionRegistration::from_trusted_path(
                path,
                CollectionResolvedKind::Image,
            )
            .unwrap()
        })
        .collect();
    let added = recv(
        client
            .add_batch(created.collection_id(), created.revision(), registrations)
            .unwrap(),
    );
    let prepared = prepare_snapshot(&added.snapshot);
    app.top_level_grid_view.begin(
        TopLevelGridSurface::Collection(CollectionGridIdentity {
            collection_id: prepared.collection_id,
        }),
        None,
    );
    app.apply_collection_grid_prepared(Arc::clone(&prepared), None);
    app.spread_mode = crate::settings::SpreadMode::Single;
    app.reading_flow = crate::settings::ReadingFlow::Paged;
    app.settings.fullscreen_page_start_position =
        crate::settings::FullscreenPageStartPosition::Center;
    app.settings.page_turn_animation_enabled = false;
    app.fullscreen_idx = Some(1);
    (app, client, prepared)
}

fn poll_navigation(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.top_level_grid_view.collection_navigation_pending() {
        assert!(
            Instant::now() < deadline,
            "collection scroll navigation did not complete"
        );
        app.poll_collection_navigation(ctx);
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn continuation(
    app: &App,
    prepared: &CollectionPreparedSnapshot,
    page_landing: Option<FsPageLanding>,
    steps: i32,
    wrap: bool,
) -> CollectionNavigationPending {
    let target_idx = app.fullscreen_idx.unwrap();
    CollectionNavigationPending::AwaitingManualContinuation {
        steps,
        landing: ManualMediaNavigationLanding::Fullscreen,
        page_landing,
        display_unit_step: true,
        still_only: false,
        wrap,
        stamp: CollectionNavigationContinuationStamp {
            context_id: app.collection_grid_context_id(),
            collection_id: prepared.collection_id,
            surface_generation: app.top_level_grid_view.generation(),
            items_generation: app.items_generation,
            intent_sequence: app.top_level_grid_view.collection_navigation_sequence(),
            target_idx,
            target: App::collection_navigation_identity_at(prepared, target_idx).unwrap(),
        },
    }
}

#[test]
fn collection_scroll_accumulator_matches_landing_and_direction_in_both_owners() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let temp = tempfile::tempdir().unwrap();
    let (mut app, client, prepared) = image_collection(&temp);
    for page_landing in [None, Some(FsPageLanding::Start), Some(FsPageLanding::End)] {
        for delta in [-1, 1] {
            for waiting_for_landing in [false, true] {
                let mut pending = if waiting_for_landing {
                    continuation(&app, &prepared, page_landing, delta, false)
                } else {
                    let mut request = manual_navigation_request(&mut app, 1);
                    request.action = CollectionNavigationAction::Manual {
                        fs_idx: 1,
                        delta,
                        queued_steps: 0,
                        display_unit_step: true,
                        landing: ManualMediaNavigationLanding::Fullscreen,
                        page_landing,
                        still_only: false,
                        wrap: false,
                    };
                    CollectionNavigationPending::RequestNeeded {
                        request,
                        watch: Some(client.subscribe().unwrap()),
                    }
                };
                for different in [None, Some(FsPageLanding::Start), Some(FsPageLanding::End)] {
                    if different != page_landing {
                        assert!(!pending.accumulate_manual_with_landing(
                            delta,
                            ManualMediaNavigationLanding::Fullscreen,
                            false,
                            true,
                            false,
                            different,
                        ));
                    }
                }
                assert!(!pending.accumulate_manual_with_landing(
                    -delta,
                    ManualMediaNavigationLanding::Fullscreen,
                    false,
                    true,
                    false,
                    page_landing,
                ));
                assert!(pending.accumulate_manual_with_landing(
                    delta,
                    ManualMediaNavigationLanding::Fullscreen,
                    false,
                    true,
                    false,
                    page_landing,
                ));
                match &pending {
                    CollectionNavigationPending::AwaitingManualContinuation {
                        steps,
                        page_landing: actual,
                        ..
                    } => {
                        assert_eq!(*steps, delta * 2);
                        assert_eq!(*actual, page_landing);
                    }
                    _ => assert!(matches!(
                        pending.action(),
                        Some(CollectionNavigationAction::Manual {
                            queued_steps,
                            page_landing: actual,
                            ..
                        }) if *queued_steps == delta && *actual == page_landing
                    )),
                }
            }
        }
    }
    app.shutdown_collection_runtime_for_exit();
}

#[test]
fn collection_scroll_actor_queue_preserves_start_and_end_until_the_final_entry() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    for (current, delta, page_landing, expected, kind) in [
        (
            0,
            1,
            FsPageLanding::Start,
            2,
            FsInitialPlacementKind::LandingStart,
        ),
        (
            3,
            -1,
            FsPageLanding::End,
            1,
            FsInitialPlacementKind::LandingEnd,
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, _client, _) = image_collection(&temp);
        app.fullscreen_idx = Some(current);
        let ctx = egui::Context::default();
        for _ in 0..2 {
            assert!(app.start_collection_manual_display_unit_navigation_with_landing(
                &ctx,
                current,
                delta,
                ManualMediaNavigationLanding::Fullscreen,
                false,
                Some(page_landing),
            ));
        }
        let pending = app
            .top_level_grid_view
            .take_collection_navigation_pending()
            .unwrap();
        assert!(matches!(
            pending.action(),
            Some(CollectionNavigationAction::Manual {
                queued_steps,
                page_landing: actual,
                ..
            }) if *queued_steps == delta && *actual == Some(page_landing)
        ));
        app.top_level_grid_view
            .set_collection_navigation_pending(Some(pending));
        poll_navigation(&mut app, &ctx);
        assert_eq!(app.fullscreen_idx, Some(expected));
        assert_eq!(app.fs_initial_placement_request.unwrap().kind, kind);
        app.shutdown_collection_runtime_for_exit();
    }
}

#[test]
fn collection_scroll_different_landing_replaces_the_pending_request() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    for (previous, next) in [
        (Some(FsPageLanding::Start), None),
        (None, Some(FsPageLanding::End)),
        (Some(FsPageLanding::Start), Some(FsPageLanding::End)),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, _client, _) = image_collection(&temp);
        let ctx = egui::Context::default();
        assert!(app.start_collection_manual_display_unit_navigation_with_landing(
            &ctx,
            1,
            1,
            ManualMediaNavigationLanding::Fullscreen,
            false,
            previous,
        ));
        let previous_sequence = app.top_level_grid_view.collection_navigation_sequence();
        assert!(app.start_collection_manual_display_unit_navigation_with_landing(
            &ctx,
            1,
            1,
            ManualMediaNavigationLanding::Fullscreen,
            false,
            next,
        ));
        assert_ne!(
            app.top_level_grid_view.collection_navigation_sequence(),
            previous_sequence
        );
        let pending = app
            .top_level_grid_view
            .take_collection_navigation_pending()
            .unwrap();
        assert!(matches!(
            pending.action(),
            Some(CollectionNavigationAction::Manual {
                queued_steps: 0,
                page_landing: actual,
                ..
            }) if *actual == next
        ));
        app.top_level_grid_view
            .set_collection_navigation_pending(Some(pending));
        poll_navigation(&mut app, &ctx);
        assert_eq!(app.fullscreen_idx, Some(2));
        assert_eq!(
            app.fs_initial_placement_request.map(|request| request.kind),
            next.map(|landing| match landing {
                FsPageLanding::Start => FsInitialPlacementKind::LandingStart,
                FsPageLanding::End => FsInitialPlacementKind::LandingEnd,
            })
        );
        app.shutdown_collection_runtime_for_exit();
    }
}

#[test]
fn collection_scroll_wrap_replacement_replaces_the_page_landing_together_with_steps() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let temp = tempfile::tempdir().unwrap();
    let (mut app, _client, prepared) = image_collection(&temp);
    let pending = continuation(&app, &prepared, Some(FsPageLanding::End), 3, false);
    app.top_level_grid_view
        .set_collection_navigation_pending(Some(pending));
    let ctx = egui::Context::default();
    assert!(app.start_collection_manual_display_unit_navigation_with_landing(
        &ctx,
        1,
        1,
        ManualMediaNavigationLanding::Fullscreen,
        true,
        Some(FsPageLanding::Start),
    ));
    let pending = app
        .top_level_grid_view
        .take_collection_navigation_pending()
        .unwrap();
    assert!(matches!(
        &pending,
        CollectionNavigationPending::AwaitingManualContinuation {
            steps: 1,
            wrap: true,
            page_landing: Some(FsPageLanding::Start),
            ..
        }
    ));
    app.top_level_grid_view
        .set_collection_navigation_pending(Some(pending));
    poll_navigation(&mut app, &ctx);
    assert_eq!(app.fullscreen_idx, Some(2));
    assert_eq!(
        app.fs_initial_placement_request.unwrap().kind,
        FsInitialPlacementKind::LandingStart
    );
    app.shutdown_collection_runtime_for_exit();
}

#[test]
fn collection_scroll_generation_change_discards_both_request_and_continuation_landing() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    for waiting_for_landing in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, _client, prepared) = image_collection(&temp);
        let ctx = egui::Context::default();
        if waiting_for_landing {
            let pending = continuation(&app, &prepared, Some(FsPageLanding::End), -1, false);
            app.top_level_grid_view
                .set_collection_navigation_pending(Some(pending));
        } else {
            assert!(app.start_collection_manual_display_unit_navigation_with_landing(
                &ctx,
                1,
                1,
                ManualMediaNavigationLanding::Fullscreen,
                false,
                Some(FsPageLanding::Start),
            ));
        }
        app.items_generation += 1;
        poll_navigation(&mut app, &ctx);
        assert_eq!(app.fullscreen_idx, Some(1));
        assert!(app.fs_initial_placement_request.is_none());
        app.shutdown_collection_runtime_for_exit();
    }
}
