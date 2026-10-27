#![cfg(all(test, windows))]

use super::section375_tests::{fixture, settle};
use super::viewer_context_registry::{ContextAsyncOwner, ContextAsyncServicePhase};
use super::*;
use book_append_refresh::{FolderRefreshCause, FolderRefreshRequest};
use std::time::{Duration, Instant, SystemTime};

fn observation_service(app: &mut App, ctx: &egui::Context) {
    ContextAsyncOwner::FolderObservation.service(app, ctx, ContextAsyncServicePhase::Background);
}

fn attach_media(app: &mut App, window: u64) -> (usize, usize) {
    let idx = app
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Video(_) | GridItem::Audio(_)))
        .unwrap();
    let path = app.items[idx].drag_source_path().unwrap().to_path_buf();
    let source = if matches!(app.items[idx], GridItem::Audio(_)) {
        DetachedSource::Audio
    } else {
        DetachedSource::Video
    };
    let mut player = crate::video::VideoPlayer::stream_ready_disconnected_for_test(path);
    player.configure_native_timing_for_test(17.0, 120.0, true, false);
    app.fs_cache.insert(
        idx,
        FsCacheEntry::Video {
            player: Box::new(player),
            load_seq: 0,
        },
    );
    // Capture the heap identity, which must survive both bundle swaps.
    let identity = match app.fs_cache.get(&idx).unwrap() {
        FsCacheEntry::Video { player, .. } => {
            (&**player as *const crate::video::VideoPlayer) as usize
        }
        _ => unreachable!("media fixture has no player"),
    };
    app.fullscreen_idx = Some(idx);
    app.selected = Some(idx);
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    app.settings.fullfeature_media_window = true;
    app.begin_mounted_detached_session_for_test(window, source);
    (idx, identity)
}

fn assert_player(app: &App, idx: usize, identity: usize) {
    assert_eq!(app.fullscreen_idx, Some(idx));
    let Some(FsCacheEntry::Video { player, .. }) = app.fs_cache.get(&idx) else {
        panic!("reader lost player")
    };
    assert_eq!(
        (&**player as *const crate::video::VideoPlayer) as usize,
        identity
    );
    assert!(player.is_playing(), "reader paused its surviving player");
    let position = player.position_secs();
    assert!(
        position.is_finite() && (17.0..120.0).contains(&position),
        "reader reset its playing position: {position}"
    );
}

fn finish_return(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        settle(app, ctx);
        if let Some(result) = app.poll_folder_nav() {
            app.apply_folder_nav_result(ctx, result);
        }
        if app.folder_nav_pending.is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && app.top_level_grid_view.open_path_classification().is_none()
            && !app.sidecar_restore_active()
        {
            break;
        }
        assert!(Instant::now() < deadline, "search return did not settle");
        std::thread::yield_now();
    }
    settle(app, ctx);
}

#[test]
fn section1375_c1_observation_distinguishes_physical_child_and_synthetic_root() {
    let (mut app, folder, _) = fixture(&["a.jpg"]);
    app.top_level_grid_view
        .replace_surface(top_level_grid_view::TopLevelGridSurface::Snapshot);
    app.projection_recipe = book_materialization::ProjectionRecipe::OrdinaryList;
    assert!(
        app.folder_observation_allowed(),
        "physical child of fixed root"
    );
    app.projection_recipe = book_materialization::ProjectionRecipe::ExtractedResult(
        book_materialization::ExtractedSurface::Snapshot,
    );
    assert!(!app.folder_observation_allowed(), "fixed root itself");
    app.projection_recipe = book_materialization::ProjectionRecipe::OrdinaryList;
    for source in [
        drive_list_synthetic_path(),
        reading_history_synthetic_path(),
        bookmark_view_synthetic_path(),
    ] {
        app.current_folder = Some(source);
        assert!(!app.folder_observation_allowed(), "synthetic row owner");
    }
    app.current_folder = Some(folder);
    assert!(app.folder_observation_allowed());
}

#[test]
fn section1375_c1_search_close_promotes_once_and_remount_never_observes() {
    for everything in [false, true] {
        for media in ["movie.mp4", "music.flac"] {
            let (mut app, folder, ctx) = fixture(&["cover.jpg"]);
            let old = app.viewer_context_main();
            let recipe = book_materialization::ProjectionRecipe::ExtractedResult(
                book_materialization::ExtractedSurface::Search(
                    top_level_grid_view::TopLevelSearchView::Global,
                ),
            );
            let search_folder = app.tmp.path().join("search-source");
            std::fs::create_dir(&search_folder).unwrap();
            let search_media = search_folder.join(media);
            std::fs::write(&search_media, b"search-media").unwrap();
            app.adopt_c1_search_results_for_test(
                &ctx,
                std::slice::from_ref(&search_media),
                everything,
            );
            assert!(app.global_search.active);
            assert_eq!(app.projection_recipe, recipe);
            assert_eq!(app.items.len(), 1);
            assert_eq!(
                app.items[0].drag_source_path(),
                Some(search_media.as_path())
            );
            assert!(app.items_are_global_search_view);
            let (idx, player) = attach_media(&mut app, 5301);
            app.close_global_search();
            finish_return(&mut app, &ctx);
            let main = app.viewer_context_main();
            assert_ne!(
                main, old,
                "search close must perform the first real promote"
            );
            assert_eq!(app.locate_window_context(5301).unwrap().0, old);
            assert_eq!(app.projected_viewer_context_id(), main);
            assert_eq!(app.current_folder.as_ref(), Some(&folder));
            app.ensure_current_folder_watch(&ctx);
            settle(&mut app, &ctx);
            let epoch = app.current_folder_watch.as_ref().unwrap().epoch;
            let generation = app.items_generation;
            let main_rows = app.folder_observation.as_ref().unwrap().rows.clone();
            app.with_viewer_context(old, |reader| {
                assert_eq!(reader.projection_recipe, recipe);
                assert_eq!(reader.items.len(), 1);
                assert_eq!(
                    reader.items[0].drag_source_path(),
                    Some(search_media.as_path())
                );
                assert!(!reader.items_are_global_search_view);
                assert!(!reader.global_search.active);
                assert_eq!(reader.navigation_scope, ViewerNavigationScope::Main);
                let rows = reader.items.clone();
                let generation = reader.items_generation;
                for _ in 0..3 {
                    observation_service(reader, &ctx);
                    reader.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
                    reader.check_external_folder_changes(&ctx, ExternalChangeCheck::Resumed);
                    reader.start_external_rescan(&ctx, folder.clone(), SystemTime::UNIX_EPOCH);
                    reader.consume_folder_refresh_pending(&ctx);
                }
                assert!(reader.current_folder_watch.is_none());
                assert!(reader.external_rescan_pending.is_none());
                assert!(reader.folder_refresh_pending.is_none());
                assert_eq!(reader.items_generation, generation);
                assert_eq!(reader.items.len(), rows.len());
                assert_player(reader, idx, player);
            })
            .unwrap();
            assert_eq!(
                app.viewer_context_main(),
                main,
                "second promote is forbidden"
            );
            assert_eq!(app.projected_viewer_context_id(), main);
            assert_eq!(app.items_generation, generation);
            assert_eq!(app.current_folder_watch.as_ref().unwrap().epoch, epoch);
            assert!(Arc::ptr_eq(
                &main_rows,
                &app.folder_observation.as_ref().unwrap().rows
            ));
            // The new main keeps a fully functional subscription after the reader frame.
            std::fs::write(folder.join("new.jpg"), b"changed").unwrap();
            app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
            settle(&mut app, &ctx);
            assert!(
                app.items
                    .iter()
                    .any(|item| item.drag_source_path() == Some(folder.join("new.jpg").as_path()))
            );
        }
    }
}

#[test]
fn section1375_c1_promoted_physical_reader_ignores_actual_change_and_late_restart() {
    let (mut app, folder, ctx) = fixture(&["movie.mp4", "a.jpg"]);
    let old = app.viewer_context_main();
    app.ensure_current_folder_watch(&ctx);
    settle(&mut app, &ctx);
    let (idx, player) = attach_media(&mut app, 5302);
    app.start_external_rescan(&ctx, folder.clone(), SystemTime::UNIX_EPOCH);
    let cancel = app.external_rescan_pending.as_ref().unwrap().cancel.clone();
    app.external_rescan_pending
        .as_mut()
        .unwrap()
        .restart_requested = true;
    assert!(app.promote_active_detached_video_for_main_context_change());
    let main = app.viewer_context_main();
    app.load_folder(folder.clone());
    settle(&mut app, &ctx);
    app.ensure_current_folder_watch(&ctx);
    settle(&mut app, &ctx);
    let epoch = app.current_folder_watch.as_ref().unwrap().epoch;
    let main_generation = app.items_generation;
    std::fs::remove_file(folder.join("a.jpg")).unwrap();
    std::fs::write(folder.join("b.jpg"), b"changed").unwrap();
    app.with_viewer_context(old, |reader| {
        let generation = reader.items_generation;
        for _ in 0..3 {
            observation_service(reader, &ctx);
        }
        reader.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
        reader.check_external_folder_changes(&ctx, ExternalChangeCheck::Resumed);
        assert!(reader.current_folder_watch.is_none());
        assert!(reader.external_rescan_pending.is_none());
        assert!(reader.folder_refresh_pending.is_none());
        assert_eq!(reader.items_generation, generation);
        assert!(
            reader
                .items
                .iter()
                .any(|item| item.drag_source_path() == Some(folder.join("a.jpg").as_path()))
        );
        assert_player(reader, idx, player);
    })
    .unwrap();
    assert!(cancel.load(Ordering::Relaxed));
    assert_eq!(app.viewer_context_main(), main);
    assert_eq!(app.items_generation, main_generation);
    assert_eq!(app.current_folder_watch.as_ref().unwrap().epoch, epoch);
    app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
    settle(&mut app, &ctx);
    assert!(
        app.items
            .iter()
            .any(|item| item.drag_source_path() == Some(folder.join("b.jpg").as_path()))
    );
}

#[test]
fn section1375_c1_organize_scan_before_promote_retains_operation_on_remount() {
    for operation in [
        crate::shell_file_ops::ShellTransferOperation::Copy,
        crate::shell_file_ops::ShellTransferOperation::Move,
    ] {
        let (mut app, folder, ctx) = fixture(&["movie.mp4", "a.jpg"]);
        let old = app.viewer_context_main();
        app.ensure_current_folder_watch(&ctx);
        settle(&mut app, &ctx);
        let (idx, player) = attach_media(&mut app, 5303);
        // Deliver a successful copy through the actual organize completion handler.
        std::fs::write(folder.join("organized.jpg"), b"copy").unwrap();
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(crate::shell_file_ops::ShellTransferOutcome {
            aborted: false,
        }))
        .unwrap();
        app.file_organize_request = crate::ui_dialogs::file_organize::FileOrganizeRequest::Running(
            Box::new(crate::ui_dialogs::file_organize::FileOrganizeRunning {
                request: crate::shell_file_ops::ShellTransferRequest {
                    operation,
                    sources: vec![folder.join("a.jpg")],
                    destination: folder.clone(),
                },
                rx,
            }),
        );
        app.poll_file_organize(&ctx);
        assert!(matches!(
            app.file_organize_request,
            crate::ui_dialogs::file_organize::FileOrganizeRequest::Hidden
        ));
        assert!(matches!(
            app.external_rescan_pending.as_ref().unwrap().cause,
            FolderRefreshCause::Operation
        ));
        app.external_rescan_pending
            .as_mut()
            .unwrap()
            .restart_requested = true;
        assert!(app.promote_active_detached_video_for_main_context_change());
        let main = app.viewer_context_main();
        app.with_viewer_context(old, |reader| {
            settle(reader, &ctx);
            observation_service(reader, &ctx);
            reader.consume_folder_refresh_pending(&ctx);
            assert!(reader.current_folder_watch.is_none());
            assert!(matches!(
                reader.folder_refresh_pending.as_ref().unwrap().cause,
                FolderRefreshCause::Operation
            ));
            assert_player(reader, idx, player);
        })
        .unwrap();
        assert_eq!(app.viewer_context_main(), main);
    }
}

#[test]
fn section1375_c1_invalidated_append_pending_survives_promote_passive_poll_and_park() {
    let (mut app, folder, ctx) = fixture(&["movie.mp4"]);
    let old = app.viewer_context_main();
    let (idx, player) = attach_media(&mut app, 5304);
    // Receipt-free Append terminal is still active delivery. It is deliberately not External.
    assert!(app.admit_book_append_followup_mutation());
    assert!(app.book_append_undo_receipt.is_none());
    app.retain_folder_refresh(FolderRefreshRequest {
        folder: folder.clone(),
        cause: FolderRefreshCause::BookAppendExternal,
    });
    assert!(app.promote_active_detached_video_for_main_context_change());
    for _ in 0..2 {
        app.with_viewer_context(old, |reader| {
            reader.pause_mounted_background_work_keep_current_frame();
            observation_service(reader, &ctx);
            reader.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
            reader.consume_folder_refresh_pending(&ctx);
            assert!(reader.current_folder_watch.is_none());
            assert!(reader.external_rescan_pending.is_none());
            assert!(matches!(
                reader.folder_refresh_pending.as_ref().unwrap().cause,
                FolderRefreshCause::BookAppendExternal
            ));
            assert!(reader.book_append_undo_receipt.is_none());
            assert_player(reader, idx, player);
        })
        .unwrap();
    }
}

#[test]
fn section1375_c1_active_cause_coalescing_keeps_operation_delivery() {
    for (left, right, append) in [
        (
            FolderRefreshCause::External,
            FolderRefreshCause::Operation,
            false,
        ),
        (
            FolderRefreshCause::Operation,
            FolderRefreshCause::External,
            false,
        ),
        (
            FolderRefreshCause::BookAppendExternal,
            FolderRefreshCause::External,
            true,
        ),
        (
            FolderRefreshCause::Operation,
            FolderRefreshCause::BookAppendExternal,
            true,
        ),
    ] {
        let (mut app, folder, ctx) = fixture(&["movie.mp4"]);
        attach_media(&mut app, 5305);
        app.retain_folder_refresh(FolderRefreshRequest {
            folder: folder.clone(),
            cause: left,
        });
        app.retain_folder_refresh(FolderRefreshRequest {
            folder,
            cause: right,
        });
        assert!(app.promote_active_detached_video_for_main_context_change());
        app.with_active_viewer_context(|reader| {
            observation_service(reader, &ctx);
            let cause = &reader.folder_refresh_pending.as_ref().unwrap().cause;
            assert!(!cause.is_passive_observation());
            assert_eq!(cause.is_append_delivery(), append);
            assert!(reader.current_folder_watch.is_none());
        })
        .unwrap();
    }
}

#[test]
fn section1375_c1_completed_passive_response_keeps_later_operation_through_restart() {
    let (mut app, folder, ctx) = fixture(&["a.jpg"]);
    let generation = app.items_generation;
    app.start_external_rescan(&ctx, folder.clone(), SystemTime::UNIX_EPOCH);
    // Freeze the real worker's completed reply before admitting the later operation.
    // The payload therefore still says External, while the request owner becomes Operation.
    let response = app
        .external_rescan_pending
        .as_ref()
        .unwrap()
        .rx
        .recv_timeout(Duration::from_secs(20))
        .unwrap();
    assert!(matches!(
        response,
        Some(ExternalRescanResult::Equal(
            _,
            _,
            FolderRefreshCause::External,
            _
        ))
    ));
    let (tx, rx) = mpsc::channel();
    tx.send(response).unwrap();
    app.external_rescan_pending.as_mut().unwrap().rx = rx;
    app.start_external_rescan_with_cause(
        &ctx,
        folder,
        SystemTime::UNIX_EPOCH,
        FolderRefreshCause::Operation,
    );
    let pending = app.external_rescan_pending.as_ref().unwrap();
    assert!(pending.restart_requested);
    assert!(matches!(pending.cause, FolderRefreshCause::Operation));
    app.poll_external_rescan(&ctx);
    let restarted = app
        .external_rescan_pending
        .as_ref()
        .expect("merged operation must restart");
    assert!(matches!(restarted.cause, FolderRefreshCause::Operation));
    assert!(!restarted.restart_requested);
    assert_eq!(
        app.items_generation, generation,
        "Equal must keep the installed list"
    );
    settle(&mut app, &ctx);
    assert_eq!(app.items_generation, generation);
}
